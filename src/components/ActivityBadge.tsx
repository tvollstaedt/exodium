import { createMemo, createSignal, Show, Index, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { Tooltip } from "@ark-ui/solid/tooltip";
import { AutoProgress } from "./ProgressBar";
import { isOffline } from "../stores/network";
import { transferStats, isTransferring, formatRate } from "../stores/transfer";
import { downloads, cancelGameDownload } from "../stores/downloads";
import { activeJobs, cancelContentPackJob } from "../stores/contentPacks";
import { seedingOn } from "../stores/seeding";
import { setOpenGameRequest } from "../stores/music";
import { formatBytes } from "../util";

interface Props {
  /** The sheet's "Network settings" shortcut - the pill itself opens the sheet. */
  onOpenSettings: () => void;
}

interface ActiveDownload {
  id: string;
  /** The game behind a "game" row; content packs have none. */
  gameId?: number;
  label: string;
  progress: number;
  status: string;
  speed: string;
  type: "game" | "content-pack";
}

const IconDown = () => (
  <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3">
    <path stroke-linecap="round" stroke-linejoin="round" d="M12 4v15m0 0l-6-6m6 6l6-6" />
  </svg>
);

const IconUp = () => (
  <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3">
    <path stroke-linecap="round" stroke-linejoin="round" d="M12 20V5m0 0l-6 6m6-6l6 6" />
  </svg>
);

/** The top bar's network pill: offline state, transfer rates, and a
 *  progress chip while anything downloads. Click opens the downloads sheet. */
export function ActivityBadge(props: Props) {
  const [showSheet, setShowSheet] = createSignal(false);
  const [speeds, setSpeeds] = createSignal<Record<string, string>>({});

  // Speed snapshots live outside the reactive graph so they persist across renders.
  let prevSnapshot: Record<string, { bytes: number; time: number }> = {};

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
        const prev = prevSnapshot[id];
        // Hold the baseline until bytes move: progress lands in whole pieces,
        // and re-basing every tick read 0 between them.
        const advanced = !prev || job.downloaded_bytes > prev.bytes;
        newSnapshot[id] = advanced ? { bytes: job.downloaded_bytes, time: now } : prev;
        if (prev) {
          const dt = (now - prev.time) / 1000;
          if (dt > 0.5) {
            const bps = Math.max(0, (job.downloaded_bytes - prev.bytes) / dt);
            newSpeeds[id] = `${formatBytes(Math.round(bps))}/s`;
          } else {
            // Too soon to measure - keep what is on screen rather than blank it.
            newSpeeds[id] = speeds()[id] ?? "";
          }
        }
      }
    }

    prevSnapshot = newSnapshot;
    // The badge is mounted for the app's whole lifetime - don't publish a
    // fresh (empty) object every idle tick, it would invalidate every
    // consumer of activeDownloads() for nothing.
    if (Object.keys(newSpeeds).length > 0 || Object.keys(speeds()).length > 0) {
      setSpeeds(newSpeeds);
    }
  }, 2000);

  onCleanup(() => clearInterval(speedInterval));

  // Memo, not a getter: rebuilt once per store tick instead of once per
  // consumer - the pill count, average, sheet gate and sheet list all read it,
  // and downloads()/activeJobs() update at 1 Hz during a download.
  const activeDownloads = createMemo((): ActiveDownload[] => {
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
          gameId: Number(id),
          label: state.title ?? `Game #${id}`,
          progress: state.progress,
          status: state.status,
          speed: "",
          type: "game",
        });
      }
    }

    return result;
  });

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

  const s = () => transferStats();
  /** Anchored to jobs, not rates: rates dip to zero between piece bursts. */
  const busy = () => totalCount() > 0;

  // `isTransferring` (sticky, see stores/transfer.ts) covers traffic with no
  // job behind it - seeding, or a preview video being streamed.
  const moving = () => !!s() && (busy() || isTransferring());

  /** Status line: sharing state first (a zero rate is ambiguous), then the
   *  peer count, which proves liveness when rates cannot. */
  const peers = (n: number) => `${n} peer${n === 1 ? "" : "s"}`;

  const statusLine = () => {
    if (isOffline()) { return "Offline - no downloads, previews, or sharing."; }
    const v = s();
    if (!v?.active) { return "Online - no torrent running."; }
    const shared = `${formatBytes(v.uploaded_bytes)} shared this session.`;
    if (!seedingOn()) { return "Sharing is off - downloading only."; }
    if (moving()) { return `${peers(v.peers)} - ${shared}`; }
    if (v.peers === 0) { return "Online, looking for peers."; }
    return `Sharing with ${peers(v.peers)} - nothing requested right now. ${shared}`;
  };

  return (
    <>
      <Tooltip.Root openDelay={300}>
        <Tooltip.Trigger asChild={(triggerProps) =>
          <button
            {...triggerProps()}
            class={`net-badge ${isOffline() ? "net-badge--offline" : "net-badge--online"}`}
            onClick={() => setShowSheet(!showSheet())}
          >
            <span class="net-badge-dot" classList={{ "is-active": !isOffline() && moving() }} />
            <Show when={!isOffline()} fallback={<>Offline</>}>
              <Show when={moving()} fallback={<>Online</>}>
                <span class="net-badge-rate"><IconDown />{formatRate(s()!.download_bps)}</span>
                <span class="net-badge-rate"><IconUp />{formatRate(s()!.upload_bps)}</span>
              </Show>
            </Show>
            <Show when={totalCount() > 0}>
              <span class="net-badge-dl">
                <AutoProgress value={avgProgress()} class="indicator-progress" />
                <span class="net-badge-dl-count">{totalCount()}</span>
              </span>
            </Show>
          </button>
        } />
        <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">
          {statusLine()}
        </Tooltip.Content></Tooltip.Positioner></Portal>
      </Tooltip.Root>

      <Show when={showSheet()}>
        <Portal>
          <div class="download-sheet-backdrop" onClick={() => setShowSheet(false)} />
          <div class="download-sheet">
            <div class="download-sheet-header">
              <span>Downloads</span>
            </div>
            <Show when={totalCount() > 0} fallback={
              <div class="download-sheet-empty">No active downloads.</div>
            }>
              {/* Index, not For: activeDownloads() builds fresh objects every
                  poll tick, and For keys by reference - it recreated every
                  row's DOM each second, restarting the progress bar animation
                  (visible as "jumping" bars on stalled downloads). Index keeps
                  the DOM and updates values in place. */}
              <Index each={activeDownloads()}>
                {(dl) => (
                  <div class="download-sheet-row">
                    <div class="download-sheet-info">
                      {/* A game row opens its detail panel - the same request
                          the player bar's cover makes (Library listens). */}
                      <Show when={dl().gameId != null} fallback={<span class="download-sheet-label">{dl().label}</span>}>
                        <button
                          class="download-sheet-label is-link"
                          title="Show game"
                          onClick={() => { setShowSheet(false); setOpenGameRequest(dl().gameId!); }}
                        >{dl().label}</button>
                      </Show>
                      <div class="download-sheet-progress-row">
                        <AutoProgress value={dl().progress} class="mini" />
                        <span class="download-sheet-status">{dl().status}</span>
                        <Show when={dl().speed}>
                          <span class="download-sheet-speed">{dl().speed}</span>
                        </Show>
                      </div>
                    </div>
                    <button
                      class="download-sheet-cancel"
                      onClick={() => handleCancel(dl())}
                      title="Cancel"
                    >✕</button>
                  </div>
                )}
              </Index>
            </Show>
            <div class="download-sheet-footer">
              <span class="download-sheet-net">{statusLine()}</span>
              <button
                class="link-btn"
                onClick={() => { setShowSheet(false); props.onOpenSettings(); }}
              >Network settings</button>
            </div>
          </div>
        </Portal>
      </Show>
    </>
  );
}
