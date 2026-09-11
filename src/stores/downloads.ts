import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { cancelDownload, downloadGame, getDownloadProgress, listActiveDownloads } from "../api/tauri";
import { refreshLoadedGames, notifyGameLibraryChanged } from "./games";
import { showToast } from "./toasts";
import { transferStats } from "./transfer";

interface DownloadState {
  status: string;
  progress: number;
  downloading: boolean;
  /** True from the moment the game itself is playable (extras may still be
   *  downloading) - components must use this, not string-match the status. */
  installed?: boolean;
  title?: string;
  /** What the download sheet needs for a cover; absent for extras-only trackers. */
  cover?: DownloadCover;
}

export interface DownloadCover { source: string; key: string | null }

const [downloads, setDownloads] = createSignal<Record<number, DownloadState>>({});

const POLL_MS = 1000;
// Null polls before "didn't start": a failed add_torrent leaves the handle
// None forever and file_progress null.
const NULL_POLL_THRESHOLD = 5; // ~5 seconds at 1s polling interval
// Seconds without progress before the status turns into peer-wait feedback,
// and before it becomes an actionable stall warning.
const STALL_HINT_SECS = 15;
const STALL_WARN_SECS = 90;

/** One in-flight download. One object, so ending a run is one delete and
 *  cancellation one flag the poll loop re-reads after every await. */
interface Tracker {
  gameId: number;
  /** Kept on the tracker so status writes inside the loop don't have to
   *  re-pass the title on every tick. */
  title?: string;
  cover?: DownloadCover;
  /** Set by whoever ends the run; checked after every await so an in-flight
   *  poll cannot resurrect the card. */
  cancelled: boolean;
  /** download_game still in flight: null progress is expected until then. */
  commandPending: boolean;
  nullPolls: number;
  /** When the game first reached 100% without finishing; 0 until then. */
  stuckSince: number;
  /** Highest progress seen - prevents the bar from jumping backwards due to
   *  librqbit stats blips or component remounts resetting the transition. */
  maxProgress: number;
  /** Set once the game itself is installed while extras are still
   *  downloading - the library refresh must fire at that moment (game is
   *  playable), not only when the extras finish minutes later. */
  announcedInstalled: boolean;
  /** Last progress increase (value, time) for the file and for the torrent:
   *  a file sits at 0 while its 8 MB piece fills, so the torrent pair is
   *  what tells a stall from a wait. */
  lastProgressVal: number;
  lastProgressAt: number;
  lastTorrentVal: number;
  lastTorrentAt: number;
}

const trackers = new Map<number, Tracker>();

export { downloads };

export function getDownloadState(gameId: number): DownloadState | undefined {
  return downloads()[gameId];
}

/** Ends a run and drops it from the registry. Idempotent, and safe to call on
 *  a tracker a newer attempt has already replaced - the identity check stops
 *  an outgoing run from unregistering its successor. */
function endTracker(t: Tracker) {
  t.cancelled = true;
  if (trackers.get(t.gameId) === t) {
    trackers.delete(t.gameId);
  }
}

function setState(t: Tracker, state: Omit<DownloadState, "title" | "cover">) {
  setDownloads((prev) => ({ ...prev, [t.gameId]: { ...state, title: t.title, cover: t.cover } }));
}

function clearState(gameId: number) {
  setDownloads((prev) => {
    if (!prev[gameId]) { return prev; }
    const next = { ...prev };
    delete next[gameId];
    return next;
  });
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** One poll. Returning ends the run only if it called endTracker. */
async function tick(t: Tracker) {
  const p = await getDownloadProgress(t.gameId);
  // The loop's guard ran BEFORE this await, so re-check: a cancel that landed
  // while the poll was in flight has already deleted the store entry, and
  // every branch below would write it back.
  if (t.cancelled) { return; }

  if (!p) {
    // Null: no handle yet. Expected while the command runs, a fault after.
    if (t.commandPending) {
      t.nullPolls = 0;
      // The first download of a collection can take minutes here (hash
      // check); say so.
      if ((Date.now() - t.lastProgressAt) / 1000 > 8) {
        setState(t, {
          status: "Preparing the collection (one-time setup, can take a few minutes)…",
          progress: 0,
          downloading: true,
        });
      }
      return;
    }
    t.nullPolls += 1;
    if (t.nullPolls >= NULL_POLL_THRESHOLD) {
      endTracker(t);
      setState(t, {
        status: "Download didn't start - open Settings → Diagnostics to view exodium.log.",
        progress: 0,
        downloading: false,
      });
    }
    return;
  }

  t.nullPolls = 0;
  // Only allow progress to increase - prevents backwards jumps.
  const safeProgress = Math.max(t.maxProgress, p.progress);
  t.maxProgress = safeProgress;

  if (p.error) {
    endTracker(t);
    setState(t, { status: p.error, progress: 0, downloading: false });
    showToast(
      t.title ? `Download failed: ${t.title}` : "Download failed",
      "error",
      { detail: p.error },
    );
    return;
  }

  if (p.installed) {
    // The game is playable now, but its extras (GameData: manuals, videos,
    // music) may still be downloading - keep polling and show that second
    // phase instead of letting it finish invisibly.
    if (p.extras_done === false) {
      const pct = ((p.extras_progress ?? 0) * 100).toFixed(0);
      if (!t.announcedInstalled) {
        t.announcedInstalled = true;
        refreshLoadedGames();
        notifyGameLibraryChanged(t.gameId);
      }
      setState(t, {
        status: `Installed - downloading extras… ${pct}%`,
        progress: 1,
        downloading: false,
        installed: true,
      });
      return;
    }
    endTracker(t);
    setState(t, { status: "Installed!", progress: 1, downloading: false, installed: true });
    refreshLoadedGames();
    // Fires metadata-cache invalidation: when extras finished AFTER the game,
    // this is what makes the manual button resolve on its own.
    notifyGameLibraryChanged(t.gameId);
    // Keep isInstalled() true until fetchGames() carries the flag; a newer
    // download owns the entry.
    setTimeout(() => {
      if (trackers.has(t.gameId)) { return; }
      clearState(t.gameId);
    }, 5000);
    return;
  }

  if (p.finished) {
    t.stuckSince = 0;
    // The archive is complete but the install needs another game's files
    // first (an overlay translation onto its English base).
    setState(t, {
      status: p.waiting_for ? `Waiting for ${p.waiting_for}…` : "Extracting...",
      progress: safeProgress,
      downloading: true,
    });
    return;
  }

  if (safeProgress >= 0.999) {
    // 100% but ZIP not yet assembled - detect if stuck.
    if (!t.stuckSince) { t.stuckSince = Date.now(); }
    const elapsed = (Date.now() - t.stuckSince) / 1000;
    setState(t, {
      status: elapsed > 30
        ? "Waiting for last pieces… try cancelling and re-downloading if this persists"
        : "100%",
      progress: safeProgress,
      downloading: true,
    });
    return;
  }

  t.stuckSince = 0;

  if (p.torrent_state === "initializing") {
    // Hash check (minutes on Windows): file progress stays 0, show the
    // torrent-level validation instead.
    const tp = typeof p.torrent_progress === "number" ? p.torrent_progress : 0;
    setState(t, {
      status: `Validating torrent ${(tp * 100).toFixed(0)}% (first run can take several minutes)`,
      progress: tp,
      downloading: true,
    });
    return;
  }

  // Stall feedback: a torrent with no peers (or a dropped connection)
  // otherwise sits at "0%" forever with no signal that anything is wrong.
  // Track the last progress increase and escalate the status.
  const now = Date.now();
  if (safeProgress > t.lastProgressVal) {
    t.lastProgressVal = safeProgress;
    t.lastProgressAt = now;
  }
  const tp = typeof p.torrent_progress === "number" ? p.torrent_progress : 0;
  if (tp > t.lastTorrentVal) {
    t.lastTorrentVal = tp;
    t.lastTorrentAt = now;
  }
  const stalledSecs = (now - t.lastProgressAt) / 1000;
  // Data is arriving for the torrent, so a wait, not a fault. Two signals:
  // piece progress goes quiet on a slow line, the byte rate does not.
  const pieceAdvanced = (now - t.lastTorrentAt) / 1000 < STALL_HINT_SECS;
  const bytesFlowing = (transferStats()?.download_bps ?? 0) >= 1024;
  const receiving = pieceAdvanced || bytesFlowing;
  const pct = `${(safeProgress * 100).toFixed(0)}%`;
  let status = pct;
  if (stalledSecs >= STALL_HINT_SECS && receiving) {
    status = `${pct} - fetching a shared data block…`;
  } else if (stalledSecs >= STALL_WARN_SECS) {
    status = `Stalled at ${pct} - no data received. Check your connection, or cancel and retry.`;
  } else if (stalledSecs >= STALL_HINT_SECS) {
    status = safeProgress === 0 ? "Looking for peers…" : `${pct} - waiting for peers…`;
  }
  setState(t, { status, progress: safeProgress, downloading: true });
}

/** Self-scheduling poll loop. It owns its own lifetime, so there is no timer
 *  handle to orphan and no generation counter to re-read - the run stops when
 *  its own tracker is cancelled. */
async function poll(t: Tracker) {
  while (!t.cancelled) {
    await sleep(POLL_MS);
    if (t.cancelled) { return; }
    try {
      await tick(t);
    } catch (e) {
      console.error(`[downloads] poll error for game ${t.gameId}:`, e);
    }
  }
}

/** A registered tracker at t=0; `commandPending` says whether a
 *  download_game call is still on its way (null polls are expected then). */
function newTracker(gameId: number, title: string | undefined, commandPending: boolean, cover?: DownloadCover): Tracker {
  const now = Date.now();
  const t: Tracker = {
    gameId,
    title,
    cover,
    cancelled: false,
    commandPending,
    nullPolls: 0,
    stuckSince: 0,
    maxProgress: 0,
    announcedInstalled: false,
    lastProgressVal: -1,
    lastProgressAt: now,
    lastTorrentVal: -1,
    lastTorrentAt: now,
  };
  trackers.set(gameId, t);
  return t;
}

export function startGameDownload(gameId: number, title?: string, cover?: DownloadCover) {
  // A still-running attempt for the same game must not write the store on
  // behalf of this one.
  const previous = trackers.get(gameId);
  if (previous) { endTracker(previous); }

  const t = newTracker(
    gameId, title ?? previous?.title ?? downloads()[gameId]?.title, true,
    cover ?? previous?.cover ?? downloads()[gameId]?.cover,
  );
  setState(t, { status: "Starting download...", progress: 0, downloading: true });

  void poll(t);

  downloadGame(gameId).then(() => {
    if (t.cancelled) { return; }
    t.commandPending = false;
  }).catch((e) => {
    if (t.cancelled) { return; }
    endTracker(t);
    setState(t, { status: `Error: ${e}`, progress: 0, downloading: false });
    showToast(
      t.title ? `Couldn't start download: ${t.title}` : "Couldn't start download",
      "error",
      { detail: String(e) },
    );
  });
}

/** Re-arm a tracker for every download the session resumed on its own
 *  (librqbit persists selections). The poll is what shows progress AND what
 *  extracts on completion, so without this a restarted download finishes
 *  invisibly and never installs. */
export async function resumeDownloads() {
  let pending: Awaited<ReturnType<typeof listActiveDownloads>>;
  try { pending = await listActiveDownloads(); } catch { return; }
  for (const { id, title, torrent_source, thumbnail_key } of pending) {
    if (trackers.has(id)) { continue; }
    // No download_game call to wait for: the session already has the file.
    const t = newTracker(id, title, false, { source: torrent_source, key: thumbnail_key });
    setState(t, { status: "Resuming download…", progress: 0, downloading: true });
    void poll(t);
  }
}

/** Pick up a download the backend started by itself: the English base an
 *  overlay language variant sits on. Without a tracker nobody polls it, and
 *  the poll is what extracts and installs. Registered once at mount. */
export async function initDependencyDownloads() {
  await listen<{ id: number; title: string; torrentSource: string | null; thumbnailKey: string | null }>(
    "dependency-download-started",
    (event) => {
      const { id, title, torrentSource, thumbnailKey } = event.payload;
      if (trackers.has(id)) { return; }
      const cover = torrentSource ? { source: torrentSource, key: thumbnailKey } : undefined;
      const t = newTracker(id, title, false, cover);
      setState(t, { status: "Starting download...", progress: 0, downloading: true });
      void poll(t);
    },
  );
}

/** Stop tracking a game in any phase (uninstall during extras would
 *  otherwise get a phantom card back). */
export function stopGameDownloadTracking(gameId: number) {
  const t = trackers.get(gameId);
  if (t) { endTracker(t); }
  clearState(gameId);
}

/** Stop tracking every download (going offline drops the managers, and
 *  null progress would read as "didn't start"). Returns the count. */
export function stopAllDownloadTracking(): number {
  const active = Object.keys(downloads()).map(Number).filter((id) => downloads()[id]?.downloading);
  for (const id of active) {
    stopGameDownloadTracking(id);
  }
  return active.length;
}

/** Resume polling an installed game's extras after a restart, so the phase
 *  stays visible. No-op when tracked or done. */
export async function watchExtrasIfPending(gameId: number, title?: string) {
  if (trackers.has(gameId) || getDownloadState(gameId)) { return; }
  try {
    const p = await getDownloadProgress(gameId);
    if (!p || !p.installed || p.extras_done !== false) { return; }
  } catch { return; }
  startGameDownload(gameId, title);
}

export async function cancelGameDownload(gameId: number) {
  const t = trackers.get(gameId);
  if (t) { endTracker(t); }
  clearState(gameId);
  try {
    // Cancelling an English base takes the translations that were waiting on
    // it: without it they can never install, and their card would sit at
    // "Waiting for…" forever.
    const alsoCancelled = await cancelDownload(gameId);
    // Second sweep after the slow backend cancel; a newer download owns
    // the entry.
    if (!trackers.has(gameId)) {
      clearState(gameId);
    }
    for (const dep of alsoCancelled ?? []) {
      const dt = trackers.get(dep.id);
      if (dt) { endTracker(dt); }
      clearState(dep.id);
    }
    if (alsoCancelled?.length) {
      showToast(
        alsoCancelled.length === 1
          ? `${alsoCancelled[0].title} was cancelled too - it needs the English version`
          : `${alsoCancelled.length} translations were cancelled too - they need the English version`,
        "info",
      );
    }
    refreshLoadedGames();
  } catch {}
}
