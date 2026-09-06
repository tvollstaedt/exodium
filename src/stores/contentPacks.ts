import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import {
  listContentPacks,
  installContentPack,
  uninstallContentPack,
  getContentPackProgress,
  cancelContentPackInstall,
  getAvailableCollections,
  type ContentPackStatus,
} from "../api/tauri";
import { loadThumbnailDir } from "./thumbnails";
import { showToast } from "./toasts";

// ── Installed pack state (reactive) ──────────────────────────────────────────

const [installedPacks, setInstalledPacks] = createSignal<Set<string>>(new Set());
export { installedPacks };

/** Every collection's packs by id, from the same sweep as the installed
 *  set, so consumers render in the same frame as the grid. */
const [packsByCollection, setPacksByCollection] = createSignal<Record<string, ContentPackStatus[]>>({});
export { packsByCollection };

/** Refresh installed state and the per-collection pack lists. */
export async function refreshInstalledPacks() {
  try {
    const collections = await getAvailableCollections();
    const allInstalled = new Set<string>();
    const byCollection: Record<string, ContentPackStatus[]> = {};
    for (const col of collections) {
      try {
        const packs = await listContentPacks(col.id);
        byCollection[col.id] = packs;
        for (const p of packs) {
          if (p.installed) {
            allInstalled.add(`${col.id}:${p.id}`);
          }
        }
      } catch {
        // Collection may not have content packs - ignore
      }
    }
    setInstalledPacks(allInstalled);
    setPacksByCollection(byCollection);
  } catch {
    // Manifest unavailable - leave current state
  }
}

/** Check if a specific pack is installed (e.g. "eXoDOS:posters"). */
export function isPackInstalled(collection: string, packId: string): boolean {
  return installedPacks().has(`${collection}:${packId}`);
}

// ── In-flight download state (polling) ───────────────────────────────────────

export interface ContentPackJobState {
  phase: string;
  progress: number;
  downloaded_bytes: number;
  total_bytes: number;
  finished: boolean;
  installed: boolean;
  error: string | null;
  label?: string;
}

const [activeJobs, setActiveJobs] = createSignal<Record<string, ContentPackJobState>>({});
export { activeJobs };

const pollIntervals: Record<string, ReturnType<typeof setInterval>> = {};
// Human-readable labels tracked separately (display_name from manifest).
const jobLabels: Record<string, string> = {};

function startPolling(collection: string, packId: string) {
  const key = `${collection}:${packId}`;
  if (pollIntervals[key]) { return; }

  pollIntervals[key] = setInterval(async () => {
    try {
      const progress = await getContentPackProgress(collection, packId);
      if (!progress) {
        // No job on the backend: drop the optimistic entry too, or the row
        // keeps showing a download nobody is running.
        stopPolling(key);
        clearJob(key);
        return;
      }
      setActiveJobs((prev) => ({
        ...prev,
        [key]: {
          phase: progress.phase,
          progress: progress.progress,
          downloaded_bytes: progress.downloaded_bytes,
          total_bytes: progress.total_bytes,
          finished: progress.finished,
          installed: progress.installed,
          error: progress.error,
          label: jobLabels[key],
        },
      }));

      if (progress.finished) {
        stopPolling(key);
        if (progress.installed) {
          // Refresh both installed-pack state AND directory caches so tier
          // resolution picks up the new poster dir without an app restart.
          await refreshInstalledPacks();
          await loadThumbnailDir();
        } else if (progress.error && progress.error !== "Cancelled") {
          // Surface failures globally - installs started from the welcome
          // flow otherwise fail silently unless the Settings dialog is open.
          const name = jobLabels[key] ?? packId;
          showToast(`Couldn't install ${name}`, "error", { detail: progress.error });
        }
        // Clear the job entry after a brief delay so the UI can show "Done!".
        setTimeout(() => {
          setActiveJobs((prev) => {
            const next = { ...prev };
            delete next[key];
            return next;
          });
        }, 5000);
      }
    } catch {
      stopPolling(key);
    }
  }, 1000);
}

function stopPolling(key: string) {
  if (pollIntervals[key]) {
    clearInterval(pollIntervals[key]);
    delete pollIntervals[key];
  }
}

function clearJob(key: string) {
  setActiveJobs((prev) => {
    if (!prev[key]) { return prev; }
    const next = { ...prev };
    delete next[key];
    return next;
  });
}

// ── Public actions ───────────────────────────────────────────────────────────

/** Pick up jobs the backend starts itself (emulator auto-queue); the poll
 *  loop only watches jobs it knows. Registered once at mount. */
export async function initContentPackEvents() {
  await listen<{ collection: string; pack_id: string; display_name: string }>(
    "content-pack-install-started",
    (event) => {
      const { collection, pack_id, display_name } = event.payload;
      const key = `${collection}:${pack_id}`;
      jobLabels[key] = display_name;
      setActiveJobs((prev) =>
        prev[key]
          ? prev
          : {
              ...prev,
              [key]: {
                phase: "starting",
                progress: 0,
                downloaded_bytes: 0,
                total_bytes: 0,
                finished: false,
                installed: false,
                error: null,
                label: display_name,
              },
            },
      );
      startPolling(collection, pack_id);
    },
  );
}

export async function startContentPackInstall(collection: string, packId: string, displayName?: string) {
  const key = `${collection}:${packId}`;
  if (displayName) { jobLabels[key] = displayName; }
  // Claim the row now; the first poll is a second out.
  setActiveJobs((prev) => ({
    ...prev,
    [key]: {
      phase: "starting",
      progress: 0,
      downloaded_bytes: 0,
      total_bytes: 0,
      finished: false,
      installed: false,
      error: null,
      label: jobLabels[key],
    },
  }));
  try {
    await installContentPack(collection, packId);
  } catch (e) {
    // Nothing is running, so the optimistic entry has to go - otherwise the
    // row is stuck on a download that never started, with only Cancel offered.
    clearJob(key);
    throw e;
  }
  startPolling(collection, packId);
}

export async function cancelContentPackJob(collection: string, packId: string) {
  const key = `${collection}:${packId}`;
  // Stop polling and clear UI state up-front so the card returns to "Install"
  // immediately. The backend marks the job failed with error "Cancelled"
  // asynchronously; by then we no longer care about its final state.
  stopPolling(key);
  clearJob(key);
  try {
    await cancelContentPackInstall(collection, packId);
  } catch (e) {
    console.error("Cancel failed:", e);
  }
}

export async function removeContentPack(collection: string, packId: string) {
  await uninstallContentPack(collection, packId);
  await refreshInstalledPacks();
  await loadThumbnailDir();
}

/** Cancel every in-flight pack install and report how many there were.
 *  Used when switching to offline: an HTTP download that keeps running behind
 *  an "Offline" badge makes the badge a lie. */
export async function cancelAllPackJobs(): Promise<number> {
  const running = Object.entries(activeJobs()).filter(([, job]) => job && !job.finished);
  for (const [key] of running) {
    const [collection, packId] = key.split(":");
    await cancelContentPackJob(collection, packId).catch(() => {});
  }
  return running.length;
}
