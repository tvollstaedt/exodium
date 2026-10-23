import { createEffect, createRoot, createSignal } from "solid-js";
import { downloads, markQueued, startGameDownload, unmarkQueued, type DownloadCover } from "./downloads";

/** "Download all" runs at most this many at once: every download polls once
 *  a second and every start re-selects the torrent's files, so a list started
 *  in one go would do both a few hundred times over. */
export const MAX_ACTIVE = 4;

export interface BatchItem {
  gameId: number;
  title: string;
  cover?: DownloadCover;
}

interface Job extends BatchItem {
  playlistId: number;
}

const waiting: Job[] = [];
const active = new Set<number>();
const [waitingByPlaylist, setWaitingByPlaylist] = createSignal<Record<number, number>>({});

function publish() {
  const counts: Record<number, number> = {};
  for (const job of waiting) { counts[job.playlistId] = (counts[job.playlistId] ?? 0) + 1; }
  setWaitingByPlaylist(counts);
}

function pump() {
  while (active.size < MAX_ACTIVE && waiting.length > 0) {
    const job = waiting.shift()!;
    active.add(job.gameId);
    startGameDownload(job.gameId, job.title, job.cover);
  }
  publish();
}

/** Queue a playlist's downloads behind the ones already running. A game that
 *  is downloading or queued already keeps its place. */
export function enqueueDownloads(playlistId: number, items: BatchItem[]) {
  const dl = downloads();
  for (const item of items) {
    if (active.has(item.gameId) || waiting.some((j) => j.gameId === item.gameId)) { continue; }
    if (dl[item.gameId]?.downloading) { continue; }
    waiting.push({ ...item, playlistId });
    markQueued(item.gameId, item.title, item.cover);
  }
  pump();
}

/** Drop what has not started yet; running downloads finish. */
export function cancelQueued(playlistId: number) {
  for (const job of waiting) {
    if (job.playlistId === playlistId) { unmarkQueued(job.gameId); }
  }
}

export const queuedCount = (playlistId: number) => waitingByPlaylist()[playlistId] ?? 0;

// The downloads store is the queue's clock: a slot frees once its entry stops
// downloading (installed, failed or cancelled), and a waiting job leaves when
// its Queued entry is gone - cancelled from a card or the activity sheet, or
// cleared by going offline.
createRoot(() => {
  createEffect(() => {
    const dl = downloads();
    let changed = false;
    for (const id of [...active]) {
      if (!dl[id]?.downloading) {
        active.delete(id);
        changed = true;
      }
    }
    for (let i = waiting.length - 1; i >= 0; i--) {
      if (!dl[waiting[i].gameId]?.queued) {
        waiting.splice(i, 1);
        changed = true;
      }
    }
    if (changed) { pump(); }
  });
});
