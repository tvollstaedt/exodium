import { createSignal, createEffect, Show, For } from "solid-js";
import { AutoProgress } from "./ProgressBar";
import {
  listContentPacks, getConfig, getAvailableCollections,
  type ContentPackStatus,
} from "../api/tauri";
import {
  activeJobs,
  startContentPackInstall,
  removeContentPack,
  cancelContentPackJob,
  installedPacks,
} from "../stores/contentPacks";
import { formatBytes } from "../util";

type CollectionPacks = {
  id: string;
  label: string;
  packs: ContentPackStatus[];
};

export function ContentPackSettings() {
  const [collections, setCollections] = createSignal<CollectionPacks[]>([]);

  const loadPacks = async () => {
    try {
      const [colStr, available] = await Promise.all([
        getConfig("collections"),
        getAvailableCollections(),
      ]);
      const ids = (colStr ?? "").split(",").filter(Boolean);
      const labelMap: Record<string, string> = {};
      for (const c of available) { labelMap[c.id] = c.display_name; }

      // Fetch each collection's packs in parallel, sort eXoDOS first.
      const sortedIds = [...ids].sort((a, b) => a === "eXoDOS" ? -1 : b === "eXoDOS" ? 1 : a.localeCompare(b));
      const entries = await Promise.all(
        sortedIds.map(async (id) => ({
          id,
          label: labelMap[id] || id,
          packs: await listContentPacks(id).catch(() => [] as ContentPackStatus[]),
        }))
      );
      setCollections(entries.filter((e) => e.packs.length > 0));
    } catch {
      setCollections([]);
    }
  };

  // Refresh when installed-packs state changes (e.g. after a pack finishes).
  createEffect(() => {
    installedPacks(); // subscribe
    loadPacks();
  });

  const handleInstall = async (collectionId: string, packId: string, displayName: string) => {
    try {
      await startContentPackInstall(collectionId, packId, displayName);
    } catch (e) {
      console.error("Install failed:", e);
    }
  };

  const handleUninstall = async (collectionId: string, packId: string) => {
    try {
      await removeContentPack(collectionId, packId);
    } catch (e) {
      console.error("Uninstall failed:", e);
    }
  };

  const handleCancel = async (collectionId: string, packId: string) => {
    try {
      await cancelContentPackJob(collectionId, packId);
    } catch (e) {
      console.error("Cancel failed:", e);
    }
  };

  return (
    <>
      <h3 class="settings-section-title">Content Packs</h3>
      <p class="settings-section-hint">Optional downloads that enhance your library with box art and media. Each language pack has its own metadata set.</p>

      <For each={collections()} fallback={<span class="setting-hint">No content packs available.</span>}>
        {(col) => (
          <div class="pack-collection-group">
            <Show when={collections().length > 1}>
              <h4 class="pack-collection-title">{col.label}</h4>
            </Show>

            <For each={col.packs}>
              {(pack) => {
                const key = () => `${col.id}:${pack.id}`;
                const job = () => activeJobs()[key()];
                const isActive = () => !!job() && !job()!.finished;
                const isFuture = () => !pack.available;
                const progress = () => job()?.progress ?? 0;

                const isSupersededByInstalled = () =>
                  col.packs.some((other) =>
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
                  <div class="pack-row">
                    <div class="pack-info">
                      <span class="pack-name">{pack.display_name}</span>
                      <span class="pack-desc">{pack.description} · {formatBytes(pack.size_bytes)}</span>
                    </div>

                    <Show when={isActive()}>
                      <span class="pack-status-inline">{statusText()}</span>
                      <button class="btn-small btn-danger" onClick={() => handleCancel(col.id, pack.id)}>Cancel</button>
                      <div class="pack-progress">
                        <AutoProgress value={progress()} class="mini" indeterminate={job()?.phase !== "downloading" || undefined} />
                      </div>
                    </Show>

                    <Show when={!isActive() && job()?.error}>
                      <span class="error" style="width: 100%; margin: 0; padding: 6px 10px; font-size: 11px">{job()!.error}</span>
                    </Show>

                    <Show when={!isActive() && !job()}>
                      <Show when={pack.installed}>
                        <span class="pack-status-inline">Installed · v{pack.installed_version}</span>
                        <button class="btn-small btn-danger" onClick={() => handleUninstall(col.id, pack.id)}>Remove</button>
                      </Show>

                      <Show when={!pack.installed && isSupersededByInstalled()}>
                        <span class="pack-status-inline">Included in another pack</span>
                      </Show>

                      <Show when={!pack.installed && !isSupersededByInstalled() && !isFuture()}>
                        <button class="btn-small" onClick={() => handleInstall(col.id, pack.id, pack.display_name)}>Install</button>
                      </Show>

                      <Show when={isFuture()}>
                        <span class="pack-status-inline">Coming soon</span>
                      </Show>
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
        )}
      </For>
    </>
  );
}
