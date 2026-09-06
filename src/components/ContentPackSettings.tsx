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
import { showToast } from "../stores/toasts";
import { Button } from "./Button";
import { isOffline } from "../stores/network";

type CollectionPacks = {
  id: string;
  label: string;
  packs: ContentPackStatus[];
};

type PendingAction = "install" | "remove" | "cancel";

export function ContentPackSettings() {
  const [collections, setCollections] = createSignal<CollectionPacks[]>([]);
  // Per-pack in-flight action so the matching button shows a spinner and is
  // disabled until the backend resolves. Keyed by `${col}:${pack}`.
  const [pending, setPending] = createSignal<Record<string, PendingAction>>({});

  const setPendingFor = (key: string, action: PendingAction | null) => {
    setPending((prev) => {
      const next = { ...prev };
      if (action) { next[key] = action; } else { delete next[key]; }
      return next;
    });
  };

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
    const key = `${collectionId}:${packId}`;
    setPendingFor(key, "install");
    try {
      await startContentPackInstall(collectionId, packId, displayName);
    } catch (e) {
      console.error("Install failed:", e);
      showToast(`Couldn't install ${displayName}`, "error", { detail: String(e) });
    } finally {
      setPendingFor(key, null);
    }
  };

  const handleUninstall = async (collectionId: string, packId: string, displayName: string) => {
    const key = `${collectionId}:${packId}`;
    setPendingFor(key, "remove");
    try {
      await removeContentPack(collectionId, packId);
      showToast(`Removed ${displayName}`, "success");
    } catch (e) {
      console.error("Uninstall failed:", e);
      showToast(`Couldn't remove ${displayName}`, "error", { detail: String(e) });
    } finally {
      setPendingFor(key, null);
    }
  };

  const handleCancel = async (collectionId: string, packId: string, displayName: string) => {
    // No pending="cancel": cancelContentPackJob clears the row's job
    // synchronously, and a pending mark would flash over the Install branch.
    try {
      await cancelContentPackJob(collectionId, packId);
    } catch (e) {
      console.error("Cancel failed:", e);
      showToast(`Couldn't cancel ${displayName}`, "error", { detail: String(e) });
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
                const pendingAction = () => pending()[key()];

                /** Installed, but the manifest has moved on. The old pack keeps
                 *  working - content packs are replaced whole, so an update is
                 *  the user's call, not something to force on them. */
                const hasUpdate = () =>
                  pack.installed
                  && pack.installed_version !== undefined
                  && pack.installed_version < pack.version;

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
                    // Written optimistically on click, before the backend has
                    // even been asked - see startContentPackInstall.
                    case "starting": return "Starting…";
                    case "downloading": return `Downloading… ${pct}%`;
                    case "verifying": return "Verifying checksum…";
                    case "extracting": return "Extracting…";
                    case "installing": return "Installing…";
                    case "installed": return "Installed!";
                    case "failed": return `Failed: ${j.error ?? "unknown error"}`;
                    default: return j.phase;
                  }
                };

                const blockedOffline = () => isOffline();

                return (
                  <div class="pack-row">
                    <div class="pack-info">
                      <span class="pack-name">{pack.display_name}</span>
                      <span class="pack-desc">{pack.description} · {formatBytes(pack.size_bytes)}</span>
                    </div>

                    <Show when={isActive()}>
                      <span class="pack-status-inline">{statusText()}</span>
                      <Button variant="danger" class="btn-small" onClick={() => handleCancel(col.id, pack.id, pack.display_name)}>
                        Cancel
                      </Button>
                      <div class="pack-progress">
                        <AutoProgress value={progress()} class="mini" indeterminate={job()?.phase !== "downloading" || undefined} />
                      </div>
                    </Show>

                    <Show when={!isActive() && !job()}>
                      <Show when={pack.installed}>
                        <span class="pack-status-inline">
                          {pendingAction() === "remove"
                            ? "Removing…"
                            : hasUpdate()
                              ? `v${pack.installed_version} · v${pack.version} available`
                              : `Installed · v${pack.installed_version}`}
                        </span>
                        <Show when={hasUpdate()}>
                          <Button
                            loading={pendingAction() === "install"}
                            loadingLabel="Starting…"
                            disabled={blockedOffline()}
                            title={blockedOffline()
                              ? "Offline mode - nothing is downloaded. Enable downloads in Settings → Network."
                              : `Re-downloads the whole pack (${formatBytes(pack.size_bytes)})`}
                            onClick={() => handleInstall(col.id, pack.id, pack.display_name)}
                          >
                            Update
                          </Button>
                        </Show>
                        <Button
                          variant="danger"
                          class="btn-small"
                          loading={pendingAction() === "remove"}
                          onClick={() => handleUninstall(col.id, pack.id, pack.display_name)}
                        >
                          Remove
                        </Button>
                      </Show>

                      <Show when={!pack.installed && isSupersededByInstalled()}>
                        <span class="pack-status-inline">Included in another pack</span>
                      </Show>

                      <Show when={!pack.installed && !isSupersededByInstalled() && !isFuture()}>
                        <Button
                          loading={pendingAction() === "install"}
                          loadingLabel="Starting…"
                          disabled={blockedOffline()}
                          title={blockedOffline()
                            ? "Offline mode - nothing is downloaded. Enable downloads in Settings → Network."
                            : undefined}
                          onClick={() => handleInstall(col.id, pack.id, pack.display_name)}
                        >
                          Install
                        </Button>
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
