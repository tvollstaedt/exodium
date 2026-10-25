import { batch, createEffect, createRoot, createSignal, on } from "solid-js";
import {
  downloads, markQueued, startGameDownload, unmarkQueued, cancelGameDownload, isPausable,
  pauseGameDownload, type DownloadCover,
} from "./downloads";
import { kidsLocked } from "./kids";

/** "Download playlist" runs at most this many at once: every download polls once
 *  a second and every start re-selects the torrent's files, so a list started
 *  in one go would do both a few hundred times over. */
export const MAX_ACTIVE = 4;

/** Owner of a resumed download that belongs to no playlist. */
const NO_PLAYLIST = -1;

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
/** The global pause: nothing new starts until it is lifted. */
const [halted, setHalted] = createSignal(false);
export { halted as queueHalted };

function publish() {
  const counts: Record<number, number> = {};
  for (const job of waiting) { counts[job.playlistId] = (counts[job.playlistId] ?? 0) + 1; }
  setWaitingByPlaylist(counts);
}

/** Nothing starts while kids mode is locked: `download_game` refuses there
 *  (§26), and a refused start would drop the job with an error toast. */
function pump() {
  while (!halted() && !kidsLocked() && active.size < MAX_ACTIVE && waiting.length > 0) {
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
  const known = new Set([...active, ...waiting.map((j) => j.gameId)]);
  batch(() => {
    for (const item of items) {
      if (known.has(item.gameId) || dl[item.gameId]?.downloading) { continue; }
      known.add(item.gameId);
      waiting.push({ ...item, playlistId });
      markQueued(item.gameId, item.title, item.cover);
    }
  });
  pump();
}

/** Drop what has not started yet; running downloads finish. */
export function cancelQueued(playlistId: number) {
  const dropped: Job[] = [];
  for (let i = waiting.length - 1; i >= 0; i--) {
    if (waiting[i].playlistId === playlistId) { dropped.push(...waiting.splice(i, 1)); }
  }
  publish();
  for (const job of dropped) { unmarkQueued(job.gameId); }
}

export const queuedCount = (playlistId: number) => waitingByPlaylist()[playlistId] ?? 0;

/** Paused games go to the FRONT of the queue, so resuming never runs more
 *  than MAX_ACTIVE queue-started downloads at once. */
export function resumeGameDownloads(gameIds: number[]) {
  const dl = downloads();
  const jobs = gameIds
    .filter((id) => dl[id]?.paused)
    .map((id) => ({ gameId: id, title: dl[id]!.title ?? "", cover: dl[id]!.cover, playlistId: NO_PLAYLIST }));
  // Marked before they join `waiting`: each mark runs the effect below,
  // which drops any waiting job whose entry is not Queued yet.
  for (const job of jobs) { markQueued(job.gameId, job.title, job.cover, true); }
  waiting.unshift(...jobs);
  pump();
}

/** One row's Play: runs at once even under the global pause - the user asked
 *  for exactly this game. */
export function resumeGameDownload(gameId: number) {
  const s = downloads()[gameId];
  if (!s?.paused) { return; }
  if (halted()) {
    startGameDownload(gameId, s.title, s.cover);
    return;
  }
  resumeGameDownloads([gameId]);
}

export const waitingCount = () => Object.values(waitingByPlaylist()).reduce((a, b) => a + b, 0);

export async function pauseAllDownloads() {
  setHalted(true);
  const ids = Object.entries(downloads()).filter(([, s]) => isPausable(s)).map(([id]) => Number(id));
  await Promise.all(ids.map((id) => pauseGameDownload(id)));
}

export function resumeAllDownloads() {
  setHalted(false);
  resumeGameDownloads(Object.entries(downloads()).filter(([, s]) => s.paused).map(([id]) => Number(id)));
}

/** Every game download - running, paused, queued. Content packs keep theirs.
 *  The queue goes first: each cancelled slot would start the next game. */
export async function cancelAllDownloads() {
  const dropped = waiting.splice(0);
  publish();
  for (const job of dropped) { unmarkQueued(job.gameId); }
  setHalted(false);
  const ids = Object.entries(downloads()).filter(([, s]) => s.downloading).map(([id]) => Number(id));
  await Promise.all(ids.map((id) => cancelGameDownload(id)));
}

// The downloads store is the queue's clock: a slot frees once its entry stops
// downloading (installed, failed, cancelled) or is paused, and a waiting job
// leaves when its Queued entry is gone - cancelled from a card or the
// activity sheet, or cleared by going offline.
createRoot(() => {
  createEffect(() => {
    const dl = downloads();
    let changed = false;
    for (const id of [...active]) {
      if (!dl[id]?.downloading || dl[id]?.paused) {
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
    // A pause with nothing left to resume would silently hold the next
    // "Download playlist" back.
    if (halted() && waiting.length === 0 && !Object.values(dl).some((s) => s.paused)) {
      setHalted(false);
    }
    if (changed) { pump(); }
  });
  createEffect(on(kidsLocked, (locked) => { if (!locked) { pump(); } }, { defer: true }));
});
