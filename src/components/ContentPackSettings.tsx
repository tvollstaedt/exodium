import { createSignal, createEffect, Show, For } from "solid-js";
import { Progress } from "@ark-ui/solid/progress";
import { listContentPacks, type ContentPackStatus } from "../api/tauri";
import {
  activeJobs,
  startContentPackInstall,
  removeContentPack,
  cancelContentPackJob,
  installedPacks,
} from "../stores/contentPacks";
import { formatBytes } from "../util";

export function ContentPackSettings() {
  const [packs, setPacks] = createSignal<ContentPackStatus[]>([]);

  const loadPacks = () => {
    listContentPacks("eXoDOS")
      .then(setPacks)
      .catch(() => setPacks([]));
  };

  // Refresh when installed-packs state changes (e.g. after a pack finishes).
  createEffect(() => {
    installedPacks(); // subscribe
    loadPacks();
  });

  const handleInstall = async (packId: string) => {
    const pack = packs().find((p) => p.id === packId);
    try {
      await startContentPackInstall("eXoDOS", packId, pack?.display_name);
    } catch (e) {
      console.error("Install failed:", e);
    }
  };

  const handleUninstall = async (packId: string) => {
    try {
      await removeContentPack("eXoDOS", packId);
    } catch (e) {
      console.error("Uninstall failed:", e);
    }
  };

  const handleCancel = async (packId: string) => {
    try {
      await cancelContentPackJob("eXoDOS", packId);
    } catch (e) {
      console.error("Cancel failed:", e);
    }
  };

  return (
    <>
      <div class="setting-row" style="flex-direction: column; align-items: flex-start; gap: 4px">
        <span class="setting-label">Content Packs</span>
        <span class="setting-hint">Optional downloads that enhance your library with box art and media.</span>
      </div>

      <For each={packs()} fallback={<span class="setting-hint">No content packs available.</span>}>
        {(pack) => {
          const key = () => `eXoDOS:${pack.id}`;
          const job = () => activeJobs()[key()];
          const isActive = () => !!job() && !job()!.finished;
          const isFuture = () => !pack.available;
          const progress = () => job()?.progress ?? 0;

          const isSupersededByInstalled = () =>
            packs().some((other) =>
              other.supersedes.includes(pack.id) && other.installed
            );

          const statusText = () => {
            const j = job();
            if (!j) { return ""; }
            if (j.error) { return j.error; }
            const pct = Math.round((j.progress ?? 0) * 100);
            switch (j.phase) {
              case "downloading": return `Downloading… ${pct}%`;
              case "verifying": return "Verifying checksum…";
              case "extracting": return "Extracting…";
              case "installing": return "Installing…";
              case "installed": return "Installed!";
              case "failed": return `Failed: ${j.error ?? "unknown error"}`;
              default: return j.phase;
            }
          };

          return (
            <div class="setting-row" style="flex-wrap: wrap; gap: 8px">
              <span class="setting-label">{pack.display_name}</span>
              <span class="setting-hint">{pack.description} (~{formatBytes(pack.size_bytes)})</span>

              {/* Active download: progress bar + status + cancel */}
              <Show when={isActive()}>
                <div style="width: 100%; display: flex; align-items: center; gap: 8px">
                  <div style="flex: 1">
                    <Progress.Root value={progress() * 100} class="ark-progress mini">
                      <Progress.Track class="ark-progress-track">
                        <Progress.Range class="ark-progress-range" />
                      </Progress.Track>
                    </Progress.Root>
                  </div>
                  <span class="setting-hint" style="white-space: nowrap; flex: none">{statusText()}</span>
                  <button class="btn-small btn-danger" onClick={() => handleCancel(pack.id)}>✕</button>
                </div>
              </Show>

              {/* Error from a finished job */}
              <Show when={!isActive() && job()?.error}>
                <span class="error" style="width: 100%; margin: 0; padding: 6px 10px; font-size: 11px">{job()!.error}</span>
              </Show>

              {/* Idle states */}
              <Show when={!isActive() && !job()}>
                <Show when={pack.installed}>
                  <span class="setting-hint" style="flex: none">Installed (v{pack.installed_version})</span>
                  <button class="btn-small btn-danger" onClick={() => handleUninstall(pack.id)}>Remove</button>
                </Show>

                <Show when={!pack.installed && isSupersededByInstalled()}>
                  <span class="setting-hint" style="flex: none">Included in another pack</span>
                </Show>

                <Show when={!pack.installed && !isSupersededByInstalled() && !isFuture()}>
                  <button class="btn-small" onClick={() => handleInstall(pack.id)}>Install</button>
                </Show>

                <Show when={isFuture()}>
                  <span class="setting-hint" style="flex: none">Coming soon</span>
                </Show>
              </Show>
            </div>
          );
        }}
      </For>
    </>
  );
}
