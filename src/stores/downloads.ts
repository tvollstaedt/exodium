import { createSignal } from "solid-js";
import { cancelDownload, downloadGame, getDownloadProgress } from "../api/tauri";
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
}

const [downloads, setDownloads] = createSignal<Record<number, DownloadState>>({});

const POLL_MS = 1000;
// Consecutive poll ticks where getDownloadProgress returned null despite the
// download being marked in-flight. If this stays high for >5s we surface a
// user-visible error instead of pretending we're still starting. Observed on
// Windows: if session.add_torrent() fails (MAX_PATH, port bind, etc.) the
// handle stays None forever and file_progress returns None silently.
const NULL_POLL_THRESHOLD = 5; // ~5 seconds at 1s polling interval
// Seconds without progress before the status turns into peer-wait feedback,
// and before it becomes an actionable stall warning.
const STALL_HINT_SECS = 15;
const STALL_WARN_SECS = 90;

/** Everything one in-flight download knows about itself.
 *
 *  One object per download rather than a table per field: ending a run is a
 *  single `trackers.delete`, and cancellation is a single flag the poll loop
 *  re-reads after every await. The previous shape kept these twelve fields in
 *  twelve module-level records, which meant every exit path had to empty all
 *  of them by hand and a forgotten one leaked state into the next attempt. */
interface Tracker {
  gameId: number;
  /** Kept on the tracker so status writes inside the loop don't have to
   *  re-pass the title on every tick. */
  title?: string;
  /** Set by whoever ends this run (cancel, uninstall, a newer attempt, or a
   *  terminal poll result). Checked after every await, which is what stops a
   *  poll that was already in flight at cancel time from writing the store
   *  back and resurrecting the card. */
  cancelled: boolean;
  /** True while the download_game backend command is still in flight.
   *  Progress legitimately polls null during that window (torrent handle not
   *  attached yet, validation pass, first-ever torrent add), so the
   *  didn't-start verdict must not fire until the command has resolved. */
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
  /** Stall detection: value + timestamp of the last observed progress
   *  increase, for this file and for the whole torrent. A game's file can sit
   *  at exactly 0 for minutes while data pours in: pieces are 8 MB and most
   *  games are far smaller, so a re-download after uninstall has to refetch
   *  the entire block the game shares with its neighbours, and per-file
   *  progress only moves when that block validates. Without the torrent-level
   *  pair the honest "no data received" warning fires on a download that is
   *  working perfectly. */
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

function setState(t: Tracker, state: Omit<DownloadState, "title">) {
  setDownloads((prev) => ({ ...prev, [t.gameId]: { ...state, title: t.title } }));
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
    // Backend returned null - torrent handle not attached yet. While the
    // download_game command is still running that's expected (first-ever
    // torrent add + validation can take a while) - keep waiting. Only once
    // the command has resolved do consecutive misses indicate the
    // silent-stuck bug.
    if (t.commandPending) {
      t.nullPolls = 0;
      // The backend can legitimately spend minutes here on the FIRST download
      // of a collection (placeholder creation + hash check of 14k files, slow
      // on Windows). Say so instead of sitting mute on "Starting download..." -
      // testers read that as a hang.
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
    // Delay cleanup so isInstalled() stays true until fetchGames() propagates
    // the updated installed flag from the DB into the games store. Skipped if
    // a new download for the same game started in the meantime - that one owns
    // the entry now.
    setTimeout(() => {
      if (trackers.has(t.gameId)) { return; }
      clearState(t.gameId);
    }, 5000);
    return;
  }

  if (p.finished) {
    t.stuckSince = 0;
    setState(t, { status: "Extracting...", progress: safeProgress, downloading: true });
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
    // librqbit is hash-checking the entire torrent's existing on-disk content
    // before any peer pieces are requested. On Windows with thousands of
    // placeholder files this can take 5–10 minutes the first time. Per-file
    // progress stays at 0 the whole time, so we surface the torrent-level
    // validation progress to the user.
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
  // Data is arriving for the torrent even if none of it has landed in this
  // game's file yet - so this is a wait, not a fault.
  //
  // Two signals, because torrent progress also moves in whole pieces: at
  // 50 KB/s an 8 MB piece takes over two minutes, so on a slow line the
  // per-piece signal goes quiet exactly like a real stall. The session byte
  // rate is continuous and settles it.
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

export function startGameDownload(gameId: number, title?: string) {
  // A still-running attempt for the same game must not write the store on
  // behalf of this one.
  const previous = trackers.get(gameId);
  if (previous) { endTracker(previous); }

  const now = Date.now();
  const t: Tracker = {
    gameId,
    title: title ?? previous?.title ?? downloads()[gameId]?.title,
    cancelled: false,
    commandPending: true,
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

/** Stop any polling/UI state for a game regardless of phase - used by
 *  uninstall, which may run during the extras phase where downloading is
 *  false but a poll loop is still alive (it would otherwise resurrect a
 *  phantom stuck/failed card for the freshly uninstalled game). */
export function stopGameDownloadTracking(gameId: number) {
  const t = trackers.get(gameId);
  if (t) { endTracker(t); }
  clearState(gameId);
}

/** Stop tracking every in-flight download and report how many there were.
 *  Going offline drops the torrent managers, after which `getDownloadProgress`
 *  returns null forever - the poll loop would read that as the silent-stuck
 *  bug and label a perfectly healthy download "Download didn't start". The
 *  torrent selection stays in the DB, so switching back online resumes it. */
export function stopAllDownloadTracking(): number {
  const active = Object.keys(downloads()).map(Number).filter((id) => downloads()[id]?.downloading);
  for (const id of active) {
    stopGameDownloadTracking(id);
  }
  return active.length;
}

/** Restart-resume for the extras phase: an installed game whose GameData
 *  was still downloading when the app quit resumes invisibly (librqbit
 *  session restore) - poll it so the phase stays visible and the completion
 *  refresh fires. No-op when a tracker already exists or extras are done. */
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
    await cancelDownload(gameId);
    // Second sweep: cancel_download can take seconds (deselect + session
    // bookkeeping), and anything that wrote the store in the meantime would
    // otherwise leave a card behind. Skipped when a new download for the same
    // game started while this was running - that one owns the entry now.
    if (!trackers.has(gameId)) {
      clearState(gameId);
    }
    refreshLoadedGames();
  } catch {}
}
