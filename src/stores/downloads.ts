import { createSignal } from "solid-js";
import { cancelDownload, downloadGame, getDownloadProgress } from "../api/tauri";
import { fetchGames } from "./games";

interface DownloadState {
  status: string;
  progress: number;
  downloading: boolean;
}

const [downloads, setDownloads] = createSignal<Record<number, DownloadState>>({});

// Track active polling intervals so they can be cancelled.
const intervals: Record<number, ReturnType<typeof setInterval>> = {};
// Track when a game first reached 100% without finishing (stuck detection).
const stuckSince: Record<number, number> = {};

export { downloads };

export function getDownloadState(gameId: number): DownloadState | undefined {
  return downloads()[gameId];
}

export function startGameDownload(gameId: number) {
  setDownloads((prev) => ({
    ...prev,
    [gameId]: { status: "Starting download...", progress: 0, downloading: true },
  }));

  const interval = setInterval(async () => {
    try {
      const p = await getDownloadProgress(gameId);
      if (p) {
        if (p.error) {
          clearInterval(interval);
          delete intervals[gameId];
          delete stuckSince[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: p.error!, progress: 0, downloading: false },
          }));
        } else if (p.installed) {
          clearInterval(interval);
          delete intervals[gameId];
          delete stuckSince[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: "Installed!", progress: 1, downloading: false },
          }));
          fetchGames();
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
            [gameId]: { status: "Extracting...", progress: p.progress, downloading: true },
          }));
        } else if (p.progress >= 0.999) {
          // 100% but ZIP not yet assembled — detect if stuck.
          if (!stuckSince[gameId]) { stuckSince[gameId] = Date.now(); }
          const elapsed = (Date.now() - stuckSince[gameId]) / 1000;
          const status = elapsed > 30
            ? "Waiting for last pieces… try cancelling and re-downloading if this persists"
            : "100%";
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status, progress: p.progress, downloading: true },
          }));
        } else {
          delete stuckSince[gameId];
          setDownloads((prev) => ({
            ...prev,
            [gameId]: {
              status: `${(p.progress * 100).toFixed(0)}%`,
              progress: p.progress,
              downloading: true,
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
    setDownloads((prev) => ({
      ...prev,
      [gameId]: { status: `Error: ${e}`, progress: 0, downloading: false },
    }));
  });
}

export async function cancelGameDownload(gameId: number) {
  clearInterval(intervals[gameId]);
  delete intervals[gameId];
  delete stuckSince[gameId];
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
