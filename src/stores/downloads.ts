import { createSignal } from "solid-js";
import { downloadGame, getDownloadProgress } from "../api/tauri";
import { fetchGames } from "./games";

interface DownloadState {
  status: string;
  progress: number;
  downloading: boolean;
}

const [downloads, setDownloads] = createSignal<Record<number, DownloadState>>({});

export { downloads };

export function getDownloadState(gameId: number): DownloadState | undefined {
  return downloads()[gameId];
}

export function startGameDownload(gameId: number) {
  // Set initial state
  setDownloads((prev) => ({
    ...prev,
    [gameId]: { status: "Starting download...", progress: 0, downloading: true },
  }));

  // Start polling
  const interval = setInterval(async () => {
    try {
      const p = await getDownloadProgress(gameId);
      if (p) {
        if (p.error) {
          clearInterval(interval);
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: p.error!, progress: 0, downloading: false },
          }));
        } else if (p.installed) {
          clearInterval(interval);
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: "Installed!", progress: 1, downloading: false },
          }));
          fetchGames();
          setTimeout(() => {
            setDownloads((prev) => {
              const next = { ...prev };
              delete next[gameId];
              return next;
            });
          }, 3000);
        } else if (p.finished) {
          setDownloads((prev) => ({
            ...prev,
            [gameId]: { status: "Extracting...", progress: p.progress, downloading: true },
          }));
        } else {
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
    } catch {}
  }, 1000);

  // Fire download command
  downloadGame(gameId).catch((e) => {
    clearInterval(interval);
    setDownloads((prev) => ({
      ...prev,
      [gameId]: { status: `Error: ${e}`, progress: 0, downloading: false },
    }));
  });
}
