import { createSignal, createEffect, on, untrack, Show, For, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AutoProgress } from "./ProgressBar";
import { Lightbox } from "./Lightbox";
import { ManualViewer } from "./ManualViewer";
import { GameActionsMenu } from "./GameActionsMenu";
import { FieldIcon, IconPlay, IconSoundOn, IconSoundOff, IconZoom, type FieldIconName } from "./icons";
import { ConfirmDialog } from "./ConfirmDialog";
import { Button } from "./Button";
import type { Game, GameMetadata } from "../api/tauri";
import { launchGame, stopGame, gameEngineInfo, gamePrintingUnavailable, scummvmEngineInfo, scummvmVariants, setScummvmOptions, win9xMultiplayerInfo, dismissWin9xNetworkPrompt, enableWin9xNetwork, mediaUrl } from "../api/tauri";
import type { GameEngineInfo, ScummVmEngineInfo, ScummVmVariants } from "../api/tauri";
import { createWin9xStatus } from "./win9xStatus";
import { formatBytes, parseLangEntries, langBadgeClass, performUninstall, performReset } from "../util";
import { launchNote, emulatorName as describeEmulator, emulatorPackId, isScummVm, svmVariantLabel, type PanelNote } from "../launchNotes";
import { showToast } from "../stores/toasts";
import { bestThumbnailPath, thumbnailCandidates } from "../stores/thumbnails";
import { downloads, startGameDownload, getDownloadState, cancelGameDownload, watchExtrasIfPending } from "../stores/downloads";
import { loadGameMetadata } from "../stores/metadata";
import { isOffline } from "../stores/network";
import { loadVariants } from "../stores/variants";
import { toggleFavorite, updateGameFavorited } from "../stores/games";
import { videos, requestVideo, releaseVideo, setForegroundVideo, getVideoState, videoPlaybackUnsupported, PHASE_QUEUED, PHASE_PROBING, isVideoTimeout } from "../stores/videos";
import { ensureDismissedNotesLoaded, isNoteDismissed, dismissedNotesLoaded, dismissNote } from "../stores/notes";
import { packsByCollection, activeJobs, installedPacks, startContentPackInstall } from "../stores/contentPacks";
import { ensurePreviewMutedLoaded, previewMuted, setPreviewMuted } from "../stores/playback";
import { attachCanvasPainter, ensureVideoMirrorKnown, needsCanvasVideo } from "../videoCanvas";
import { musicJobs, getMusicState, requestTheme, playTheme, withdrawAutoTheme, pauseForGame, resumeFromGame, runningGameIds, togglePlay, currentTrack, wantedTrack, musicPlaying, musicAutoplay, musicUserPaused, playerHidden, ensureMusicAutoplayLoaded, musicUnsupported, MUSIC_QUEUED, isTimeoutError } from "../stores/music";
import { attachVideo, showPreview, clearPreview, replayPreview, setPreviewMutedNow, setLightbox, heroPlayingFor, heroGameId, heroError, type VideoPort } from "../stores/heroVideo";

interface Props {
  game: Game | null;
  onClose: () => void;
  onDownloadStart?: (gameId: number) => void;
}

/** How long the cover keeps the hero to itself before the preview fades in. */
const VIDEO_START_DELAY_MS = 2000;
/** Matches the slot's transition in main.css. */
const NOTE_CLOSE_MS = 240;

/** Fallback for the slide-in's `animationend` (main.css: 260ms). Only reached
 *  when the event cannot arrive - prefers-reduced-motion, a hidden window -
 *  so it may sit a little past the animation without costing anything. */
const SETTLE_FALLBACK_MS = 350;

/** A credit line under the title: pictogram, then the value. The pictogram
 *  replaces the label, so `title` carries it for anyone who needs it spelled
 *  out (tooltip and screen readers both). */
const Credit = (props: { icon: FieldIconName; value: string | number | null | undefined; title: string }) => (
  <Show when={props.value != null && props.value !== ""}>
    <div class="game-detail-credit" title={props.title}>
      <FieldIcon name={props.icon} />
      <span>{props.value}</span>
    </div>
  </Show>
);

/** A categorical fact as a tinted chip. `kind` picks the hue, so the same
 *  category always reads the same colour across every game. */
const Tag = (props: {
  kind: "genre" | "platform" | "mode" | "emulator";
  value: string | null | undefined;
  title: string;
}) => (
  <Show when={props.value}>
    <span class={`game-detail-tag is-${props.kind}`} title={props.title}>{props.value}</span>
  </Show>
);

/** One metadata row: pictogram, label, value. Nothing when absent. */
const Field = (props: {
  icon: FieldIconName;
  label: string;
  value: string | number | null | undefined;
  valueClass?: string;
}) => (
  <Show when={props.value != null && props.value !== ""}>
    <div class="game-detail-field">
      <FieldIcon name={props.icon} />
      <span class="game-detail-field-label">{props.label}</span>
      <span class={props.valueClass}>{props.value}</span>
    </div>
  </Show>
);

const isWindows = typeof navigator !== "undefined"
  && /Win/i.test(navigator.platform || navigator.userAgent || "");

/** Language codes seen in the eXoDOS catalogue, spelled out for prose like
 *  "no German description". Unknown codes fall back to the raw code. */
const LANGUAGE_NAMES: Record<string, string> = {
  EN: "English", DE: "German", PL: "Polish", ES: "Spanish",
  FR: "French", IT: "Italian", RU: "Russian", CZ: "Czech",
  NL: "Dutch", PT: "Portuguese", SV: "Swedish", HU: "Hungarian",
};
const languageName = (code: string | null | undefined) =>
  (code && LANGUAGE_NAMES[code]) || code || "";

export function GameDetailPanel(props: Props) {
  const [variants, setVariants] = createSignal<Game[]>([]);
  const [status, setStatus] = createSignal("");
  const [imgError, setImgError] = createSignal(false);
  const [metadata, setMetadata] = createSignal<GameMetadata | null>(null);
  const [metadataLoading, setMetadataLoading] = createSignal(false);
  const [brokenImages, setBrokenImages] = createSignal(new Set<number>());
  const [lightboxOpen, setLightboxOpen] = createSignal(false);
  const [lightboxStart, setLightboxStart] = createSignal(0);
  /** The lightbox lists the preview video as entry 0 when there is one, so an
   *  index into the screenshot array has to be shifted to match. */
  const lightboxIndexOfImage = (imageIndex: number) => (videoSrc() ? imageIndex + 1 : imageIndex);
  const [manualOpen, setManualOpen] = createSignal(false);
  // Preview video. Fetching starts on open (see the effect below); this is only
  // the playback state - the video takes over the hero while it plays and hands
  // the cover back when it ends.
  /** The hero shows frames for the selected row (stores/heroVideo owns the element). */
  const videoPlaying = () => heroPlayingFor(selected()?.id);
  // 13 eXoDOS titles print as their core feature; Staging has no printer
  // emulation yet, so those get a heads-up note. The backend owns the whole
  // answer (conf + engine selection), so no platform logic lives here.
  const [printingUnavailable, setPrintingUnavailable] = createSignal(false);
  /** Game id awaiting the multiplayer question, or null. */
  const [netPromptFor, setNetPromptFor] = createSignal<number | null>(null);
  /** Set by the dialog's confirm path, which starts its own launch - so the
   *  close handler (which fires for both answers) does not start a second. */
  let netPromptAccepted = false;
  /** ECE or Staging, from the backend's `resolve_engine` (§9) - never
   *  derived here. */
  const [engineInfo, setEngineInfo] = createSignal<GameEngineInfo | null>(null);
  /** Null until the backend answers. Guessing "Staging" in the meantime made
   *  every ECE game on Windows flash the wrong engine and the wrong note. */
  const runsUnderEce = () => engineInfo()?.uses_ece ?? isWindows;
  const [svmEngine, setSvmEngine] = createSignal<ScummVmEngineInfo | null>(null);
  /** The platform variants inside an installed ScummVM game; null until
   *  unpacked (the tree lives in the zip). */
  const [svmVariants, setSvmVariants] = createSignal<ScummVmVariants | null>(null);
  const loadSvmVariants = (id: number) => {
    scummvmVariants(id)
      .then((v) => { if (props.game?.id === id) { setSvmVariants(v); } })
      .catch(() => {});
  };
  const chooseSvmVariant = async (name: string) => {
    const id = props.game?.id;
    const sel = svmVariants()?.selected;
    if (id == null || !sel || sel.variant === name) { return; }
    try {
      await setScummvmOptions(id, { variant: name, sub: null, sound: sel.sound, subtitles: sel.subtitles, aspect: sel.aspect });
      loadSvmVariants(id);
    } catch (e) {
      showToast("Couldn't switch the version", "error", { detail: String(e) });
    }
  };
  const emulatorName = () => describeEmulator(selected() ?? props.game, svmEngine(), runsUnderEce());
  const packCollection = () => selected()?.torrent_source ?? props.game?.torrent_source ?? "eXoWin9x";
  /** The variant that picks the emulator pack (Win9x slug or ScummVM pin). */
  /** The content pack that could supply the missing Win9x emulator. */
  const emulatorPack = () => {
    const g = selected() ?? props.game;
    const packId = isScummVm(g) ? svmEngine()?.pack_id ?? null : emulatorPackId(g?.dosbox_variant);
    if (!packId) { return null; }
    return (packsByCollection()[packCollection()] ?? [])
      .find((p) => p.id === packId && p.available && !p.installed) ?? null;
  };
  const rawNote = (): PanelNote | null => {
    const pack = emulatorPack();
    return launchNote({
      game: selected() ?? props.game,
      isWindows,
      offline: isOffline(),
      installed: selectedInstalled(),
      downloading: selectedDownloading(),
      svmEngine: svmEngine(),
      svmNote: svmVariants()?.note ?? null,
      engineInfo: engineInfo(),
      win9xEngineMissing: win9x.engineMissing(),
      support: win9x.support(),
      mp: win9x.mp(),
      printingUnavailable: printingUnavailable(),
      videoUnsupported: videoPlaybackUnsupported(),
      emulatorPack: pack,
      packJob: pack ? (activeJobs()[`${packCollection()}:${pack.id}`] ?? null) : null,
      installPack: (p) => {
        startContentPackInstall(packCollection(), p.id, p.display_name).catch((e) => {
          showToast(`Couldn't start the ${p.display_name} download`, "error", { detail: String(e) });
        });
      },
    });
  };

  /** The note actually rendered. Dismissal is remembered per note KIND, so
   *  answering "tuned for ECE" once silences it on all ~2,000 ECE titles -
   *  per game it would be busywork rather than a setting. */
  const note = (): PanelNote | null => {
    const n = rawNote();
    if (!n || n.blocking) { return n; }
    return dismissedNotesLoaded() && !isNoteDismissed(n.key) ? n : null;
  };
  /** The note the slot renders. It outlives `note()` by the length of the
   *  closing transition so the panel body glides instead of jumping, and
   *  `noteOpen` drives the slot's height between 0fr and 1fr. */
  const [shownNote, setShownNote] = createSignal<PanelNote | null>(null);
  const [noteOpen, setNoteOpen] = createSignal(false);
  createEffect(() => {
    const n = note();
    if (n) {
      setShownNote(n);
      // Two frames: the slot has to be laid out closed before it opens, or
      // the browser has nothing to transition from.
      let frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setNoteOpen(true));
      });
      onCleanup(() => cancelAnimationFrame(frame));
      return;
    }
    setNoteOpen(false);
    const timer = window.setTimeout(() => setShownNote(null), NOTE_CLOSE_MS);
    onCleanup(() => clearTimeout(timer));
  });
  /** The one `<video>`: attached to the controller while the panel is open,
   *  stopped and unloaded when it closes - never left with a source. */
  let heroEl: HTMLVideoElement | undefined;
  const attachHero = (el: HTMLVideoElement) => {
    heroEl = el;
    const port: VideoPort = {
      setSrc(url) {
        if (url) { el.src = url; } else { el.removeAttribute("src"); }
        el.load();
      },
      play: () => el.play(),
      pause: () => el.pause(),
      setMuted(m) { el.muted = m; },
      isMuted: () => el.muted,
      setVolume(v) { el.volume = v; },
      reload: () => el.load(),
      seekStart() { try { el.currentTime = 0; } catch { /* no metadata yet */ } },
      onPlay(cb) { el.onplay = cb; },
      onPause(cb) { el.onpause = cb; },
      onEnded(cb) { el.onended = cb; },
      onError(cb) {
        el.onerror = () => {
          const err = el.error;
          cb(err ? `MediaError ${err.code}${err.message ? `: ${err.message}` : ""}` : "media error");
        };
      },
    };
    attachVideo(port);
    onCleanup(() => { attachVideo(null); heroEl = undefined; });
  };
  /** The ⋯ button must never open an empty menu; Playlist needs only an id. */
  const hasMoreActions = () => selected()?.id != null;

  const [moreMenu, setMoreMenu] = createSignal<{ x: number, y: number } | null>(null);
  const openMoreMenu = (e: MouseEvent & { currentTarget: HTMLElement }) => {
    const rect = e.currentTarget.getBoundingClientRect();
    // Anchored to the RIGHT edge: the button sits at the end of the bar, and a
    // left-anchored menu would hang off a narrow panel.
    setMoreMenu({ x: rect.right, y: rect.bottom + 4 });
  };

  const [launchingId, setLaunchingId] = createSignal<number | null>(null);
  const [uninstallingId, setUninstallingId] = createSignal<number | null>(null);
  const [resettingId, setResettingId] = createSignal<number | null>(null);
  // The panel describes exactly ONE row; the chip switcher picks it (§12).
  const [selectedId, setSelectedId] = createSignal<number | null>(null);
  let launchTimer: number | undefined;
  onCleanup(() => { if (launchTimer) { clearTimeout(launchTimer); } });

  const langEntries = () => props.game ? parseLangEntries(props.game) : [];
  const isMultiLang = () => langEntries().length > 1;


  // ── Selected variant ───────────────────────────────────────────────────
  // A single-language game is a group of one; there is no second path.
  const rows = (): Game[] => {
    const v = variants();
    if (v.length > 0) { return v; }
    return props.game ? [props.game] : [];
  };
  const selected = (): Game | null => {
    const list = rows();
    return list.find((r) => r.id === selectedId()) ?? list[0] ?? null;
  };
  /** Default pick when a game opens: whatever the user can act on right now -
   *  an installed version first (EN among equals), then one being fetched,
   *  then the English row. */
  const defaultVariant = (list: Game[]): Game | undefined =>
    list.find((v) => v.installed && v.language === "EN")
    ?? list.find((v) => v.installed)
    ?? list.find((v) => v.in_library)
    ?? list.find((v) => v.language === "EN")
    ?? list[0];

  const selectedDl = () => {
    const id = selected()?.id;
    return id != null ? downloads()[id] : undefined;
  };
  const selectedDownloading = () => selectedDl()?.downloading ?? false;
  const selectedInstalled = () =>
    (selected()?.installed ?? false) || (selectedDl()?.installed ?? false);

  /** LP rows carry almost no catalogue text of their own (developer, genre and
   *  friends live on the EN row), so every field falls back to the primary. */
  const field = <K extends keyof Game>(key: K): Game[K] | undefined => {
    const v = selected()?.[key];
    if (v !== null && v !== undefined && v !== "") { return v; }
    return props.game?.[key];
  };

  /** The description shown and whether it is the EN fallback (most LP rows
   *  have no text of their own). */
  const descriptionSource = () => {
    const sel = selected();
    const primary = props.game;
    if (sel?.description) {
      return { text: sel.description, notes: sel.notes, fallbackFrom: null as string | null };
    }
    if (primary?.description) {
      const differs = sel?.language && primary.language && sel.language !== primary.language;
      return {
        text: primary.description,
        notes: primary.notes,
        fallbackFrom: differs ? sel!.language : null,
      };
    }
    return null;
  };

  /** The manual to open for the selected variant: its own if the catalogue
   *  lists one, otherwise the English manual (the backend's metadata scan
   *  already falls back to the eXoDOS pack for assets). */
  const manualRow = (): Game | null => {
    const sel = selected();
    if (sel?.manual_path) { return sel; }
    return props.game?.manual_path ? props.game : null;
  };
  const manualIsFallback = () => {
    const sel = selected();
    const row = manualRow();
    return !!row && !!sel?.language && !!row.language && row.language !== sel.language;
  };
  // Launch/uninstall messages only; download progress lives in the action
  // bar and the chips.
  const currentStatus = () => status();


  // ── Settle gate ──────────────────────────────────────────────────────────
  // Heavy work (metadata scan, Win9x probes) waits for the 260 ms slide-in,
  // which stutters on a software renderer (§17). `animationend` is the
  // signal, the timeout the fallback. Layout-defining data stays immediate.
  const [panelSettled, setPanelSettled] = createSignal(false);
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  // Guarded on open/closed rather than on props.game itself: the prop is
  // replaced with a fresh object on every library refresh, and re-arming the
  // fallback on those would push it out indefinitely during a download.
  let panelOpen = false;
  const markSettled = () => {
    clearTimeout(settleTimer);
    setPanelSettled(true);
  };
  createEffect(() => {
    const open = props.game != null;
    if (open === panelOpen) { return; }
    panelOpen = open;
    clearTimeout(settleTimer);
    setPanelSettled(false);
    if (open) { settleTimer = setTimeout(markSettled, SETTLE_FALLBACK_MS); }
  });
  onCleanup(() => clearTimeout(settleTimer));
  const win9x = createWin9xStatus(() => props.game, panelSettled);
  // A finished pack install - or, on Windows, the support payload turning
  // ready - has to clear the "not found" note without the panel being
  // reopened; the Win9x status does the same re-probe.
  createEffect(() => {
    void installedPacks();
    void win9x.support()?.phase;
    if (!panelSettled()) { return; }
    const g = props.game;
    if (!isScummVm(g) || g?.id == null) { return; }
    const id = g.id;
    scummvmEngineInfo(id)
      .then((e) => { if (props.game?.id === id) { setSvmEngine(e); } })
      .catch(() => {});
  });

  // Reset only when the displayed game changes: library refreshes replace
  // the object for the same id.
  let lastGameId: number | null | undefined = undefined;
  let lastMetaKey: string | null = null;
  /** The CARD the last metadata load belonged to. Two variants of one card
   *  differ in `torrent_source`, so the load key cannot answer "same game". */
  let lastMetaCard: number | null = null;
  createEffect(() => {
    const g = props.game;
    if (!g) { lastGameId = null; return; }
    if (g.id === lastGameId) { return; }
    lastGameId = g.id;
    // Restart-resume: extras may still be downloading for this installed
    // game with no live tracker (app restarted mid-extras).
    if (g.installed && g.id != null && g.gamedata_torrent_index != null) {
      watchExtrasIfPending(g.id, g.title);
    }
    setImgError(false);
    setStatus("");
    setVariants([]);
    setMetadata(null);
    setBrokenImages(new Set<number>());
    setLightboxOpen(false);
    setManualOpen(false);
    setSelectedId(g.id ?? null);
    setPrintingUnavailable(false);
    setEngineInfo(null);
    if (g.id != null) {
      const id = g.id;
      gamePrintingUnavailable(id)
        .then((p) => { if (props.game?.id === id) { setPrintingUnavailable(p); } })
        .catch(() => {});
      gameEngineInfo(id)
        .then((e) => { if (props.game?.id === id) { setEngineInfo(e); } })
        .catch(() => {});
    }
    setSvmEngine(null);
    setSvmVariants(null);
    if (isScummVm(g) && g.id != null) {
      const id = g.id;
      scummvmEngineInfo(id)
        .then((e) => { if (props.game?.id === id) { setSvmEngine(e); } })
        .catch(() => {});
      loadSvmVariants(id);
    }
    // Force a metadata refetch: the cache key below would otherwise match the
    // previous visit to this same game and leave the panel with the null
    // metadata this reset just wrote (no screenshots, no manual).
    lastMetaKey = null;
    if (g.shortcode && isMultiLang()) {
      const shortcode = g.shortcode;
      loadVariants(g).then((v) => {
        // Guard: game may have changed while the async call was in flight
        if (props.game?.shortcode !== shortcode) { return; }
        setVariants(v);
        setSelectedId(defaultVariant(v)?.id ?? g.id ?? null);
      }).catch(() => {});
    }
  });

  // Metadata belongs to the SELECTED variant; keyed on id+source+manual so
  // a variant refresh (same rows, new objects) does not refetch.
  createEffect(() => {
    const v = selected();
    const row = manualRow();
    if (!v?.title || !v.torrent_source) { return; }
    const key = `${v.id}:${v.torrent_source}:${row?.manual_path ?? ""}`;
    if (key === lastMetaKey) { return; }
    // Held until settled; the loading flag keeps the Manual button busy.
    if (!panelSettled()) { setMetadataLoading(true); return; }
    // Switching the language chip keeps the gallery on screen: the media
    // belongs to the collection and title, not to the variant, and clearing
    // it here unmounted the strip and mounted a loading line in its place -
    // two layout jumps per click. Only a different GAME starts empty.
    const card = props.game?.id ?? null;
    const sameGame = lastMetaCard != null && lastMetaCard === card;
    lastMetaCard = card;
    lastMetaKey = key;
    if (!sameGame) {
      setMetadata(null);
      setBrokenImages(new Set<number>());
    }
    // A new variant gets a fresh chance at its cover; the walk itself
    // resets in the keyed effect above.
    setImgError(false);
    if (!sameGame) { setMetadataLoading(true); }
    loadGameMetadata(v.torrent_source, v.title, v.shortcode ?? null, row?.manual_path ?? null)
      .then((m) => { if (selected()?.id === v.id) { setMetadata(m); } })
      .finally(() => setMetadataLoading(false));
  });

  // Refresh the variants once per install transition (the store's
  // `installed` flag, not its status text).
  const announcedInstalls = new Set<number>();
  createEffect(() => {
    const dl = downloads();
    let freshInstall = false;
    for (const [idStr, d] of Object.entries(dl)) {
      const id = Number(idStr);
      if (d.installed && !announcedInstalls.has(id)) {
        announcedInstalls.add(id);
        freshInstall = true;
      }
    }
    for (const id of [...announcedInstalls]) {
      if (!(id in dl)) { announcedInstalls.delete(id); }
    }
    if (!freshInstall) { return; }
    const g = props.game;
    if (isScummVm(g) && g?.id != null) { loadSvmVariants(g.id); }
    if (!g?.shortcode || !isMultiLang()) { return; }
    const shortcode = g.shortcode;
    loadVariants(g, true).then((v) => {
      if (props.game?.shortcode === shortcode) { setVariants(v); }
    }).catch(() => {});
  });

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") { return; }
    // One Escape closes one layer; the actions menu covers its dialogs.
    if (lightboxOpen() || manualOpen()) { return; }
    if (moreMenu()) { setMoreMenu(null); return; }
    props.onClose();
  };

  // Register once for the lifetime of the component - the handler reads props.onClose()
  // reactively through the Proxy so it always calls the current callback.
  onMount(() => {
    ensureDismissedNotesLoaded();
    ensurePreviewMutedLoaded();
    // Capture phase: the overlay-open guard must read the signals BEFORE
    // Ark's document-level handler closes the overlay in the same keypress.
    window.addEventListener("keydown", handleKeyDown, true);
    onCleanup(() => window.removeEventListener("keydown", handleKeyDown, true));
  });

  // Own candidates, then the primary row's: an LP key with no file behind
  // it only shows as a 404, so `<img onError>` walks the list.
  const thumbCandidates = () => {
    const g = selected() ?? props.game;
    if (!g) { return []; }
    const own = thumbnailCandidates(g.torrent_source, g.thumbnail_key);
    const p = props.game;
    const primary = p && p.id !== g.id ? thumbnailCandidates(p.torrent_source, p.thumbnail_key) : [];
    return [...own, ...primary.filter((c) => !own.includes(c))];
  };
  const [thumbIdx, setThumbIdx] = createSignal(0);
  // Same reset rule as GameCard: a new row OR a changed tier list starts the
  // walk over. Keyed on the value so the settle-time re-run of the metadata
  // effect below cannot restart a walk that already fell through to Tier 0.
  createEffect(on(
    () => `${selected()?.id}|${thumbCandidates().join("|")}`,
    () => { setImgError(false); setThumbIdx(0); },
    { defer: true },
  ));
  const thumbSrc = () => {
    const list = thumbCandidates();
    if (list.length === 0) { return null; }
    // Clamp: the reset effect lands a frame after a shrinking list, and an
    // out-of-range index would show the placeholder over a valid cover.
    return convertFileSrc(list[thumbIdx()] ?? list[list.length - 1]);
  };
  const handleThumbError = (e: Event) => {
    // The <img> survives row switches (only its src changes), so a 404 still
    // in flight when the panel moves on would advance the NEW row's walk.
    const failed = (e.currentTarget as HTMLImageElement).getAttribute("src");
    if (failed !== thumbSrc()) { return; }
    if (thumbIdx() < thumbCandidates().length - 1) {
      setThumbIdx(thumbIdx() + 1);
    } else {
      setImgError(true);
    }
  };

  const handleDownload = (gameId: number, title?: string) => {
    const g = rows().find((r) => r.id === gameId) ?? props.game;
    const cover = g?.torrent_source ? { source: g.torrent_source, key: g.thumbnail_key ?? null } : undefined;
    startGameDownload(gameId, title ?? props.game?.title, cover);
  };

  // ── Preview video ──────────────────────────────────────────────────────
  const videoState = () => {
    const id = selected()?.id;
    videos(); // subscribe
    return id != null ? getVideoState(id) : undefined;
  };
  const videoReady = () => videoState()?.phase === "ready" && !!videoState()?.path;
  // Each stage of the probe says what it is; silence looks broken.
  const videoConfirmed = () => (videoState()?.total_bytes ?? 0) > 0;
  const videoProbing = () => videoState()?.phase === PHASE_PROBING;
  const videoFetching = () => videoState()?.phase === "fetching" && videoConfirmed();
  const videoQueued = () => videoState()?.phase === PHASE_QUEUED;
  const videoFailed = () => videoState()?.phase === "error";

  // No "no video" pill: most DOS titles have none. The src comes from
  // media_url (localhost on Linux, §14), else convertFileSrc.
  const [videoSrc, setVideoSrc] = createSignal<string | null>(null);
  createEffect(() => {
    const p = videoState()?.path;
    if (!p) {
      setVideoSrc(null);
      return;
    }
    let stale = false;
    mediaUrl(p)
      .then((url) => { if (!stale) { setVideoSrc(url ?? convertFileSrc(p)); } })
      .catch(() => { if (!stale) { setVideoSrc(convertFileSrc(p)); } });
    onCleanup(() => { stale = true; });
  });

  // Start the fetch a beat after the panel settles on a game. The delay is the
  // point: clicking through the grid would otherwise queue a torrent read per
  // card, and each one can pull tens of megabytes.
  createEffect(() => {
    const id = selected()?.id;
    if (id == null) { return; }
    setForegroundVideo(id);
    const timer = window.setTimeout(() => requestVideo(id), 400);
    onCleanup(() => {
      // Only the not-yet-started fetch is dropped. One that is already running
      // keeps going in the background so the video is simply there next time.
      clearTimeout(timer);
      releaseVideo(id);
    });
  });

  // ── Theme track ────────────────────────────────────────────────────────
  // Same beat as the video. Autoplay on: it becomes the wanted track; off:
  // fetched so the row can offer it.
  ensureMusicAutoplayLoaded();
  ensureVideoMirrorKnown();
  /** The theme belongs to the GROUP, not the selected variant: extras live in
   *  the EN archive only (every LP row has a NULL gamedata index, §14), so
   *  switching the language chip must neither restart nor re-request it. */
  const themeOwner = () => variants().find((v) => v.language === "EN") ?? props.game ?? selected();
  /** The theme this panel's autoplay asked for, so it can be withdrawn. */
  let autoThemeFor: number | null = null;
  createEffect(() => {
    const g = themeOwner();
    const id = g?.id;
    if (g == null || id == null || musicUnsupported()) { return; }
    const timer = window.setTimeout(() => {
      // Already the active track (e.g. started from the Browse list, or the
      // owner resolved while playing): leave the player alone - re-issuing
      // playTheme would restart it and yank a list queue into theme mode.
      if (currentTrack()?.gameId === id || wantedTrack()?.gameId === id) { return; }
      // Autoplay never overrules the listener: `playTheme` clears the bar's
      // hidden and paused flags, which is right for a click on ▶ and wrong
      // here - it made the × last only until the next game was opened.
      const dismissed = musicUserPaused() || playerHidden();
      if (musicAutoplay() && !dismissed) { playTheme(g, { auto: true }); autoThemeFor = id; } else { void requestTheme(id); }
    }, 400);
    onCleanup(() => clearTimeout(timer));
  });
  // A theme this panel auto-requested is withdrawn when the panel goes away
  // before the bytes arrive - or a track nobody is looking at starts out of
  // nowhere a minute later (report 2026-09-08). A click's track stays.
  const dropAutoTheme = () => {
    if (autoThemeFor != null) { withdrawAutoTheme(autoThemeFor); autoThemeFor = null; }
  };
  createEffect(() => { if (!props.game) { dropAutoTheme(); } });
  onCleanup(dropAutoTheme);
  const musicState = () => {
    const id = themeOwner()?.id;
    musicJobs(); // subscribe
    return id != null ? getMusicState(id) : undefined;
  };
  const musicReady = () => musicState()?.phase === "ready" && !!musicState()?.path;
  const musicBusy = () => {
    const p = musicState()?.phase;
    return p === "fetching" || p === PHASE_PROBING || p === MUSIC_QUEUED;
  };
  const musicFailed = () => musicState()?.phase === "error";
  // No "no theme for this game" line, for the same reason there is no such
  // pill for the video: the honest answer is also the useless one.
  const musicRowVisible = () => musicReady() || musicBusy() || musicFailed();
  const isPlayingThisTheme = () => {
    const id = themeOwner()?.id;
    return id != null && currentTrack()?.gameId === id && musicPlaying();
  };

  // Was a fetch phase observed for the current game? Then the user already
  // spent the wait looking at the cover, and the ready video starts at once.
  // A cache hit reports "ready" as its first state and keeps the cover beat.
  const [videoJustFetched, setVideoJustFetched] = createSignal(false);
  let videoPhaseGame: number | null | undefined;
  createEffect(() => {
    const id = selected()?.id;
    const phase = videoState()?.phase;
    if (id !== videoPhaseGame) { videoPhaseGame = id; setVideoJustFetched(false); }
    if (phase === "fetching" || phase === PHASE_PROBING || phase === PHASE_QUEUED) {
      setVideoJustFetched(true);
    }
  });

  // The hero follows the selected row and its source, nothing else: a row
  // with a ready video is shown (the controller waits out the cover beat),
  // anything else clears it. The mute preference is read at start time.
  createEffect(() => {
    const id = selected()?.id;
    const url = videoSrc();
    if (id == null || !url) {
      clearPreview();
      return;
    }
    showPreview(id, url, untrack(() => ({
      muted: previewMuted(),
      delayMs: videoJustFetched() ? 0 : VIDEO_START_DELAY_MS,
    })));
  });
  /** Why the preview did not start, for this row. */
  const videoError = () => (heroGameId() === selected()?.id ? heroError() : null);

  const toggleMute = () => {
    const next = !previewMuted();
    void setPreviewMuted(next);
    setPreviewMutedNow(next);
  };

  // The lightbox plays the same preview in its own element; the hero steps
  // aside, and while the lightbox has the trailer with sound (entry 0) the
  // speakers stay claimed.
  const lightboxHoldsAudio = () =>
    lightboxOpen() && !!videoSrc() && !previewMuted() && lightboxStart() === 0;
  createEffect(() => setLightbox(lightboxOpen(), lightboxHoldsAudio()));

  const handleManualClick = () => {
    if (metadata()?.manual_path) { setManualOpen(true); }
  };

  /** Online-capable Win9x games ask once, on the first Play, whether to turn
   *  multiplayer on - the backend answers false for every game and every
   *  state where the question would be noise. */
  const handleLaunch = async (gameId: number) => {
    if (launchingId() != null) { return; }
    try {
      const info = await win9xMultiplayerInfo(gameId);
      win9x.setMp(info);
      if (info.prompt) {
        setNetPromptFor(gameId);
        return;
      }
    } catch { /* older backend - just launch */ }
    void startLaunch(gameId);
  };

  const startLaunch = async (gameId: number) => {
    setLaunchingId(gameId);
    setStatus("");
    const startedAt = Date.now();
    // The game has its own sound; the theme waits and `game-exited` brings it
    // back - only if this launch is what paused it.
    pauseForGame(gameId);
    try {
      await launchGame(gameId);
      // DOSBox spawns immediately but the window can take 1-3s to paint
      // (codesign re-sign on macOS dev, asset preload). Hold the spinner
      // for at least 4s so the user sees it before the button reverts.
      const elapsed = Date.now() - startedAt;
      const remaining = Math.max(0, 4000 - elapsed);
      if (launchTimer) { clearTimeout(launchTimer); }
      launchTimer = window.setTimeout(() => setLaunchingId(null), remaining);
    } catch (e) {
      setLaunchingId(null);
      setStatus("");
      const detail = String(e).replace(/^Error:\s*/, "");
      // "already running" means the first launch still holds the speakers.
      if (!detail.includes("already running")) { resumeFromGame(gameId); }
      showToast(`Couldn't launch ${props.game?.title ?? "game"}`, "error", { detail });
    }
  };

  const handleUninstall = async (gameId: number) => {
    if (uninstallingId() != null) { return; }
    // Capture shortcode + title now - props.game may change before the async callback runs.
    const shortcode = props.game?.shortcode;
    const title = variants().find((v) => v.id === gameId)?.title ?? props.game?.title;
    setUninstallingId(gameId);
    // The action row renders the "Uninstalling" state itself now - swallow
    // performUninstall's identical status text (it also feeds GameCard,
    // which has no action row) so the panel doesn't show it twice.
    const statusSink = (msg: string) => {
      if (msg === "Uninstalling...") { return; }
      setStatus(msg);
    };
    try {
      await performUninstall(gameId, statusSink, async () => {
        if (shortcode) {
          const v = props.game
            ? await loadVariants(props.game, true).catch(() => [])
            : [];
          setVariants(v);
        }
      }, title);
    } finally {
      setUninstallingId(null);
    }
  };

  // A pending "really?" must not carry over to another language variant -
  // the action bar targets the selected row, so the second click would hit a
  // game the user never armed.
  createEffect(() => {
    selected()?.id;
  });

  /** Only the in-bar "Resetting…" state lives here; the work and its toasts
   *  are `performReset`, shared with the grid's context menu. */
  const handleReset = async (gameId: number) => {
    if (resettingId() != null) { return; }
    const title = variants().find((v) => v.id === gameId)?.title ?? props.game?.title;
    setResettingId(gameId);
    try {
      // The action row renders the progress itself, so swallow the status text.
      await performReset(gameId, () => {}, title);
    } finally {
      setResettingId(null);
    }
  };

  const ratingStars = (rating: number | null) => {
    if (rating == null) { return null; }
    // eXoDOS ratings are 0–5 scale
    const full = Math.round(rating);
    const empty = 5 - full;
    return "★".repeat(full) + "☆".repeat(empty);
  };

  // Manual: the selected variant's, else the EN row's with the language in
  // the label. Unresolved = GameData still downloading; a click retries.
  /** Favourite state of the CARD's row, not the variant: the grid and the
   *  Favorites shelf star `props.game` (§12). */
  const [favorited, setFavorited] = createSignal(false);
  createEffect(() => { setFavorited(props.game?.favorited ?? false); });

  const handleToggleFavorite = async () => {
    const id = props.game?.id;
    if (id == null) { return; }
    const next = !favorited();
    setFavorited(next);
    try {
      await toggleFavorite(id);
      updateGameFavorited(id, next);
    } catch (e) {
      setFavorited(!next);
      showToast("Couldn't update favorites", "error", { detail: String(e) });
    }
  };

  const manualAvailable = () => !!metadata()?.manual_path;

  const ManualButton = () => (
    <Button
      variant="action"
      class="btn-manual"
      onClick={handleManualClick}
      // First open of a game can scan for seconds (gallery thumbnails, lazy
      // manual extraction) - show that instead of a dead-looking button.
      loading={metadataLoading()}
      // Not yet extracted: the button is simply inert rather than clickable
      // into a "not available" message. The metadata cache is invalidated when
      // a download finishes, so it enables itself once the extras land.
      disabled={!manualAvailable()}
      title={
        !metadataLoading() && !manualAvailable()
          ? "Arrives with the game's extras download"
          : manualIsFallback()
            ? `Only the ${languageName(manualRow()?.language)} manual is in the catalogue`
            : undefined
      }
    >
      Manual
      <Show when={!metadataLoading() && manualAvailable() && manualIsFallback()}>
        <span class="btn-suffix">{manualRow()?.language}</span>
      </Show>
    </Button>
  );

  // A running game's Play button becomes "Stop game", Steam-style. The
  // `runningGameIds` set is armed at launch and cleared by `game-exited`, so
  // the button reverts by itself however the game ends.
  const [stoppingId, setStoppingId] = createSignal<number | null>(null);
  createEffect(() => {
    const s = stoppingId();
    if (s != null && !runningGameIds().has(s)) { setStoppingId(null); }
  });
  const handleStop = async (gameId: number) => {
    if (stoppingId() != null) { return; }
    setStoppingId(gameId);
    try {
      await stopGame(gameId);
      // The reaper's game-exited clears the running set; the effect above
      // then puts the button back.
    } catch (e) {
      setStoppingId(null);
      showToast("Couldn't stop the game", "error", { detail: String(e).replace(/^Error:\s*/, "") });
    }
  };

  // Shared "Play" button - same disabled+spinner UX whether it's the main
  // single-language action or one row of the multi-language variant list.
  // A pending note (emulator or support files still downloading) keeps it
  // spinning: a launch would only fail with the note's own message. While
  // the game runs (and the Starting… beat is over), it is the Stop button.
  const PlayButton = (p: { id: number; class?: string; disabled?: boolean }) => (
    <Show
      when={!runningGameIds().has(p.id) || launchingId() === p.id}
      fallback={
        <Button
          variant="action"
          class={`${p.class ?? ""} is-stop`}
          onClick={() => handleStop(p.id)}
          loading={stoppingId() === p.id}
          loadingLabel="Stopping…"
        >
          ■ Stop game
        </Button>
      }
    >
      <Button
        variant="action"
        class={p.class}
        onClick={() => handleLaunch(p.id)}
        disabled={p.disabled}
        loading={launchingId() === p.id || note()?.pending === true}
        loadingLabel={launchingId() === p.id ? "Starting…" : "Preparing…"}
      >
        ▶ Play
      </Button>
    </Show>
  );

  // Genre column is semicolon-joined. The hero chip shows the first piece
  // alone (the "primary" genre); the fields row joins all pieces with " · ".
  const genreList = (): string[] => {
    const raw = props.game?.genre;
    if (!raw) { return []; }
    return raw.split(";").map((p) => p.trim()).filter(Boolean);
  };
  /** Whether the "Information" block has anything to show. Without this the
   *  heading and its rule would sit above nothing for the many DOS titles that
   *  carry no series, region, player count or rating. */
  const hasInformation = () =>
    field("series") != null || field("region") != null
    || field("max_players") != null || field("rating") != null;
  const allGenres = (): string | null => {
    const list = genreList();
    return list.length > 0 ? list.join(" · ") : null;
  };

  return (
    <Show when={props.game}>
      <Portal>
        <div class="game-detail-backdrop" onClick={props.onClose} />
        {/* animationend bubbles, so only the panel's own slide-in counts -
            a child's spinner or badge burst must not settle the panel early. */}
        <div
          class="game-detail-panel"
          data-testid="game-detail"
          onAnimationEnd={(e) => { if (e.target === e.currentTarget) { markSettled(); } }}
        >
          {/* Hero: thumbnail + title. The close button lives INSIDE it so the
              hover that reveals it survives the pointer reaching the button -
              as a sibling, moving onto it left the hero un-hovered and the
              control faded out from under the cursor. */}
          <div class="game-detail-hero">
            <button class="game-detail-close" onClick={props.onClose} title="Close">✕</button>
            <div class="game-detail-hero-art">
            <Show when={thumbSrc() && !imgError()}>
              <img class="game-detail-thumb-backdrop" src={thumbSrc()!} alt="" aria-hidden="true" />
              <img
                class="game-detail-thumb"
                src={thumbSrc()!}
                alt=""
                onError={handleThumbError}
                onClick={() => { setLightboxStart(lightboxIndexOfImage(0)); setLightboxOpen(true); }}
              />
            </Show>
            <Show when={!thumbSrc() || imgError()}>
              <div class="game-detail-thumb-placeholder" />
            </Show>

            {/* The preview takes the cover's place while it runs, then fades
                back out - it stays reachable in the lightbox afterwards. */}
            {/* Always mounted, never given a `src` from here: the controller
                sets and clears it, so no element is ever detached mid-start. */}
            <video
              ref={attachHero}
              class={`game-detail-hero-video${needsCanvasVideo() ? " has-canvas" : ""}${videoPlaying() ? " is-visible" : ""}`}
              playsinline
              preload="auto"
              onClick={() => { setLightboxStart(0); setLightboxOpen(true); }}
            />
            {/* Linux: the frames are painted here, not by the element -
                WebKitGTK's own video path is untrusted (videoCanvas.ts). */}
            <Show when={needsCanvasVideo()}>
              <canvas
                ref={(c) => {
                  if (heroEl) {
                    onCleanup(attachCanvasPainter(heroEl, c, "cover"));
                  }
                }}
                class={`game-detail-hero-video game-detail-hero-canvas${videoPlaying() ? " is-visible" : ""}`}
                aria-hidden="true"
              />
            </Show>

            {/* Status while the bytes are still coming over the torrent. */}
            <Show when={videoProbing() || videoFetching() || videoQueued()}>
              <div class="game-detail-video-status">
                <span class="btn-spinner" />
                <Show when={videoQueued()} fallback={
                  <Show when={videoConfirmed()} fallback={<>Looking for a video…</>}>
                    Loading video {Math.round((videoState()?.progress ?? 0) * 100)}%
                  </Show>
                }>
                  Video queued…
                </Show>
              </div>
            </Show>

            {/* The bytes are here but the element will not run them: name
                the reason rather than leave a cover that never moves. */}
            <Show when={videoError() && !videoPlaying()}>
              <div class="game-detail-video-status game-detail-video-error" title={videoError()!}>
                Preview can't play - {videoError()}
              </div>
            </Show>

            {/* A failed fetch must not look like "this game has no video" -
                a stalled torrent read is worth retrying, a missing video is not. */}
            <Show when={videoFailed()}>
              <button
                class="game-detail-video-status game-detail-video-retry"
                title={videoState()?.error ?? undefined}
                onClick={() => { const id = selected()?.id; if (id != null) { requestVideo(id); } }}
              >{isVideoTimeout(videoState()) ? "↻ No peers yet - video retry" : "↻ Video retry"}</button>
            </Show>

            {/* Nothing about a cover says it can be opened larger. The hint
                appears with the other hover controls and is inert - the click
                belongs to the artwork underneath, which already opens the
                lightbox, so this must not become a second target that swallows
                it near the middle. */}
            <Show when={thumbSrc() && !imgError()}>
              <div class="game-detail-zoom-hint" aria-hidden="true"><IconZoom /></div>
            </Show>

            {/* Sound toggle, only while the preview is actually running. The
                preference is global and persistent - per game it would mean
                muting the same trailer over and over. */}
            <Show when={videoPlaying()}>
              <button
                class="game-detail-video-sound"
                title={previewMuted() ? "Unmute previews" : "Mute previews"}
                aria-label={previewMuted() ? "Unmute previews" : "Mute previews"}
                onClick={toggleMute}
              >{previewMuted() ? <IconSoundOff /> : <IconSoundOn />}</button>
            </Show>

            {/* Replay control once it has run its course. */}
            <Show when={videoReady() && !videoPlaying()}>
              <button
                class="game-detail-video-replay"
                title="Play the preview again"
                onClick={() => replayPreview(previewMuted())}
              ><IconPlay size={22} /></button>
            </Show>
            </div>

            {/* Title, then who made it, then what kind of thing it is. The
                three sit in that order because that is the order they answer
                "what am I looking at" - and none of them competes with the
                description for width any more. */}
            <div class="game-detail-hero-info">
              <div class="game-detail-title" data-testid="game-detail-title">{selected()?.title ?? props.game!.title}</div>

              {/* Credits carry an icon instead of a label: with only three
                  lines, and values that read as names and a year, uppercase
                  labels were three columns of furniture for no gain. */}
              <div class="game-detail-credits">
                <Credit icon="developer" value={field("developer")} title="Developer" />
                <Credit icon="publisher" value={field("publisher")} title="Publisher" />
                <Credit icon="year" value={field("year")} title="Year" />
              </div>

              {/* The categorical facts - the ones a user filters or scans by.
                  Tinted per category rather than per state, so the row reads
                  as a legend instead of a status. */}
              <div class="game-detail-tags">
                <Tag kind="genre" value={allGenres()} title="Genre" />
                <Tag kind="platform" value={field("platform")} title="System" />
                <Tag kind="mode" value={field("play_mode")} title="Mode" />
                <Tag kind="emulator" value={emulatorName()} title="Emulator" />
              </div>
            </div>
          </div>

          <div class="game-detail-body">
            {/* Status message */}
            <Show when={currentStatus()}>
              <div class="game-detail-status">{currentStatus()}</div>
            </Show>

            {/* Exactly one note. Three stacked boxes read as a wall of
                warnings and buried the one that mattered, so they are ordered
                by how much the reader can do about it. */}
            <Show when={shownNote()}>
              {(n) => (
                <div class={`game-detail-note-slot${noteOpen() ? " is-open" : ""}`}>
                <div class="game-detail-note-clip">
                <div class={`game-detail-note${n().blocking ? " is-blocking" : ""}`}>
                  <span class="game-detail-note-mark" aria-hidden="true">
                    {n().blocking ? "!" : "i"}
                  </span>
                  <p class="game-detail-note-text">{n().text}</p>
                  <Show when={n().action}>
                    {(a) => (
                      <Button variant="action" class="game-detail-note-action" onClick={() => a().onClick()}>
                        {a().label}
                      </Button>
                    )}
                  </Show>
                  <Show when={!n().blocking}>
                    <button
                      class="game-detail-note-dismiss"
                      title="Don't show this note again"
                      aria-label="Don't show this note again"
                      onClick={() => { void dismissNote(n().key); }}
                    >✕</button>
                  </Show>
                </div>
                </div>
                </div>
              )}
            </Show>

            {/* ScummVM platform variants (§18): one chip per folder, the
                choice is a per-game setting the next launch uses. */}
            <Show when={isScummVm(selected() ?? props.game) && (svmVariants()?.variants.length ?? 0) > 1}>
              <div class="variant-switcher" role="group" aria-label="Versions">
                <For each={svmVariants()!.variants}>
                  {(v) => (
                    <button
                      class={`variant-chip${svmVariants()?.selected.variant === v.name ? " is-selected" : ""}`}
                      title={v.name}
                      onClick={() => { void chooseSvmVariant(v.name); }}
                    >
                      <span class="variant-chip-label">{svmVariantLabel(v.name)}</span>
                    </button>
                  )}
                </For>
              </div>
            </Show>

            {/* Language switcher: picking a chip re-points the whole panel -
                actions, description, manual and screenshots all follow it. */}
            <Show when={isMultiLang()}>
              <div class="variant-switcher" role="group" aria-label="Language versions" data-testid="language-variants">
                <Show when={rows().length < 2}>
                  <div class="game-detail-loading">Loading versions…</div>
                </Show>
                <For each={rows()}>
                  {(variant) => {
                    const vId = () => variant.id;
                    const vDl = () => vId() != null ? getDownloadState(vId()!) : undefined;
                    const state = () => variant.installed ? 2 : variant.in_library ? 1 : 0;
                    return (
                      <button
                        class={`variant-chip${selected()?.id === vId() ? " is-selected" : ""}`}
                        data-testid="variant-chip"
                        onClick={() => { if (vId() != null) { setSelectedId(vId()!); } }}
                        title={languageName(variant.language)}
                      >
                        <span class={`badge badge-lang ${langBadgeClass(state())}`}>
                          {variant.language}
                        </span>
                        <span class="variant-chip-state">
                          <Show when={vDl()?.downloading} fallback={
                            <Show when={variant.installed} fallback={
                              <Show when={!isOffline() && variant.game_torrent_index != null} fallback={<>Not installed</>}>
                                ↓ {formatBytes(variant.download_size ?? 0)}
                              </Show>
                            }>
                              ✓ Installed
                            </Show>
                          }>
                            {Math.round((vDl()?.progress ?? 0) * 100)}%
                          </Show>
                        </span>
                      </button>
                    );
                  }}
                </For>
              </div>
            </Show>

            {/* One action bar for every game. Everything here targets the
                SELECTED row, so a merged card can play the German version and
                open the German manual without a second code path. */}
            <Show when={selected()}>
              {(sel) => (
                <Show when={uninstallingId() !== sel().id} fallback={
                  <div class="game-detail-actions fade-swap">
                    <div class="game-detail-btn btn-uninstalling">
                      <span class="btn-spinner" /> Uninstalling…
                    </div>
                  </div>
                }>
                  <div class="game-detail-actions fade-swap">
                    <Show when={selectedInstalled() && sel().id != null}>
                      <PlayButton id={sel().id!} class="btn-play" />
                    </Show>
                    <Show when={selectedInstalled() && manualRow()}>
                      <ManualButton />
                    </Show>
                    <Show when={!selectedInstalled() && selectedDownloading()}>
                      <div class="game-detail-btn btn-downloading">
                        <AutoProgress
                          value={selectedDl()?.progress ?? 0}
                          class="mini"
                          indeterminate={selectedDl()?.status?.startsWith("Waiting") || selectedDl()?.status?.startsWith("Extracting") || undefined}
                        />
                        <span>{selectedDl()?.status}</span>
                      </div>
                      <Button variant="action" class="btn-cancel" onClick={() => cancelGameDownload(sel().id!)}>
                        ✕ Cancel
                      </Button>
                    </Show>
                    <Show when={!selectedInstalled() && !selectedDownloading() && sel().game_torrent_index != null && !isOffline()}>
                      <Button
                        variant="action"
                        class="btn-download"
                        onClick={() => handleDownload(sel().id!, isMultiLang() ? `${sel().title} [${sel().language}]` : sel().title)}
                      >
                        {sel().in_library
                          ? "↓ Re-download"
                          : `↓ Download ${sel().download_size ? formatBytes(sel().download_size!) : ""}`}
                      </Button>
                    </Show>
                    <Show when={!selectedInstalled() && !selectedDownloading() && isOffline()}>
                      <div class="game-detail-btn btn-offline" title="Enable downloads in Settings → Network">
                        Not installed - offline mode
                      </div>
                    </Show>
                    {/* Frequent, reversible, and the one action that is not
                        about launching - so it stays in the bar rather than
                        moving into the menu with the destructive items. */}
                    <Show when={props.game!.id != null}>
                      <Button
                        variant="action"
                        class={`btn-fav${favorited() ? " is-favorited" : ""}`}
                        title={favorited() ? "Remove from favorites" : "Add to favorites"}
                        aria-label={favorited() ? "Remove from favorites" : "Add to favorites"}
                        onClick={() => { void handleToggleFavorite(); }}
                      >
                        {favorited() ? "★" : "☆"}
                      </Button>
                    </Show>

                    {/* Everything else lives behind one control. The bar used
                        to carry five, and the two that matter - the primary
                        action for the current state, and the manual - had to
                        compete with three that are rarely wanted and two of
                        which destroy data. Reset still confirms in place, so
                        the menu cannot turn a stray click into a wipe. */}
                    <Show when={resettingId() !== sel().id} fallback={
                      <div class="game-detail-btn btn-uninstalling">
                        <span class="btn-spinner" /> Resetting…
                      </div>
                    }>
                      <Show when={hasMoreActions()}>
                        <Button
                          variant="action"
                          class="btn-more"
                          title="More actions"
                          aria-label="More actions"
                          onClick={openMoreMenu}
                        >
                          ⋯
                        </Button>
                      </Show>
                    </Show>
                  </div>
                </Show>
              )}
            </Show>

            {/* Two columns side by side on a wide panel, stacked when it's
                narrow (flex-wrap, no breakpoint) - the pair is what keeps the
                screenshots on screen without scrolling. */}
            <div class="game-detail-scroll">
              {/* One column. Fields beside the description gave each of them
                  half of a 560 px panel: the field table wrapped company names
                  over three lines and the text ran at ~40 characters, half a
                  comfortable measure. Neither needed the other's company - a
                  reader takes them in sequence, not side by side. */}
              <div class="game-detail-text">
                <Show when={descriptionSource()}>
                  {(src) => (
                    <>
                      <div class={`game-detail-collapse${src().fallbackFrom ? " is-open" : ""}`}>
                        <div>
                          <div class="game-detail-fallback-note">
                            English description - the catalogue has no{" "}
                            {languageName(src().fallbackFrom ?? selected()?.language ?? null)} text
                            for this game.
                          </div>
                        </div>
                      </div>
                      <div class="game-detail-description">{src().text}</div>
                      <Show when={src().notes}>
                        <div class="game-detail-notes">{src().notes}</div>
                      </Show>
                    </>
                  )}
                </Show>
                <Show when={metadataLoading()}>
                  <div class="game-detail-loading">Loading media…</div>
                </Show>
              </div>

              {/* The long tail: everything not already answered by the credits
                  or the tag row. Full width, so two pairs fit per line and
                  nothing wraps - and below the description, because these are
                  looked up rather than read. */}
              <Show when={hasInformation()}>
                <div class="game-detail-info">
                  <div class="game-detail-section-label">Information</div>
                  <div class="game-detail-fields">
                    <Field icon="series" label="Series" value={field("series")} />
                    <Field icon="region" label="Region" value={field("region")} />
                    <Field icon="players" label="Players" value={field("max_players")} />
                    <Field
                      icon="rating"
                      label="Rating"
                      value={field("rating") != null ? ratingStars(field("rating") as number) : null}
                      valueClass="game-detail-stars"
                    />
                  </div>
                </div>
              </Show>

              {/* The theme track, once the archive has confirmed one - or
                  while it is still being asked. Playback itself lives in the
                  player bar; this is where it is started for this game. */}
              <Show when={musicRowVisible()}>
                <div class="game-detail-music">
                  <div class="game-detail-section-label">Theme</div>
                  <div class="game-detail-music-row">
                    <Show when={musicReady()}>
                      <Button
                        variant="small"
                        onClick={() => {
                          const g = themeOwner();
                          if (!g) { return; }
                          if (isPlayingThisTheme()) { togglePlay(); } else { playTheme(g); }
                        }}
                      >{isPlayingThisTheme() ? "Pause" : "Play"}</Button>
                      <span class="game-detail-music-name">
                        {themeOwner()?.music_file?.replace(/\.[^.]+$/, "") ?? themeOwner()?.title}
                      </span>
                    </Show>
                    <Show when={musicBusy()}>
                      <span class="btn-spinner" />
                      <span class="game-detail-music-name">
                        <Show when={musicState()?.phase === "fetching"} fallback={
                          musicState()?.phase === MUSIC_QUEUED ? "Theme queued…" : "Looking for a theme…"
                        }>
                          Loading theme {Math.round((musicState()?.progress ?? 0) * 100)}%
                        </Show>
                      </span>
                    </Show>
                    <Show when={musicFailed()}>
                      <Button
                        variant="small"
                        title={musicState()?.error ?? undefined}
                        onClick={() => { const id = themeOwner()?.id; if (id != null) { void requestTheme(id); } }}
                      >{isTimeoutError(musicState()) ? "↻ No peers yet - theme retry" : "↻ Theme retry"}</Button>
                    </Show>
                  </div>
                </div>
              </Show>
            </div>

            {/* Media: screenshots/art - only renders if the metadata content
                pack has assets for this game. Pinned to the bottom of the
                panel so it's always visible. */}
            <Show when={!metadataLoading() && metadata() && metadata()!.images.length > 0}>
              <div class="game-detail-media">
                {(() => {
                  const visible = () => (metadata()?.images ?? []).filter((_, i) => !brokenImages().has(i));
                  return (
                    <Show when={metadata()!.images.length > 0 && visible().length > 0}>
                      <div class="game-detail-section-label">
                        Screenshots &amp; Art
                        <span class="section-count">{visible().length}</span>
                      </div>
                      <div class="game-detail-gallery-strip">
                        <For each={metadata()!.images}>
                          {(path, i) => (
                            <img
                              // Strip shows the cached 160px copy; the lightbox
                              // opens the full-resolution file behind it.
                              src={convertFileSrc(metadata()!.thumbnails[i()] ?? path)}
                              class="gallery-thumb"
                              loading="lazy"
                              alt=""
                              onClick={() => {
                                const vi = visible().indexOf(path);
                                setLightboxStart(lightboxIndexOfImage(vi >= 0 ? vi : 0));
                                setLightboxOpen(true);
                              }}
                              onError={() => setBrokenImages((prev) => new Set(prev).add(i()))}
                              style={{ display: brokenImages().has(i()) ? "none" : undefined }}
                            />
                          )}
                        </For>
                      </div>
                    </Show>
                  );
                })()}
              </div>
            </Show>
          </div>
        </div>

        <Lightbox
          video={videoSrc()}
          images={(() => {
            const filtered = (metadata()?.images ?? []).filter((_, i) => !brokenImages().has(i));
            if (filtered.length > 0) { return filtered; }
            // Fallback: use the hero thumbnail so clicking box art works even
            // without the metadata pack installed.
            const hero = bestThumbnailPath(props.game?.torrent_source, props.game?.thumbnail_key);
            return hero ? [hero] : [];
          })()}
          startIndex={lightboxStart()}
          open={lightboxOpen()}
          onClose={() => setLightboxOpen(false)}
        />
        <ManualViewer
          path={metadata()?.manual_path ?? null}
          kind={metadata()?.manual_kind ?? null}
          open={manualOpen()}
          onClose={() => setManualOpen(false)}
        />
        {/* Asked on Play, not in Settings: this is the moment the online mode
            would otherwise silently be missing. Either answer can be
            remembered - a question that only goes quiet when accepted is not
            a question. */}
        <ConfirmDialog
          open={netPromptFor() != null}
          title="Play online?"
          message="This game can play online against others who own the collection, over a community-run IPX gateway. That needs one-time permission from your system to bridge the emulated network card; you can also play on your own without it."
          confirmLabel="Set up now…"
          cancelLabel="Play offline"
          rememberLabel="Don't ask again"
          onConfirm={async (remember) => {
            netPromptAccepted = true;
            const gameId = netPromptFor();
            if (remember) { void dismissWin9xNetworkPrompt(); }
            try {
              await enableWin9xNetwork();
            } catch (e) {
              const msg = String(e);
              if (!msg.includes("cancelled")) {
                showToast("Could not enable multiplayer", "error", { detail: msg });
              }
            }
            // Launch either way: a dismissed system dialog means "not now",
            // not "don't play".
            if (gameId != null) { void startLaunch(gameId); }
          }}
          onClose={(remember) => {
            const gameId = netPromptFor();
            setNetPromptFor(null);
            if (remember) { void dismissWin9xNetworkPrompt(); }
            // onClose also fires after onConfirm, which launches on its own.
            if (gameId != null && !netPromptAccepted) { void startLaunch(gameId); }
            netPromptAccepted = false;
          }}
        />
        {/* One component for both surfaces - see GameActionsMenu. It owns the
            confirm steps and the dialogs it opens, so this only supplies the
            anchor and clears it when everything has closed. */}
        <Show when={moreMenu() && selected()}>
          <GameActionsMenu
            game={selected()!}
            x={moreMenu()!.x}
            y={moreMenu()!.y}
            rightAnchored
            abovePanel
            downloading={selectedDownloading()}
            onReset={handleReset}
            onUninstall={handleUninstall}
            onClose={() => setMoreMenu(null)}
          />
        </Show>
      </Portal>
    </Show>
  );
}
