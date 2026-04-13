import { createSignal, Show, For, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { AutoProgress } from "./ProgressBar";
import { downloads, cancelGameDownload } from "../stores/downloads";
import { activeJobs, cancelContentPackJob } from "../stores/contentPacks";
import { formatBytes } from "../util";

interface ActiveDownload {
  id: string;
  label: string;
  progress: number;
  status: string;
  speed: string;
  type: "game" | "content-pack";
}

// Speed snapshots live outside the reactive graph so they persist across renders.
let prevSnapshot: Record<string, { bytes: number; time: number }> = {};

export function DownloadIndicator() {
  const [showSheet, setShowSheet] = createSignal(false);
  const [speeds, setSpeeds] = createSignal<Record<string, string>>({});

  // Snapshot bytes every 2 seconds (independent of the reactive graph) and
  // compute speed by comparing against the previous snapshot.
  const speedInterval = setInterval(() => {
    const now = Date.now();
    const newSpeeds: Record<string, string> = {};
    const newSnapshot: Record<string, { bytes: number; time: number }> = {};

    const jobs = activeJobs();
    for (const [key, job] of Object.entries(jobs)) {
      if (!job.finished && job.phase === "downloading") {
        const id = `cp:${key}`;
        newSnapshot[id] = { bytes: job.downloaded_bytes, time: now };
        const prev = prevSnapshot[id];
        if (prev) {
          const dt = (now - prev.time) / 1000;
          if (dt > 0.5) {
            const bps = (job.downloaded_bytes - prev.bytes) / dt;
            if (bps > 0) {
              newSpeeds[id] = `${formatBytes(Math.round(bps))}/s`;
            }
          }
        }
      }
    }

    prevSnapshot = newSnapshot;
    setSpeeds(newSpeeds);
  }, 2000);

  onCleanup(() => clearInterval(speedInterval));

  const activeDownloads = (): ActiveDownload[] => {
    const result: ActiveDownload[] = [];

    // Content pack downloads.
    const jobs = activeJobs();
    for (const [key, job] of Object.entries(jobs)) {
      if (!job.finished) {
        const pct = Math.round((job.progress ?? 0) * 100);
        let status = job.phase;
        if (job.phase === "downloading") { status = `${pct}%`; }
        // Fall back to a capitalized pack_id if no display name was provided.
        const fallback = (key.split(":")[1] ?? key).replace(/^./, (c) => c.toUpperCase());
        result.push({
          id: `cp:${key}`,
          label: job.label ?? fallback,
          progress: job.progress ?? 0,
          status,
          speed: speeds()[`cp:${key}`] ?? "",
          type: "content-pack",
        });
      }
    }

    // Game downloads.
    const dl = downloads();
    for (const [id, state] of Object.entries(dl)) {
      if (state.downloading) {
        result.push({
          id: `game:${id}`,
          label: state.title ?? `Game #${id}`,
          progress: state.progress,
          status: state.status,
          speed: "",
          type: "game",
        });
      }
    }

    return result;
  };

  const totalCount = () => activeDownloads().length;
  const avgProgress = () => {
    const list = activeDownloads();
    if (list.length === 0) { return 0; }
    return list.reduce((sum, d) => sum + d.progress, 0) / list.length;
  };

  const handleCancel = (dl: ActiveDownload) => {
    if (dl.type === "content-pack") {
      const parts = dl.id.replace("cp:", "").split(":");
      if (parts.length === 2) {
        cancelContentPackJob(parts[0], parts[1]);
      }
    } else {
      const gameId = parseInt(dl.id.replace("game:", ""));
      if (!isNaN(gameId)) {
        cancelGameDownload(gameId);
      }
    }
  };

  return (
    <Show when={totalCount() > 0}>
      <button
        class="download-indicator"
        onClick={() => setShowSheet(!showSheet())}
        title={`${totalCount()} download${totalCount() > 1 ? "s" : ""} in progress`}
      >
        <AutoProgress value={avgProgress()} class="indicator-progress" />
        <span class="download-indicator-count">{totalCount()}</span>
      </button>

      <Show when={showSheet()}>
        <Portal>
          <div class="download-sheet-backdrop" onClick={() => setShowSheet(false)} />
          <div class="download-sheet">
            <div class="download-sheet-header">
              <span>Downloads</span>
            </div>
            <For each={activeDownloads()}>
              {(dl) => (
                <div class="download-sheet-row">
                  <div class="download-sheet-info">
                    <span class="download-sheet-label">{dl.label}</span>
                    <div class="download-sheet-progress-row">
                      <AutoProgress value={dl.progress} class="mini" />
                      <span class="download-sheet-status">{dl.status}</span>
                      <Show when={dl.speed}>
                        <span class="download-sheet-speed">{dl.speed}</span>
                      </Show>
                    </div>
                  </div>
                  <button
                    class="download-sheet-cancel"
                    onClick={() => handleCancel(dl)}
                    title="Cancel"
                  >✕</button>
                </div>
              )}
            </For>
          </div>
        </Portal>
      </Show>
    </Show>
  );
}
