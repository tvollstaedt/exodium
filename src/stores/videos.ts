import { createSignal } from "solid-js";
import { startGameVideo, getVideoStatus, cancelGameVideo, videoPlaybackSupported, type VideoStatus } from "../api/tauri";
import { requestSlot, releaseSlot, isActive, activeCount, queuedCount, type MediaJob } from "./mediaQueue";

/** Preview-video state per game.
 *
 *  The video is streamed out of the game's GameData archive, which can take a
 *  minute when the torrent is cold - so this mirrors the downloads store: fire
 *  and poll, never block the panel.
 *
 *  Closing the panel does NOT stop a fetch; the point of the feature is that
 *  the video is simply there next time. But each fetch is a torrent stream with
 *  its own 32 MB lookahead, and several at once fight over the same peers, so
 *  only MAX_CONCURRENT run at a time.
 *
 *  Anything over the limit WAITS - it is never dropped. An earlier version
 *  deleted the evicted entry, and the panel then showed nothing at all, which
 *  reads as "this game has no video". A queued fetch keeps its place and says
 *  so. Giving up a slot is cheap either way: librqbit writes fetched pieces
 *  into the archive on disk, so resuming finds them locally.
 *
 *  The slots themselves live in `mediaQueue` and are shared with the theme
 *  tracks, which come out of the same archives over the same kind of stream. */
const [videos, setVideos] = createSignal<Record<number, VideoStatus>>({});
export { videos };

const POLL_MS = 700;

/** Frontend-only phase: waiting for a slot. */
export const PHASE_QUEUED = "queued";
/** Backend is reading the archive index - existence of a video is still open. */
export const PHASE_PROBING = "probing";

const intervals: Record<number, ReturnType<typeof setInterval>> = {};
/** The game whose panel is open; always gets a slot, never evicted. */
let foreground: number | null = null;

const KEY_PREFIX = "v:";
const keyOf = (gameId: number) => `${KEY_PREFIX}${gameId}`;

export function getVideoState(gameId: number): VideoStatus | undefined {
  return videos()[gameId];
}

function put(gameId: number, status: VideoStatus) {
  // Offline (or for a collection with no torrent manager) the backend answers
  // "none" instantly - the archive was never opened, so that is not an
  // inventory answer. It says so by carrying OFFLINE_TOKEN in `error`
  // (media.rs: phase "none" plus a non-null error). Keeping it would blacklist
  // the game for the rest of the session - `requestVideo` refuses to ask
  // twice - so the preview would stay missing after the app went back online.
  // It is recorded and forgotten again, which shows the panel nothing (not an
  // error, not a retry button) and lets the next open probe once more.
  const provisional = status.phase === "none" && status.error != null;
  setVideos((prev) => ({ ...prev, [gameId]: status }));
  if (provisional) { forgetVideo(gameId); }
}

/** Forget the entry, so the next `requestVideo` probes again instead of
 *  finding a status nothing is working on any more. */
function forgetVideo(gameId: number) {
  setVideos((prev) => {
    if (!(gameId in prev)) { return prev; }
    const next = { ...prev };
    delete next[gameId];
    return next;
  });
}

function queuedStatus(previous?: VideoStatus): VideoStatus {
  return {
    phase: PHASE_QUEUED,
    progress: 0,
    // Carry the known size across an eviction: it is the panel's signal that a
    // video was confirmed, and losing it would make a confirmed video look
    // like an open question again.
    total_bytes: previous?.total_bytes ?? 0,
    path: null,
    error: null,
  };
}

function stopPolling(gameId: number) {
  clearInterval(intervals[gameId]);
  delete intervals[gameId];
  releaseSlot(keyOf(gameId));
}

/** The slot went to something more important: the backend read is cancelled
 *  and the entry shows as queued - never dropped, so the panel keeps saying
 *  something true about it. */
function onEvicted(gameId: number) {
  clearInterval(intervals[gameId]);
  delete intervals[gameId];
  cancelGameVideo(gameId).catch(() => {});
  put(gameId, queuedStatus(videos()[gameId]));
}

function jobFor(gameId: number): MediaJob {
  return {
    key: keyOf(gameId),
    // The visible game outranks everything; a background video waits behind
    // the track the player is waiting on.
    priority: () => (foreground === gameId ? 0 : 2),
    run: () => beginFetch(gameId),
    onEvicted: () => onEvicted(gameId),
    onQueued: () => put(gameId, queuedStatus(videos()[gameId])),
  };
}

async function beginFetch(gameId: number) {
  if (intervals[gameId]) { return; }
  let initial: VideoStatus;
  try {
    initial = await startGameVideo(gameId);
  } catch (e) {
    put(gameId, { phase: "error", progress: 0, total_bytes: 0, path: null, error: String(e) });
    stopPolling(gameId);
    return;
  }
  // Deliberately outside the try: writing the status runs subscribers
  // synchronously, and a throw in one of them is a UI bug, not a failed fetch.
  // Recording it as one hid a perfectly good video behind a retry button.
  if (!initial) {
    stopPolling(gameId);
    return;
  }
  put(gameId, initial);
  if (initial.phase !== "fetching" && initial.phase !== PHASE_PROBING) {
    // Cached, absent, or failed outright - nothing to poll for.
    stopPolling(gameId);
    return;
  }

  intervals[gameId] = setInterval(async () => {
    try {
      const status = await getVideoStatus(gameId);
      // The backend has no job for this game any more (cancelled, or the
      // session restarted). Leaving the last "fetching" behind would freeze
      // the panel there and make requestVideo refuse to ask again.
      if (!status) { forgetVideo(gameId); stopPolling(gameId); return; }
      put(gameId, status);
      if (status.phase !== "fetching" && status.phase !== PHASE_PROBING) { stopPolling(gameId); }
    } catch {
      stopPolling(gameId);
    }
  }, POLL_MS);
}

/** Ask for a game's video. Runs now if a slot is free (or if this is the game
 *  on screen), waits otherwise. */
/** null until the one-time probe answers; the panel uses this to explain WHY
 *  there is no preview rather than silently showing none. */
const [playbackUnsupported, setPlaybackUnsupported] = createSignal(false);
export { playbackUnsupported as videoPlaybackUnsupported };

let supportKnown: Promise<boolean> | null = null;
function ensurePlaybackSupportKnown(): Promise<boolean> {
  // An unreachable probe must not disable previews on the platforms that have
  // no problem - only an explicit "no" does.
  // Only an explicit "no" disables the feature. A missing command, an odd
  // payload or a failed invoke must not switch previews off on the platforms
  // that have no problem.
  supportKnown ??= videoPlaybackSupported()
    .then((ok) => { const unsupported = ok === false; setPlaybackUnsupported(unsupported); return !unsupported; })
    .catch(() => true);
  return supportKnown;
}

export async function requestVideo(gameId: number) {
  // Fetching would be wasted torrent traffic for a video that must never be
  // mounted - on an affected system the <video> element itself is what
  // freezes the app, so the whole feature stands down.
  if (!(await ensurePlaybackSupportKnown())) { return; }
  const known = videos()[gameId];
  if (known && known.phase !== "error" && known.phase !== PHASE_QUEUED) { return; }
  if (intervals[gameId] || isActive(keyOf(gameId))) { return; }

  // The visible game jumps the queue - waiting behind a fetch nobody is
  // looking at is the one case where the cap would be felt as a bug. The
  // scheduler evicts a background fetch for it, or queues it otherwise.
  await requestSlot(jobFor(gameId));
}

/** Mark which game the panel is showing, so its fetch is never the one evicted
 *  and it can jump the queue. */
export function setForegroundVideo(gameId: number | null) {
  foreground = gameId;
}

/** The panel moved on. The fetch keeps running - it just loses its protection
 *  from eviction when other games queue up behind it. */
export function releaseVideo(gameId: number) {
  if (foreground === gameId) { foreground = null; }
}

/** In-flight fetches (for tests and diagnostics). */
export function activeVideoCount(): number {
  return activeCount(KEY_PREFIX);
}

/** Fetches waiting for a slot (for tests and diagnostics). */
export function queuedVideoCount(): number {
  return queuedCount(KEY_PREFIX);
}
