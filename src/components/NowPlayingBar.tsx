import { createEffect, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  attachAudio, currentTrack, musicPlaying, musicVolume, setMusicVolume, pauseReasons, wantedTrack, musicMode,
  musicContinuous, setMusicContinuous,
  togglePlay, next, prev, hidePlayer, playerHidden, startShuffle, setOpenGameRequest, getMusicState, musicJobs, type AudioPort,
} from "../stores/music";
import { thumbnailCandidates } from "../stores/thumbnails";
import { IconAutoplay, IconShuffle, IconSoundOff, IconSoundOn } from "./icons";

/** `m:ss`, or `--:--` while the element has no duration to report (a stream
 *  still loading its metadata answers NaN, a live one Infinity). */
function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) { return "--:--"; }
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

/** The player, as a bottom row of the app shell.
 *
 *  Owns the one `<audio>` element - the store drives it through `AudioPort`
 *  and never touches the DOM - and the element exists for the app's whole
 *  lifetime, while the visible bar appears only once a track is loaded. Not a
 *  floating pill: as the last flex child of `#root` it needs no fixed
 *  positioning, and the detail panel steps aside via `--player-h`. */
export function NowPlayingBar() {
  let audioRef: HTMLAudioElement | undefined;
  /** The seek bar reads the element directly rather than going through
   *  `AudioPort`: position is a property of the playing element, not of the
   *  store's queue, and the port stays as small as the store needs it. */
  const [position, setPosition] = createSignal(0);
  const [duration, setDuration] = createSignal(NaN);
  const durationKnown = () => Number.isFinite(duration()) && duration() > 0;

  onMount(() => {
    const el = audioRef;
    if (!el) { return; }
    const onTime = () => setPosition(el.currentTime);
    const onMeta = () => setDuration(el.duration);
    el.addEventListener("timeupdate", onTime);
    el.addEventListener("durationchange", onMeta);
    el.addEventListener("loadedmetadata", onMeta);
    onCleanup(() => {
      el.removeEventListener("timeupdate", onTime);
      el.removeEventListener("durationchange", onMeta);
      el.removeEventListener("loadedmetadata", onMeta);
    });
    const port: AudioPort = {
      setSrc(url) {
        if (url) { el.src = url; el.load(); } else { el.removeAttribute("src"); el.load(); }
      },
      play: () => {
        const p = el.play();
        return p && typeof p.then === "function" ? p : Promise.resolve();
      },
      pause: () => el.pause(),
      setVolume(v) { el.volume = v; },
      onEnded(cb) { el.onended = cb; },
    };
    attachAudio(port);
    onCleanup(() => attachAudio(null));
  });

  // The preference lands after the element is attached, and can change from
  // the slider on another instance.
  createEffect(() => {
    if (audioRef) { audioRef.volume = musicVolume(); }
  });

  /** What the bar shows, if anything: the × only hides it, so a loaded track
   *  is not enough - the layout has to give the room back too.
   *
   *  A track still being fetched counts. The first shuffle pick takes as long
   *  as the swarm takes, and a bar that appears only afterwards leaves the
   *  click looking ignored for a minute. */
  const barTrack = () => currentTrack() ?? wantedTrack();
  const visibleTrack = () => (playerHidden() ? null : barTrack());
  /** Nothing is loaded yet: the transport has nothing to act on. */
  const loadingOnly = () => currentTrack() == null;

  // The panel and the backdrop read this to leave room for the bar.
  createEffect(() => {
    document.body.classList.toggle("has-player", visibleTrack() != null);
  });
  onCleanup(() => document.body.classList.remove("has-player"));

  // Cover with the same tier walk as the cards: poster first, preview second.
  const candidates = () => {
    const t = barTrack();
    return t ? thumbnailCandidates(t.collection, t.thumbnailKey) : [];
  };
  const [coverIdx, setCoverIdx] = createSignal(0);
  createEffect(on(() => barTrack()?.gameId, () => {
    setCoverIdx(0);
    setPosition(0);
    setDuration(NaN);
  }, { defer: true }));
  const coverSrc = () => {
    const list = candidates();
    const path = list[coverIdx()];
    return path ? convertFileSrc(path) : null;
  };

  /** What the bar is waiting on, if anything - the next shuffle pick still
   *  coming over the torrent, say. */
  const pending = () => {
    const w = wantedTrack();
    if (!w) { return null; }
    musicJobs();
    const state = getMusicState(w.gameId);
    // The title line already names this track while nothing else is loaded,
    // so the line below it does not repeat the name.
    const what = loadingOnly() ? "a theme" : w.title;
    if (!state) { return `Loading ${what}…`; }
    if (state.phase === "fetching") { return `Loading ${what} ${Math.round(state.progress * 100)}%`; }
    if (state.phase === "queued") { return `${loadingOnly() ? "Theme" : w.title} queued…`; }
    return `Looking for ${what}…`;
  };

  const modeNote = () => {
    if (musicMode() === "shuffle") { return "Shuffle · theme"; }
    if (musicMode() === "list") { return "Browse list · theme"; }
    return "Theme";
  };

  const pausedNote = () => {
    const r = pauseReasons();
    if (r.includes("game")) { return "Paused while the game runs"; }
    if (r.includes("video")) { return "Paused for the preview"; }
    return null;
  };

  return (
    <>
      <audio ref={audioRef} preload="auto" />
      <Show when={visibleTrack()}>
        {(track) => (
          <div class="now-playing-bar" role="region" aria-label="Now playing">
            <button
              class="player-cover"
              title={`Open ${track().title}`}
              onClick={() => setOpenGameRequest(track().gameId)}
            >
              <Show when={coverSrc()} fallback={<span class="player-cover-placeholder">♪</span>}>
                <img
                  src={coverSrc()!}
                  alt=""
                  onError={() => setCoverIdx((i) => i + 1)}
                />
              </Show>
            </button>
            <div class="player-meta">
              <div class="player-title">{track().title}</div>
              <div class="player-sub">
                <Show when={pending()} fallback={
                  <Show when={pausedNote()} fallback={modeNote()}>
                    {pausedNote()}
                  </Show>
                }>
                  {pending()}
                </Show>
              </div>
            </div>
            <div class="player-seek">
              <span class="player-time">{formatTime(position())}</span>
              <input
                type="range"
                class="player-seek-range"
                min="0"
                max={durationKnown() ? duration() : 1}
                step="0.1"
                value={position()}
                disabled={!durationKnown()}
                aria-label="Seek"
                onInput={(e) => {
                  const t = Number(e.currentTarget.value);
                  // Moving the thumb moves the element in the same tick, so
                  // the next `timeupdate` already reports the new position.
                  setPosition(t);
                  if (audioRef) { audioRef.currentTime = t; }
                }}
              />
              <span class="player-time">{formatTime(duration())}</span>
            </div>
            <div class="player-controls">
              {/* Theme mode is one game's single track and has no queue, so
                  the skip buttons have nothing to skip to - they give way to
                  the one move that does make sense there. */}
              {/* Until a track is loaded the transport has nothing to act on:
                  the buttons stay in place (the bar must not resize when the
                  bytes land) but do nothing. */}
              <Show when={musicMode() !== "theme"}>
                <button class="player-btn" title="Previous game" aria-label="Previous game" disabled={loadingOnly()} onClick={() => prev()}>⏮</button>
              </Show>
              <button
                class="player-btn player-btn-main"
                title={musicPlaying() ? "Pause" : "Play"}
                aria-label={musicPlaying() ? "Pause" : "Play"}
                disabled={loadingOnly()}
                onClick={() => togglePlay()}
              >{musicPlaying() ? "⏸" : "▶"}</button>
              <Show when={musicMode() !== "theme"}>
                <button
                  class="player-btn"
                  title="Next game"
                  aria-label="Next game"
                  disabled={loadingOnly()}
                  onClick={() => { void next(); }}
                >⏭</button>
              </Show>
              <Show when={musicMode() === "theme"}>
                <button
                  class="player-btn player-shuffle"
                  title="Shuffle all themes"
                  aria-label="Shuffle all themes"
                  // A pick already on its way owns the bar: `startShuffle`
                  // refuses to stack a second fetch behind it, so an enabled
                  // button here is one that does nothing when clicked.
                  disabled={loadingOnly() || wantedTrack() != null}
                  onClick={() => { void startShuffle(); }}
                ><IconShuffle /></button>
              </Show>
              <button
                class={`player-btn player-continuous${musicContinuous() ? " is-on" : ""}`}
                title={musicContinuous() ? "Continuous playback on" : "Continuous playback off"}
                aria-label="Continuous playback"
                aria-pressed={musicContinuous()}
                onClick={() => { void setMusicContinuous(!musicContinuous()); }}
              ><IconAutoplay /></button>
            </div>
            <label class="player-volume" title="Volume">
              <span aria-hidden="true">{musicVolume() === 0 ? <IconSoundOff /> : <IconSoundOn />}</span>
              <input
                type="range"
                min="0"
                max="1"
                step="0.02"
                value={musicVolume()}
                aria-label="Volume"
                onInput={(e) => { void setMusicVolume(Number(e.currentTarget.value)); }}
              />
            </label>
            <button class="player-btn player-close" title="Hide player" aria-label="Hide player" onClick={() => hidePlayer()}>×</button>
          </div>
        )}
      </Show>
    </>
  );
}
