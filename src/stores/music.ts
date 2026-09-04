import { createEffect, createRoot, createSignal, on } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  startGameMusic, getMusicStatus, cancelGameMusic, musicPlaybackSupported, musicShuffleCandidates, musicCacheIndex, mediaUrl,
  type Game, type MediaStatus, type MusicCandidate, type MusicSupport,
} from "../api/tauri";
import { createConfigSignal } from "./configSignal";
import { requestSlot, releaseSlot, dropQueued, isActive, type MediaJob } from "./mediaQueue";
import { games, hasMore, fetchMoreGames } from "./games";
import { isOffline, networkMode } from "./network";

/** The music player.
 *
 *  A game's theme track sits in its GameData archive next to the preview
 *  video and is fetched the same way (see `videos.ts`): fire a backend job,
 *  poll, never block. What this store adds is the player on top - one track
 *  loaded at a time, a "wanted" track that becomes the current one when its
 *  bytes arrive, and the two modes: the theme of the game on screen, or a
 *  shuffle across the whole collection.
 *
 *  Two rules keep it from fighting the rest of the app for the speakers. A
 *  preview video playing WITH sound pauses the music, and a running game
 *  does too; each hands in a reason, and playback resumes only when every
 *  reason is withdrawn. The reasons are a level, not an edge: they are
 *  recorded whether or not anything was playing at the time, so a track that
 *  loads WHILE a game runs stays quiet. A game launched into silence still
 *  starts nothing on exit - that is `userPaused` doing the work, not the
 *  reason set. And the shuffle never starts on its own: opening the app must
 *  not make noise. */

export interface Track {
  /** Where the track came from. Only the GameData theme exists today; an
   *  album from the media pack would be a second source in the same list. */
  source: "gamedata";
  gameId: number;
  title: string;
  collection: string | null;
  thumbnailKey: string | null;
}

/** What the store needs from an `<audio>` element - small enough that tests
 *  hand in a fake and the bar hands in the real one. */
export interface AudioPort {
  setSrc(url: string | null): void;
  play(): Promise<void>;
  pause(): void;
  setVolume(volume: number): void;
  onEnded(cb: () => void): void;
}

export type PauseReason = "video" | "game";

const POLL_MS = 700;
/** A shuffle pick nobody seeds must not hold the player: past this it is
 *  dropped for the next candidate, silently - the listener asked for music,
 *  not for a report on the swarm. */
export const SHUFFLE_SKIP_MS = 60_000;
const CANDIDATE_BATCH = 10;
const CANDIDATE_LOW_WATER = 3;
const HISTORY_MAX = 50;

/** Frontend-only phase while a fetch waits for a slot. */
export const MUSIC_QUEUED = "queued";

// ── Preferences ──────────────────────────────────────────────────────────────

/** Start a game's theme when its details open. On unless switched off: the
 *  preview video already plays with sound there. */
const autoplay = createConfigSignal<boolean>(
  "music_autoplay",
  true,
  (raw) => (raw == null ? true : raw === "1"),
  (v) => (v ? "1" : "0"),
);
export const ensureMusicAutoplayLoaded = autoplay.ensureLoaded;
export const musicAutoplay = autoplay.value;
export const setMusicAutoplay = autoplay.set;

/** Keep going when a track ends. Off means a queue plays only the track it
 *  was pointed at - the ⏭ button still walks it by hand. */
const continuous = createConfigSignal<boolean>(
  "music_continuous",
  true,
  (raw) => (raw == null ? true : raw === "1"),
  (v) => (v ? "1" : "0"),
);
export const ensureMusicContinuousLoaded = continuous.ensureLoaded;
export const musicContinuous = continuous.value;
export const setMusicContinuous = continuous.set;

const volume = createConfigSignal<number>(
  "music_volume",
  0.8,
  (raw) => {
    const v = raw == null ? NaN : Number(raw);
    return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0.8;
  },
  (v) => v.toFixed(2),
);
export const musicVolume = volume.value;
export function setMusicVolume(v: number) {
  port?.setVolume(v);
  return volume.set(v);
}

// ── Fetch state (mirrors videos.ts) ──────────────────────────────────────────

const [musicJobs, setMusicJobs] = createSignal<Record<number, MediaStatus>>({});
export { musicJobs };

export function getMusicState(gameId: number): MediaStatus | undefined {
  return musicJobs()[gameId];
}

const intervals: Record<number, ReturnType<typeof setInterval>> = {};
const KEY_PREFIX = "m:";
const keyOf = (gameId: number) => `${KEY_PREFIX}${gameId}`;
/** Ids somebody still wants the bytes of: the panel's own probe, the player's
 *  wanted track, the one prefetch. A job that reaches its slot with nothing in
 *  here any more gives the slot straight back. */
const requested = new Set<number>();

function put(gameId: number, status: MediaStatus) {
  // Offline (or for a collection with no torrent manager) the backend answers
  // "none" instantly - not an inventory answer, so it carries OFFLINE_TOKEN
  // in `error` (media.rs). Remembering it would blacklist the game for the
  // rest of the session and, in a queue, spin the walk. So it is recorded
  // nowhere and forgotten again right after the player has seen it, which
  // lets a later probe ask once more. `isOffline()` stays as a second guard
  // for a backend that predates the token.
  const provisional = status.phase === "none" && (status.error != null || isOffline());
  setMusicJobs((prev) => ({ ...prev, [gameId]: status }));
  if (!provisional) { noteInIndex(gameId, status.phase); }
  reconcile();
  if (provisional) { forgetJob(gameId); }
}

/** Drop the entry entirely, so the next request probes again rather than
 *  finding a status nothing is working on any more. */
function forgetJob(gameId: number) {
  requested.delete(gameId);
  setMusicJobs((prev) => {
    if (!(gameId in prev)) { return prev; }
    const next = { ...prev };
    delete next[gameId];
    return next;
  });
  // The player may have been waiting on exactly this job. `reconcile` reads
  // the entry, so with the entry gone nothing would ever answer for it: the
  // bar sits on "Loading…" for the rest of the session and a queue never
  // moves again. Every caller that means to keep the wait clears `wanted`
  // itself first, so reaching here with it still set IS the stranded case.
  if (wanted()?.gameId === gameId) {
    clearSkipTimer();
    setWanted(null);
    if (autoAdvances()) { advancePastDud(); }
  }
}

/** Nobody is waiting for these bytes any more: stop the backend job, free the
 *  slot, take it out of the queue and forget it. */
function abandonFetch(gameId: number) {
  dropQueued(keyOf(gameId));
  cancelGameMusic(gameId).catch(() => {});
  stopPolling(gameId);
  forgetJob(gameId);
}

/** The lighter version for a fetch that has not started yet: it only ever cost
 *  a place in the queue, so there is no backend job to cancel. */
function releaseQueued(gameId: number) {
  if (musicJobs()[gameId]?.phase !== MUSIC_QUEUED) { return; }
  dropQueued(keyOf(gameId));
  forgetJob(gameId);
}

function queuedStatus(previous?: MediaStatus): MediaStatus {
  return { phase: MUSIC_QUEUED, progress: 0, total_bytes: previous?.total_bytes ?? 0, path: null, error: null };
}

function stopPolling(gameId: number) {
  clearInterval(intervals[gameId]);
  delete intervals[gameId];
  // The job is over, so the request that carried it is too. An EVICTION goes
  // through onEvicted instead and keeps it: that job comes back.
  requested.delete(gameId);
  releaseSlot(keyOf(gameId));
}

function onEvicted(gameId: number) {
  clearInterval(intervals[gameId]);
  delete intervals[gameId];
  cancelGameMusic(gameId).catch(() => {});
  put(gameId, queuedStatus(musicJobs()[gameId]));
}

function jobFor(gameId: number): MediaJob {
  return {
    key: keyOf(gameId),
    // The track the player is waiting on beats background videos and the
    // prefetch; the game on screen (its video) still comes first.
    priority: () => (wanted()?.gameId === gameId ? 1 : 2),
    run: () => beginFetch(gameId),
    onEvicted: () => onEvicted(gameId),
    onQueued: () => put(gameId, queuedStatus(musicJobs()[gameId])),
  };
}

function isInFlight(phase: string | undefined) {
  return phase === "fetching" || phase === "probing";
}

/** Does anyone still want this track? Priorities and intent both move while a
 *  job waits for a slot - the pick may have been skipped, the player stopped. */
function stillWanted(gameId: number): boolean {
  return wanted()?.gameId === gameId || upNext?.gameId === gameId || requested.has(gameId);
}

async function beginFetch(gameId: number) {
  if (intervals[gameId]) { return; }
  if (!stillWanted(gameId)) {
    // Queued long enough for its reason to disappear: give the slot back
    // instead of streaming bytes for a track nobody will hear.
    forgetJob(gameId);
    stopPolling(gameId);
    return;
  }
  let initial: MediaStatus;
  try {
    initial = await startGameMusic(gameId);
  } catch (e) {
    put(gameId, { phase: "error", progress: 0, total_bytes: 0, path: null, error: String(e) });
    stopPolling(gameId);
    return;
  }
  if (!initial) {
    // An older backend without the command: not an error, just no theme -
    // but nothing will ever reconcile it either, so it goes through
    // forgetJob rather than leaving a wait behind.
    forgetJob(gameId);
    stopPolling(gameId);
    return;
  }
  put(gameId, initial);
  if (!isInFlight(initial.phase)) {
    stopPolling(gameId);
    return;
  }
  intervals[gameId] = setInterval(async () => {
    try {
      const status = await getMusicStatus(gameId);
      // The backend forgot the job (a cancel, a restart): the entry goes with
      // it, or it would sit at "fetching" forever and block every re-probe.
      if (!status) { forgetJob(gameId); stopPolling(gameId); return; }
      put(gameId, status);
      if (!isInFlight(status.phase)) { stopPolling(gameId); }
    } catch {
      stopPolling(gameId);
    }
  }, POLL_MS);
}

// ── Cache index ──────────────────────────────────────────────────────────────

/** What the backend already knows about every game's theme, so a list can
 *  render its play affordances without a probe per row: the ids whose track
 *  is cached, and the ids a finished probe found nothing for. Both sets are
 *  REPLACED on change - a mutated Set is the same object and Solid would not
 *  see it. */
const [musicCached, setMusicCached] = createSignal<Set<number>>(new Set());
const [musicNone, setMusicNone] = createSignal<Set<number>>(new Set());
export { musicCached, musicNone };

let indexing: Promise<void> | null = null;

export async function refreshMusicIndex(): Promise<void> {
  indexing ??= musicCacheIndex()
    .then((index) => {
      setMusicCached(new Set(index?.cached ?? []));
      setMusicNone(new Set(index?.none ?? []));
    })
    .catch((e) => { console.warn("[music] cache index failed:", e); })
    .finally(() => { indexing = null; });
  await indexing;
}

/** Keep the sets in step with what the fetches learn, so a row can lose its
 *  play button (or gain one) without a reload. */
function noteInIndex(gameId: number, phase: string) {
  if (phase === "ready") {
    if (!musicCached().has(gameId)) { setMusicCached((prev) => new Set(prev).add(gameId)); }
    return;
  }
  if (phase !== "none") { return; }
  if (!musicNone().has(gameId)) { setMusicNone((prev) => new Set(prev).add(gameId)); }
  if (musicCached().has(gameId)) {
    setMusicCached((prev) => {
      const next = new Set(prev);
      next.delete(gameId);
      return next;
    });
  }
}

// ── Playback support ─────────────────────────────────────────────────────────

const [musicUnsupported, setMusicUnsupported] = createSignal(false);
export { musicUnsupported };
let support: MusicSupport | null = null;
let supportKnown: Promise<boolean> | null = null;

/** Only an explicit "no mp3" switches the feature off - a missing command or
 *  a failed probe must not silence the platforms that have no problem. */
function ensureSupportKnown(): Promise<boolean> {
  supportKnown ??= musicPlaybackSupported()
    .then((s) => {
      support = s;
      const off = s?.mp3 === false;
      setMusicUnsupported(off);
      return !off;
    })
    .catch(() => true);
  return supportKnown;
}

/** The two containers the webview is offered, minus a codec the platform
 *  probe ruled out. Tracker modules and the like are not playable. */
function playable(fileName: string): boolean {
  const lower = fileName.toLowerCase();
  // An unanswered probe means "no reason to think otherwise" - only an
  // explicit no rules a container out.
  if (lower.endsWith(".ogg")) { return support?.ogg !== false; }
  if (lower.endsWith(".mp3")) { return support?.mp3 !== false; }
  return false;
}

/** Is this row worth offering a play button? The catalogue hint plus what we
 *  have since learned - the archive still has the last word. */
export function playableHint(game: Pick<Game, "id" | "music_file">): boolean {
  if (game.id == null) { return false; }
  // The codec probe comes first: bytes on disk say nothing about whether the
  // webview can decode them, so a cached .mp3 on a box without an mp3 decoder
  // is still not worth a play button.
  if (game.music_file != null && !playable(game.music_file)) { return false; }
  // Past that, a cached track plays whatever the catalogue claims about it.
  if (musicCached().has(game.id)) { return true; }
  if (musicNone().has(game.id)) { return false; }
  return game.music_file != null;
}

/** Ask for a game's theme without changing what plays. The panel calls this
 *  when autoplay is off, so its row can still offer the track. Answers false
 *  when nothing will come of it, so a caller waiting on the bytes can stop
 *  waiting. */
export async function requestTheme(gameId: number): Promise<boolean> {
  if (!(await ensureSupportKnown())) { return false; }
  const known = musicJobs()[gameId];
  if (known && known.phase !== "error" && known.phase !== MUSIC_QUEUED) { reconcile(); return true; }
  if (intervals[gameId] || isActive(keyOf(gameId))) { return true; }
  requested.add(gameId);
  await requestSlot(jobFor(gameId));
  return true;
}

// ── Player ───────────────────────────────────────────────────────────────────

const [currentTrack, setCurrentTrack] = createSignal<Track | null>(null);
const [wanted, setWanted] = createSignal<Track | null>(null);
/** "theme" plays one track and stops; "shuffle" walks the whole collection;
 *  "list" walks the Browse list the user is looking at. */
const [mode, setMode] = createSignal<"theme" | "shuffle" | "list">("theme");
const [playing, setPlaying] = createSignal(false);
const [userPaused, setUserPaused] = createSignal(false);
const [pauseReasons, setPauseReasons] = createSignal<PauseReason[]>([]);
/** The bar's × was clicked: the track stays loaded, only the bar is gone.
 *  The top bar's ♪ brings it back - hiding is not stopping. */
const [barHidden, setBarHidden] = createSignal(false);
/** The bar's cover was clicked: Library opens this game's panel. */
const [openGameRequest, setOpenGameRequest] = createSignal<number | null>(null);
export { currentTrack, wanted as wantedTrack, mode as musicMode, playing as musicPlaying, userPaused as musicUserPaused, pauseReasons, barHidden as playerHidden, openGameRequest, setOpenGameRequest };

let port: AudioPort | null = null;
const reasons = new Set<PauseReason>();
let history: Track[] = [];
let upNext: Track | null = null;
let candidates: MusicCandidate[] = [];
let refilling: Promise<void> | null = null;
/** Where the list walk stands. Held apart from the player because a wanted
 *  track that turns out to have no theme is cleared before the walk resumes. */
let listCursor: number | null = null;
let skipTimer: ReturnType<typeof setTimeout> | undefined;
/** Bumped per load so a slow URL lookup cannot land on a later track. */
let loadSeq = 0;
/** Duds walked past since the last track that actually played. A queue that
 *  finds nothing anywhere (no torrent manager, an exhausted collection) would
 *  otherwise ask the backend for candidates forever; past the cap the walk
 *  simply stops and the bar keeps the last track. */
let autoSkips = 0;
const AUTO_SKIP_MAX = 5;
/** Picks this walk has already asked for, so a refill cannot hand back an id
 *  the walk just failed on. Cleared whenever a track loads. */
const triedThisWalk = new Set<number>();
/** The pick `prev()` is walking back to: its load must not push the outgoing
 *  track onto the history it came from, or ⏮ bounces between two songs. */
let steppingBackTo: number | null = null;

export function attachAudio(next: AudioPort | null) {
  port = next;
  if (!port) { return; }
  port.onEnded(handleEnded);
  port.setVolume(musicVolume());
}

function trackOf(game: Pick<Game, "id" | "title" | "torrent_source" | "thumbnail_key">, id: number): Track {
  return { source: "gamedata", gameId: id, title: game.title, collection: game.torrent_source, thumbnailKey: game.thumbnail_key };
}

function trackOfCandidate(c: MusicCandidate): Track {
  return { source: "gamedata", gameId: c.id, title: c.title, collection: c.torrent_source, thumbnailKey: c.thumbnail_key };
}

function play() {
  if (!port || !currentTrack()) { return; }
  const started = port.play();
  if (started && typeof started.then === "function") {
    started.then(() => setPlaying(true)).catch(() => setPlaying(false));
  } else {
    setPlaying(true);
  }
}

function pause() {
  port?.pause();
  setPlaying(false);
}

function clearSkipTimer() {
  if (skipTimer) { clearTimeout(skipTimer); skipTimer = undefined; }
}

/** Both queue modes move on by themselves; theme mode plays one track. */
const autoAdvances = () => mode() !== "theme";

/** Set the track the player is waiting on. It becomes the current one the
 *  moment its bytes are on disk; until then whatever plays keeps playing. */
function want(track: Track) {
  clearSkipTimer();
  if (steppingBackTo !== track.gameId) { steppingBackTo = null; }
  const previous = wanted();
  // Asking for a track is an implicit "show me the player".
  setBarHidden(false);
  setWanted(track);
  // The pick being replaced may still be waiting for a slot; it is nobody's
  // track any more, so it must not take one. Released only AFTER the new
  // wanted track is in place: `forgetJob` treats a dropped entry the player
  // is still waiting on as a dud, and would otherwise skip this very pick.
  if (previous && previous.gameId !== track.gameId && previous.gameId !== upNext?.gameId) {
    releaseQueued(previous.gameId);
  }
  triedThisWalk.add(track.gameId);
  if (mode() === "list") { listCursor = track.gameId; }
  void startWantedFetch(track);
}

/** The skip timer is armed only once the fetch is actually under way: a
 *  request the support probe turned down never arrives, and a wait on it would
 *  leave the bar loading for a minute before the queue moved on. */
async function startWantedFetch(track: Track) {
  const accepted = await requestTheme(track.gameId);
  if (wanted()?.gameId !== track.gameId) { return; }
  if (!accepted) {
    // Refused means the platform cannot decode anything we have - no fetch
    // was started and none ever will be. Leaving the queue armed would park
    // the bar on a walk that can never move, so it is stood down here.
    setWanted(null);
    listCursor = null;
    if (!currentTrack()) { setMode("theme"); }
    return;
  }
  if (!autoAdvances()) { return; }
  skipTimer = setTimeout(() => {
    const w = wanted();
    if (!w || w.gameId !== track.gameId) { return; }
    if (getMusicState(w.gameId)?.phase === "ready") { return; }
    // Cleared before the abandon: `forgetJob` skips for a wanted track whose
    // entry disappears, and the walk below is that skip.
    setWanted(null);
    abandonFetch(w.gameId);
    advancePastDud();
  }, SHUFFLE_SKIP_MS);
}

/** The wanted track's bytes arrived (or never will): act on it. Runs after
 *  every status change, so the wait costs no polling of its own. */
function reconcile() {
  const w = wanted();
  if (!w) { return; }
  const state = getMusicState(w.gameId);
  if (!state) { return; }
  if (state.phase === "ready" && state.path) {
    clearSkipTimer();
    setWanted(null);
    void load(w, state.path);
  } else if (state.phase === "none" || state.phase === "error") {
    // No theme, or a failed read: in theme mode the previous track keeps
    // playing and the panel says what happened; a queue skips the dud.
    clearSkipTimer();
    setWanted(null);
    if (autoAdvances()) { advancePastDud(); }
  }
}

/** Walk past a pick that produced nothing. Bounded, because every arm of the
 *  walk can dud out at once - offline, or a collection whose archives hold no
 *  music - and an unbounded walk is a livelock that talks to the backend. */
function advancePastDud() {
  autoSkips += 1;
  if (autoSkips >= AUTO_SKIP_MAX) {
    autoSkips = 0;
    // The walk is over, so its blacklist goes with it. Kept, it would follow
    // the listener into the NEXT shuffle and filter out picks whose only sin
    // was being tried while the swarm was quiet.
    triedThisWalk.clear();
    setWanted(null);
    pause();
    return;
  }
  void advance();
}

async function load(track: Track, path: string) {
  const seq = ++loadSeq;
  let url: string;
  try {
    url = (await mediaUrl(path)) ?? convertFileSrc(path);
  } catch {
    url = convertFileSrc(path);
  }
  if (seq !== loadSeq) { return; }
  // A track that plays ends the dud streak and starts a fresh walk.
  autoSkips = 0;
  triedThisWalk.clear();
  const steppingBack = steppingBackTo === track.gameId;
  steppingBackTo = null;
  const previous = currentTrack();
  // Stepping back, the outgoing track goes to `upNext`, not onto the history
  // we are walking down - pushing it there makes ⏮ bounce between two songs.
  if (!steppingBack && previous && previous.gameId !== track.gameId) {
    history = [...history.filter((t) => t.gameId !== previous.gameId), previous].slice(-HISTORY_MAX);
  }
  setCurrentTrack(track);
  if (upNext?.gameId === track.gameId) { upNext = null; }
  setPlaying(false);
  port?.setSrc(url);
  if (!userPaused() && reasons.size === 0) { play(); }
  // No prefetch when nothing will follow: the bytes would never be played.
  if (autoAdvances() && musicContinuous()) { void prefetchNext(); }
}

function handleEnded() {
  setPlaying(false);
  if (autoAdvances() && musicContinuous()) { void next(); }
}

/** Play a game's theme: the panel's autoplay, and the row's play button. */
export function playTheme(game: Pick<Game, "id" | "title" | "torrent_source" | "thumbnail_key">) {
  if (game.id == null) { return; }
  const track = trackOf(game, game.id);
  setMode("theme");
  setUserPaused(false);
  setBarHidden(false);
  upNext = null;
  if (currentTrack()?.gameId === track.gameId) {
    setWanted(null);
    if (!playing() && reasons.size === 0) { play(); }
    return;
  }
  want(track);
}

// ── The visible list as a queue ──────────────────────────────────────────────

/** Play a game's theme and then keep going down the Browse list the user is
 *  looking at. The queue is that list as it stands - filters, sort and all. */
export function playFromList(game: Pick<Game, "id" | "title" | "torrent_source" | "thumbnail_key" | "music_file">) {
  if (game.id == null) { return; }
  const track = trackOf(game, game.id);
  setMode("list");
  setUserPaused(false);
  setBarHidden(false);
  upNext = null;
  listCursor = track.gameId;
  if (currentTrack()?.gameId === track.gameId) {
    setWanted(null);
    if (!playing() && reasons.size === 0) { play(); }
    return;
  }
  want(track);
}

/** The next row of the live list that is worth playing, walking in `dir`.
 *  Never wraps: the end of the list is the end of playback. */
async function listNeighbour(gameId: number, dir: 1 | -1): Promise<Track | null> {
  let list = games();
  let index = list.findIndex((g) => g.id === gameId);
  // The list changed under the player (a filter, a search): going forward
  // starts over from the top, going back has nothing to go back to.
  if (index === -1) {
    if (dir === -1) { return null; }
    index = -1;
  }
  let fetched = false;
  for (let i = index + dir; ; i += dir) {
    if (i < 0) { return null; }
    if (i >= list.length) {
      if (fetched || !hasMore()) { return null; }
      fetched = true;
      await fetchMoreGames();
      list = games();
      if (i >= list.length) { return null; }
    }
    const g = list[i];
    if (g.id != null && playableHint(g)) { return trackOf(g, g.id); }
  }
}

// ── Shuffle ──────────────────────────────────────────────────────────────────

async function refill() {
  if (candidates.length >= CANDIDATE_LOW_WATER) { return; }
  refilling ??= musicShuffleCandidates(CANDIDATE_BATCH)
    .then((batch) => {
      // The picks the walk already knows are hopeless: a probe that found no
      // music, and everything it has tried since the last track that played.
      // `music_shuffle_candidates` orders by RANDOM and knows neither.
      const seen = new Set<number>([
        ...candidates.map((c) => c.id),
        ...history.slice(-10).map((t) => t.gameId),
        ...musicNone(),
        ...triedThisWalk,
        currentTrack()?.gameId ?? -1,
      ]);
      for (const c of batch ?? []) {
        if (!seen.has(c.id) && playable(c.music_file)) { candidates.push(c); seen.add(c.id); }
      }
    })
    .catch(() => {})
    .finally(() => { refilling = null; });
  await refilling;
}

/** Take the next entry of the running queue as the wanted track. */
async function advance() {
  // Offline nothing can be fetched, and the backend answers every pick with
  // an instant "none" - walking on would spin through the whole catalogue in
  // one tick. A track already on disk still plays: that goes through
  // playTheme/playFromList, not through the walk.
  if (isOffline()) { return; }
  if (mode() === "list") {
    if (listCursor == null) { return; }
    const ahead = await listNeighbour(listCursor, 1);
    // The end of the list is not an error: playback stops and the bar keeps
    // showing the last track.
    if (ahead && mode() === "list") { want(ahead); }
    return;
  }
  if (mode() !== "shuffle") { return; }
  if (candidates.length === 0) { await refill(); }
  if (mode() !== "shuffle") { return; }
  const c = candidates.shift();
  if (!c) { return; }
  want(trackOfCandidate(c));
  void refill();
}

/** Exactly one track ahead: enough to hide the swarm's latency between
 *  songs, not enough to compete with the videos for peers. */
async function prefetchNext() {
  if (!autoAdvances() || upNext) { return; }
  if (mode() === "list") {
    const cursor = currentTrack()?.gameId ?? listCursor;
    if (cursor == null) { return; }
    const ahead = await listNeighbour(cursor, 1);
    if (!ahead || mode() !== "list" || upNext) { return; }
    upNext = ahead;
    void requestTheme(ahead.gameId);
    return;
  }
  if (candidates.length === 0) { await refill(); }
  const c = candidates[0];
  if (!c || mode() !== "shuffle" || upNext) { return; }
  upNext = trackOfCandidate(c);
  void requestTheme(c.id);
}

/** Start the collection-wide shuffle. Only ever called from a click. */
export async function startShuffle() {
  // Nothing to shuffle through offline, and the walk would only spin.
  if (isOffline()) { return; }
  // A pick is already on its way: a second click must not stack a second
  // fetch behind it - the bar is showing that one load.
  if (wanted()) { return; }
  setMode("shuffle");
  setUserPaused(false);
  setBarHidden(false);
  autoSkips = 0;
  await advance();
}

export async function next() {
  // Only theme mode has no queue to move along; there "next" starts one.
  if (!autoAdvances()) {
    await startShuffle();
    return;
  }
  const ahead = upNext;
  upNext = null;
  if (ahead) {
    if (mode() === "shuffle" && candidates[0]?.id === ahead.gameId) { candidates.shift(); }
    want(ahead);
    return;
  }
  await advance();
}

export function prev() {
  const previous = history.pop();
  if (!previous) { return; }
  const cur = currentTrack();
  // The current track goes in front of the queue rather than back into
  // history, so "prev" then "next" returns to it.
  if (cur && mode() === "shuffle") {
    upNext = cur;
  }
  steppingBackTo = previous.gameId;
  want(previous);
}

export function togglePlay() {
  if (playing()) {
    setUserPaused(true);
    pause();
    return;
  }
  // The click is the listener's word: it overrides whatever paused the music.
  setUserPaused(false);
  reasons.clear();
  setPauseReasons([]);
  if (currentTrack()) { play(); }
}

/** Put the bar away without giving up the track. A user gesture, so it counts
 *  as a pause of the listener's own - nothing may resume it behind a hidden
 *  bar, least of all a game exiting. */
export function hidePlayer() {
  setBarHidden(true);
  setUserPaused(true);
  pause();
}

/** Bring the bar back, still paused: showing is not playing. */
export function showPlayer() {
  setBarHidden(false);
}

export function stop() {
  clearSkipTimer();
  setBarHidden(false);
  pause();
  loadSeq++;
  const ahead = upNext;
  upNext = null;
  const abandoned = wanted();
  setWanted(null);
  // Both the pick being waited on and the one ahead of it lose their reason to
  // exist. Cancelling alone is not enough: a job still in the queue would
  // start the moment a slot frees.
  for (const track of [abandoned, ahead]) {
    if (track && getMusicState(track.gameId)?.phase !== "ready") { abandonFetch(track.gameId); }
  }
  setCurrentTrack(null);
  port?.setSrc(null);
  listCursor = null;
  autoSkips = 0;
  triedThisWalk.clear();
  steppingBackTo = null;
  setMode("theme");
  setUserPaused(false);
  reasons.clear();
  setPauseReasons([]);
}

// ── Arbitration ──────────────────────────────────────────────────────────────

/** Something else needs the speakers. The reason is recorded whether or not
 *  anything is playing right now - it describes a STATE that lasts (a preview
 *  with sound, a running emulator), and music started while it lasts has to
 *  respect it too. Recording it only when the music was already audible made
 *  the arbitration edge-triggered: pressing ▶ on a theme row over a running
 *  game, or over an unmuted trailer, played straight through it.
 *
 *  Withdrawing a reason that changed nothing still changes nothing: `resumeFrom`
 *  only starts playback when there is a track and the listener has not paused
 *  it themselves, so a game launched into silence starts nothing on exit. */
export function pauseFor(reason: PauseReason) {
  if (!reasons.has(reason)) {
    reasons.add(reason);
    setPauseReasons([...reasons]);
  }
  pause();
}

export function resumeFrom(reason: PauseReason) {
  if (!reasons.delete(reason)) { return; }
  setPauseReasons([...reasons]);
  if (reasons.size === 0 && !userPaused() && currentTrack() && !playing()) { play(); }
}

/** The games currently running. The backend starts one emulator process per
 *  launch and reports each exit with its own id, so two games open at once are
 *  two reasons to stay quiet - and the first exit must not undo the second. */
const runningGames = new Set<number>();

export function pauseForGame(id: number) {
  runningGames.add(id);
  pauseFor("game");
}

/** Withdraw one game's claim. Without an id (an exit event that carries none)
 *  the set cannot be kept honest, so it is dropped whole - the old behaviour. */
export function resumeFromGame(id?: number | null) {
  if (id == null) { runningGames.clear(); } else { runningGames.delete(id); }
  if (runningGames.size > 0) { return; }
  resumeFrom("game");
}

/** Registered once at app mount. */
export async function initMusic() {
  autoplay.ensureLoaded();
  continuous.ensureLoaded();
  volume.ensureLoaded();
  // One readdir, and every Browse row can say whether its theme is on disk
  // without a probe of its own. Also re-read when the app goes offline, where
  // the cache is the only thing that can still play.
  void refreshMusicIndex();
  createRoot(() => {
    createEffect(on(networkMode, () => { void refreshMusicIndex(); }, { defer: true }));
  });
  await listen<{ id: number }>("game-exited", (event) => resumeFromGame(event.payload?.id));
}
