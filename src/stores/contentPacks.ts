import { createSignal } from "solid-js";
import {
  listContentPacks,
  installContentPack,
  uninstallContentPack,
  getContentPackProgress,
  cancelContentPackInstall,
  getAvailableCollections,
} from "../api/tauri";
import { loadThumbnailDir } from "./thumbnails";

// ── Installed pack state (reactive) ──────────────────────────────────────────

const [installedPacks, setInstalledPacks] = createSignal<Set<string>>(new Set());
export { installedPacks };

/** Refresh the set of installed content packs from the backend. */
export async function refreshInstalledPacks() {
  try {
    const collections = await getAvailableCollections();
    const allInstalled = new Set<string>();
    for (const col of collections) {
      try {
        const packs = await listContentPacks(col.id);
        for (const p of packs) {
          if (p.installed) {
            allInstalled.add(`${col.id}:${p.id}`);
          }
        }
      } catch {
        // Collection may not have content packs — ignore
      }
    }
    setInstalledPacks(allInstalled);
  } catch {
    // Manifest unavailable — leave current state
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
}

const [activeJobs, setActiveJobs] = createSignal<Record<string, ContentPackJobState>>({});
export { activeJobs };

const pollIntervals: Record<string, ReturnType<typeof setInterval>> = {};

function startPolling(collection: string, packId: string) {
  const key = `${collection}:${packId}`;
  if (pollIntervals[key]) { return; }

  pollIntervals[key] = setInterval(async () => {
    try {
      const progress = await getContentPackProgress(collection, packId);
      if (!progress) {
        stopPolling(key);
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
        },
      }));

      if (progress.finished) {
        stopPolling(key);
        if (progress.installed) {
          // Refresh both installed-pack state AND directory caches so tier
          // resolution picks up the new poster dir without an app restart.
          await refreshInstalledPacks();
          await loadThumbnailDir();
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

// ── Public actions ───────────────────────────────────────────────────────────

export async function startContentPackInstall(collection: string, packId: string) {
  await installContentPack(collection, packId);
  startPolling(collection, packId);
}

export async function cancelContentPackJob(collection: string, packId: string) {
  await cancelContentPackInstall(collection, packId);
  stopPolling(`${collection}:${packId}`);
}

export async function removeContentPack(collection: string, packId: string) {
  await uninstallContentPack(collection, packId);
  await refreshInstalledPacks();
  await loadThumbnailDir();
}
