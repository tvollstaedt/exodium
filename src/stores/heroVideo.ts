import { createSignal, untrack } from "solid-js";
import { describePlayError, pauseFor, resumeFrom } from "./music";

/** The one hero preview: a single `<video>` element the panel attaches for
 *  its lifetime, driven from here. Every transition cancels the previous one
 *  (timer, play sequence, source), so a start still pending for one game can
 *  neither play on after the panel moved on nor write into the next game's
 *  state - and a detached element is never left with a source it could
 *  start on its own. The "video" pause reason (§14) is taken and given back
 *  here only. */

/** What the controller needs from a `<video>`; tests hand in a fake. */
export interface VideoPort {
  setSrc(url: string | null): void;
  play(): Promise<void> | undefined;
  pause(): void;
  setMuted(muted: boolean): void;
  isMuted(): boolean;
  setVolume(volume: number): void;
  /** Re-run the element's load: the one answer to a transient media error. */
  reload(): void;
  seekStart(): void;
  onPlay(cb: () => void): void;
  onPause(cb: () => void): void;
  onEnded(cb: () => void): void;
  onError(cb: (message: string) => void): void;
}

export type HeroPhase = "idle" | "loading" | "playing" | "paused" | "ended" | "failed";

const [gameId, setGameId] = createSignal<number | null>(null);
const [phase, setPhase] = createSignal<HeroPhase>("idle");
/** Why the last start failed; null while nothing is wrong. */
const [error, setError] = createSignal<string | null>(null);
export { gameId as heroGameId, phase as heroPhase, error as heroError };

/** True while the hero shows frames for THIS game. */
export const heroPlayingFor = (id: number | null | undefined): boolean =>
  id != null && gameId() === id && phase() === "playing";

let port: VideoPort | null = null;
let src: string | null = null;
let startTimer: ReturnType<typeof setTimeout> | undefined;
/** Bumped by every transition; a `play()` outcome from an earlier one is
 *  dropped. */
let seq = 0;
/** Whether this controller currently holds the "video" pause reason. Kept
 *  here so give-backs are idempotent and never undo a hold nobody took. */
let holdingSpeakers = false;
/** The lightbox is playing the trailer with sound: the hero is paused, but
 *  the speakers stay claimed until it closes. */
let lightboxHold = false;
let lightboxOpen = false;
/** The preview's sound eases in over the music's fade-out instead of
 *  cutting across it; one ramp at a time, a new start replaces it. */
let fadeTimer: ReturnType<typeof setInterval> | undefined;
const FADE_MS = 600;
/** The game whose media error already got its one silent retry (a valid
 *  file failed with MediaError 4 exactly once on the NVIDIA path and played
 *  on replay). A source that fails again gets named. */
let errorRetriedFor: number | null = null;
const ERROR_RETRY_MS = 800;

function claimSpeakers() {
  if (holdingSpeakers) { return; }
  holdingSpeakers = true;
  pauseFor("video");
}

function releaseSpeakers() {
  if (!holdingSpeakers || lightboxHold) { return; }
  holdingSpeakers = false;
  resumeFrom("video");
}

function clearTimer() {
  if (startTimer) { clearTimeout(startTimer); startTimer = undefined; }
}

function clearFade() {
  if (fadeTimer) { clearInterval(fadeTimer); fadeTimer = undefined; }
}

function fadeIn() {
  if (!port) { return; }
  clearFade();
  port.setVolume(0);
  const t0 = Date.now();
  fadeTimer = setInterval(() => {
    const k = Math.min(1, (Date.now() - t0) / FADE_MS);
    port?.setVolume(k);
    if (k === 1) { clearFade(); }
  }, 40);
}

/** Start the element, honouring `muted`; an unmuted start the engine refuses
 *  (no gesture) is retried muted, and the speakers go back with it. The
 *  outcome is only applied while `mySeq` is still the current transition. */
function start(mySeq: number, muted: boolean) {
  if (!port) { return; }
  port.setMuted(muted);
  if (muted) { releaseSpeakers(); } else { claimSpeakers(); fadeIn(); }
  let started: Promise<void> | undefined;
  try {
    started = port.play();
  } catch (e) {
    fail(mySeq, e);
    return;
  }
  if (!started || typeof started.then !== "function") {
    // An engine without play promises reports nothing; the `play` event
    // still lands and confirms.
    setPhase("playing");
    return;
  }
  started.catch((e) => {
    if (mySeq !== seq) { return; }
    if (!muted) {
      // Autoplay with sound needs a user gesture the webview may not have
      // seen. A silent preview beats no preview - but the preference is not
      // written back: the user did not choose it.
      start(mySeq, true);
      return;
    }
    fail(mySeq, e);
  });
}

function fail(mySeq: number, e: unknown) {
  if (mySeq !== seq) { return; }
  const why = describePlayError(e);
  if (why == null) { return; }
  setPhase("failed");
  setError(why);
  releaseSpeakers();
}

/** Show a game's preview: after `delayMs` of cover, start it with the given
 *  mute preference. Cancels whatever was pending or playing before. */
export function showPreview(id: number, url: string, opts: { muted: boolean; delayMs: number }) {
  untrack(() => {
    // Already on it: a re-run of the caller's effect must not restart a
    // preview that is playing or about to.
    if (gameId() === id && src === url && (phase() === "playing" || phase() === "loading")) { return; }
    clearTimer();
    clearFade();
    const mySeq = ++seq;
    if (phase() !== "idle" && (gameId() !== id || src !== url)) { port?.pause(); }
    if (src !== url) {
      port?.setSrc(url);
      src = url;
    }
    setGameId(id);
    setPhase("loading");
    setError(null);
    // A preview about to play with sound claims the speakers NOW, not at its
    // first frame: the theme would start within the cover beat otherwise and
    // be cut off two seconds later.
    if (opts.muted) { releaseSpeakers(); } else { claimSpeakers(); }
    startTimer = setTimeout(() => {
      startTimer = undefined;
      if (mySeq !== seq) { return; }
      port?.seekStart();
      start(mySeq, opts.muted);
    }, opts.delayMs);
  });
}

/** Nothing to show: stop, unload, give the speakers back. The element keeps
 *  no source, so it cannot start later on its own. */
export function clearPreview() {
  untrack(() => {
    clearTimer();
    clearFade();
    seq++;
    port?.pause();
    if (src != null) {
      port?.setSrc(null);
      src = null;
    }
    setGameId(null);
    setPhase("idle");
    setError(null);
    releaseSpeakers();
  });
}

/** Something else took the screen and the speakers - a game launched. The
 *  preview stops and stays stopped: a trailer resuming by itself when the
 *  emulator quits is not what anyone asked for. The replay button brings it
 *  back. */
export function stopForGame() {
  untrack(() => {
    if (phase() === "idle") { return; }
    clearTimer();
    clearFade();
    seq++;
    port?.pause();
    if (phase() !== "ended" && phase() !== "failed") { setPhase("paused"); }
    releaseSpeakers();
  });
}

/** The replay button: a gesture, so it starts with the real preference. */
export function replayPreview(muted: boolean) {
  untrack(() => {
    if (!port || gameId() == null) { return; }
    clearTimer();
    const mySeq = ++seq;
    setError(null);
    if (phase() === "ended") { port.seekStart(); }
    setPhase("loading");
    start(mySeq, muted);
  });
}

/** The mute toggle. Silent, the preview no longer needs the speakers; with
 *  sound it does - and the click is the gesture a fallback-muted start
 *  lacked, so a paused preview restarts. */
export function setPreviewMutedNow(muted: boolean) {
  untrack(() => {
    if (!port || gameId() == null) { return; }
    port.setMuted(muted);
    if (muted) {
      releaseSpeakers();
      return;
    }
    if (phase() === "playing") {
      claimSpeakers();
      fadeIn();
    } else if (phase() === "paused" || phase() === "failed") {
      replayPreview(false);
    }
  });
}

/** The lightbox opens over the hero: the hero pauses. While the lightbox
 *  plays the trailer with sound, it inherits the hero's claim. */
export function setLightbox(open: boolean, holdsAudio: boolean) {
  untrack(() => {
    const wasOpen = lightboxOpen;
    lightboxOpen = open;
    if (open) {
      if (holdsAudio) { claimSpeakers(); }
      lightboxHold = holdsAudio;
      if (phase() === "playing" || phase() === "loading") {
        clearTimer();
        seq++;
        port?.pause();
        if (phase() === "loading") { setPhase("paused"); }
      }
      if (!holdsAudio) { releaseSpeakers(); }
      return;
    }
    // Closed, and only a real close acts: the caller's effect also re-runs
    // when the hold's inputs move while the lightbox is shut.
    if (!wasOpen) { return; }
    lightboxHold = false;
    if (phase() !== "playing") { releaseSpeakers(); }
  });
}

/** The panel's element comes and goes with the panel; detaching stops it. */
export function attachVideo(next: VideoPort | null) {
  if (port && port !== next) {
    // Whatever was on the way out must not play on detached.
    clearTimer();
    clearFade();
    seq++;
    port.pause();
    port.setSrc(null);
    src = null;
    setGameId(null);
    setPhase("idle");
    setError(null);
    releaseSpeakers();
  }
  port = next;
  if (!port) { return; }
  port.onPlay(() => {
    if (phase() === "idle") { return; }
    setPhase("playing");
    setError(null);
    // Frames are flowing, so the transient failure is spent: the next one
    // for this game gets its reload again.
    errorRetriedFor = null;
    if (!port?.isMuted()) { claimSpeakers(); }
  });
  port.onPause(() => {
    // Only a pause of something that played: a source change fires one too.
    if (phase() !== "playing") { return; }
    setPhase("paused");
    releaseSpeakers();
  });
  port.onEnded(() => {
    if (phase() === "idle") { return; }
    setPhase("ended");
    releaseSpeakers();
  });
  port.onError((message) => {
    if (phase() === "idle") { return; }
    const id = gameId();
    if (id != null && errorRetriedFor !== id) {
      errorRetriedFor = id;
      clearTimer();
      clearFade();
      const mySeq = ++seq;
      const muted = port?.isMuted() ?? true;
      startTimer = setTimeout(() => {
        startTimer = undefined;
        if (mySeq !== seq || !port) { return; }
        port.reload();
        start(mySeq, muted);
      }, ERROR_RETRY_MS);
      return;
    }
    setPhase("failed");
    setError(message);
    releaseSpeakers();
  });
}

/** Test hook: everything back to the initial state. */
export function resetHeroVideoForTests() {
  clearTimer();
  clearFade();
  seq++;
  port = null;
  src = null;
  errorRetriedFor = null;
  holdingSpeakers = false;
  lightboxHold = false;
  lightboxOpen = false;
  setGameId(null);
  setPhase("idle");
  setError(null);
}
