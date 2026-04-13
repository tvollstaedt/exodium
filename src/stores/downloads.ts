import { createSignal } from "solid-js";
import { cancelDownload, downloadGame, getDownloadProgress } from "../api/tauri";
import { fetchGames, notifyGameLibraryChanged } from "./games";

interface DownloadState {
  status: string;
  progress: number;
  downloading: boolean;
  title?: string;
}

const [downloads, setDownloads] = createSignal<Record<number, DownloadState>>({});

// Track active polling intervals so they can be cancelled.
const intervals: Record<number, ReturnType<typeof setInterval>> = {};
// Track when a game first reached 100% without finishing (stuck detection).
const stuckSince: Record<number, number> = {};
// Highest progress seen per game — prevents bar from jumping backwards due to
// librqbit stats blips or component remounts resetting the CSS transition.
const maxProgress: Record<number, number> = {};
// Titles tracked separately so state updates inside the poll loop don't have
// to re-pass the title every time.
const titles: Record<number, string> = {};

export { downloads };

export function getDownloadState(gameId: number): DownloadState | undefined {
  return downloads()[gameId];
}

export function startGameDownload(gameId: number, title?: string) {
  maxProgress[gameId] = 0;
  if (title) { titles[gameId] = title; }
  setDownloads((prev) => ({
    ...prev,
    [gameId]: { status: "Starting download...", progress: 0, downloading: true, title },
  }));

  const interval = setInterval(async () => {
    try {
      const p = await getDownloadProgress(gameId);
      if (p) {
        // Only allow progress to increase — prevents backwards jumps.
        const safeProgress = Math.max(maxProgress[gameId] ?? 0, p.progress);
        maxProgress[gameId] = safeProgress;

        if (p.error) {
          clearInterval(interval);
          delete intervals[gameId];
          delete stuckSince[gameId];
          delete maxProgress[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: p.error!, progress: 0, downloading: false, title: titles[gameId] },
          }));
        } else if (p.installed) {
          clearInterval(interval);
          delete intervals[gameId];
          delete stuckSince[gameId];
          delete maxProgress[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: "Installed!", progress: 1, downloading: false, title: titles[gameId] },
          }));
          fetchGames();
          notifyGameLibraryChanged(gameId);
          // Delay cleanup so isInstalled() stays true until fetchGames() propagates the
          // updated installed flag from the DB into the games store.
          setTimeout(() => {
            setDownloads((prev) => {
              const next = { ...prev };
              delete next[gameId];
              return next;
            });
          }, 5000);
        } else if (p.finished) {
          delete stuckSince[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: "Extracting...", progress: safeProgress, downloading: true, title: titles[gameId] },
          }));
        } else if (safeProgress >= 0.999) {
          // 100% but ZIP not yet assembled — detect if stuck.
          if (!stuckSince[gameId]) { stuckSince[gameId] = Date.now(); }
          const elapsed = (Date.now() - stuckSince[gameId]) / 1000;
          const status = elapsed > 30
            ? "Waiting for last pieces… try cancelling and re-downloading if this persists"
            : "100%";
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status, progress: safeProgress, downloading: true, title: titles[gameId] },
          }));
        } else {
          delete stuckSince[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: {
              status: `${(safeProgress * 100).toFixed(0)}%`,
              progress: safeProgress,
              downloading: true,
              title: titles[gameId],
            },
          }));
        }
      }
    } catch (e) {
      console.error(`[downloads] poll error for game ${gameId}:`, e);
    }
  }, 1000);

  intervals[gameId] = interval;

  // Fire download command
  downloadGame(gameId).catch((e) => {
    clearInterval(interval);
    delete intervals[gameId];
    delete stuckSince[gameId];
    delete maxProgress[gameId];
    setDownloads((prev) => ({
      ...prev,
      [gameId]: { status: `Error: ${e}`, progress: 0, downloading: false, title: titles[gameId] },
    }));
  });
}

export async function cancelGameDownload(gameId: number) {
  clearInterval(intervals[gameId]);
  delete intervals[gameId];
  delete stuckSince[gameId];
  delete maxProgress[gameId];
  setDownloads((prev) => {
    const next = { ...prev };
    delete next[gameId];
    return next;
  });
  try {
    await cancelDownload(gameId);
    fetchGames();
  } catch {}
}
