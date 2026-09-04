import { createSignal, createEffect, on, Show, For, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AutoProgress } from "./ProgressBar";
import { Lightbox } from "./Lightbox";
import { ManualViewer } from "./ManualViewer";
import { GameActionsMenu } from "./GameActionsMenu";
import { FieldIcon, IconSoundOn, IconSoundOff, IconZoom, type FieldIconName } from "./icons";
import { ConfirmDialog } from "./ConfirmDialog";
import { Button } from "./Button";
import type { Game, GameMetadata } from "../api/tauri";
import { launchGame, gameEngineInfo, gamePrintingUnavailable, win9xEngineAvailable, win9xMultiplayerInfo, dismissWin9xNetworkPrompt, enableWin9xNetwork, getWin9xSupportStatus, mediaUrl } from "../api/tauri";
import type { GameEngineInfo, Win9xMultiplayerInfo, Win9xSupportStatus } from "../api/tauri";
import { formatBytes, parseLangEntries, langBadgeClass, performUninstall, performReset } from "../util";
import { showToast } from "../stores/toasts";
import { bestThumbnailPath, thumbnailCandidates } from "../stores/thumbnails";
import { downloads, startGameDownload, getDownloadState, cancelGameDownload, watchExtrasIfPending } from "../stores/downloads";
import { loadGameMetadata } from "../stores/metadata";
import { isOffline } from "../stores/network";
import { loadVariants } from "../stores/variants";
import { toggleFavorite, updateGameFavorited } from "../stores/games";
import { videos, requestVideo, releaseVideo, setForegroundVideo, getVideoState, videoPlaybackUnsupported, PHASE_QUEUED, PHASE_PROBING } from "../stores/videos";
import { ensureDismissedNotesLoaded, isNoteDismissed, dismissedNotesLoaded, dismissNote } from "../stores/notes";
import { packsByCollection, activeJobs, installedPacks, startContentPackInstall } from "../stores/contentPacks";
import { ensurePreviewMutedLoaded, previewMuted, setPreviewMuted } from "../stores/playback";
import { musicJobs, getMusicState, requestTheme, playTheme, pauseFor, resumeFrom, pauseForGame, resumeFromGame, togglePlay, currentTrack, wantedTrack, musicPlaying, musicAutoplay, ensureMusicAutoplayLoaded, musicUnsupported, MUSIC_QUEUED } from "../stores/music";

interface Props {
  game: Game | null;
  onClose: () => void;
  onDownloadStart?: (gameId: number) => void;
}

/** How long the cover keeps the hero to itself before the preview fades in. */
const VIDEO_START_DELAY_MS = 2000;

/** Fallback for the slide-in's `animationend` (main.css: 260ms). Only reached
 *  when the event cannot arrive - prefers-reduced-motion, a hidden window -
 *  so it may sit a little past the animation without costing anything. */
const SETTLE_FALLBACK_MS = 350;

/** A note, plus the two things the UI needs to decide about it: a stable key
 *  to remember a dismissal under, and whether it may be dismissed at all.
 *  `blocking` notes describe a launch that cannot work; hiding those would
 *  leave the Play button failing with no explanation on screen. */
interface PanelNote {
  key: string;
  text: string;
  blocking?: boolean;
  /** Optional remedy rendered as a button in the note - a blocking note that
   *  names a fix the app can perform (download the emulator pack) should
   *  offer it right there instead of sending the user to Settings. */
  action?: { label: string; onClick: () => void };
}

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

/** One metadata row: pictogram, label, value. Ten of these written out was
 *  six lines each of identical markup, and adding a field meant remembering
 *  the shape. Renders nothing when the value is absent, which is what every
 *  caller's `<Show>` used to do. */
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
  const [videoPlaying, setVideoPlaying] = createSignal(false);
  // 13 eXoDOS titles print as their core feature; Staging has no printer
  // emulation yet, so those get a heads-up note. The backend owns the whole
  // answer (conf + engine selection), so no platform logic lives here.
  const [printingUnavailable, setPrintingUnavailable] = createSignal(false);
  // Win9x games run in DOSBox-X/86Box, not Staging. The variant slugs only
  // exist in the eXoWin9x catalogue, so they double as the collection test.
  const isWin9x = (g: Game | null) => {
    const v = g?.dosbox_variant;
    return v === "x98" || v === "pcbox" || (v?.startsWith("86box") ?? false);
  };
  const [win9xEngineMissing, setWin9xEngineMissing] = createSignal(false);
  /** Shared Win9x support payload (OS images + eXo's emulators): null until
   *  probed. Drives the download-progress note and the one-time-size hint. */
  const [supportStatus, setSupportStatus] = createSignal<Win9xSupportStatus | null>(null);
  /** Online-play state of the open game: drives both the panel note and the
   *  question on Play. Null until probed, so nothing flashes. */
  const [mpInfo, setMpInfo] = createSignal<Win9xMultiplayerInfo | null>(null);
  /** Game id awaiting the multiplayer question, or null. */
  const [netPromptFor, setNetPromptFor] = createSignal<number | null>(null);
  /** Set by the dialog's confirm path, which starts its own launch - so the
   *  close handler (which fires for both answers) does not start a second. */
  let netPromptAccepted = false;
  /** ECE or Staging for the selected game, straight from the backend's own
   *  `resolve_engine`. Not derived from the variant here: the answer depends
   *  on the platform, on the ECE build being extracted, and on the user's
   *  per-game override, and a label contradicting what launches is worse than
   *  none. */
  const [engineInfo, setEngineInfo] = createSignal<GameEngineInfo | null>(null);
  /** Null until the backend answers. Guessing "Staging" in the meantime made
   *  every ECE game on Windows flash the wrong engine and the wrong note. */
  const runsUnderEce = () => engineInfo()?.uses_ece ?? isWindows;
  /** Which emulator will actually run the selected game. */
  const emulatorName = () => {
    const v = selected()?.dosbox_variant ?? props.game?.dosbox_variant;
    if (v === "x98") { return "DOSBox-X"; }
    if (v === "pcbox") { return "PCBox (not shipped)"; }
    if (v?.startsWith("86box")) { return "86Box"; }
    if (v?.startsWith("ece")) { return runsUnderEce() ? "DOSBox ECE" : "DOSBox Staging"; }
    return "DOSBox Staging";
  };
  /** Blocking progress for the shared support payload; `withEmulators` says
   *  whether the payload is also this platform's emulator source (Windows). */
  const supportProgressNote = (s: Win9xSupportStatus, withEmulators: boolean): PanelNote => {
    const what = withEmulators ? "OS images + emulators" : "OS images";
    const pct = Math.round(s.progress * 100);
    return {
      key: "win9x-support-progress",
      blocking: true,
      text: pct >= 100
        ? `Setting up the Windows 9x support files (${what})…`
        : `Downloading the Windows 9x support files (${what})… ${pct}%`,
    };
  };
  const supportFailedNote = (): PanelNote => ({
    key: "win9x-support-failed",
    blocking: true,
    text: "Setting up the Windows 9x support files failed - make sure the library drive has "
      + "enough free space, then restart Exodium to retry.",
  });
  /** The single note shown above the action bar, most actionable first:
   *  a launch that cannot work, then a feature that is missing, then what
   *  merely differs from a DOS game. Null when there is nothing to say. */
  const rawNote = (): PanelNote | null => {
    const v = selected()?.dosbox_variant ?? props.game?.dosbox_variant;
    if (v === "pcbox") {
      return {
        key: "pcbox",
        blocking: true,
        text: "This game needs PCBox, a Windows-only emulator Exodium does not ship yet - "
          + "launching it will fail for now.",
      };
    }
    if (win9xEngineMissing()) {
      // Same variant → pack mapping as the backend's emulator_pack_for_variant.
      const packId = v?.startsWith("86box") ? "86box" : v === "pcbox" ? null : "dosbox-x";
      const col = selected()?.torrent_source ?? props.game?.torrent_source ?? "eXoWin9x";
      const pack = packId
        ? (packsByCollection()[col] ?? []).find((p) => p.id === packId && p.available && !p.installed)
        : undefined;
      if (pack) {
        const job = activeJobs()[`${col}:${pack.id}`];
        if (job && !job.finished) {
          const pct = job.total_bytes > 0
            ? Math.round((job.downloaded_bytes / job.total_bytes) * 100)
            : 0;
          return {
            key: "engine-missing",
            blocking: true,
            text: job.phase === "extracting"
              ? `Installing ${pack.display_name}…`
              : `Downloading ${pack.display_name}… ${pct}%`,
          };
        }
        if (isOffline()) {
          return {
            key: "engine-missing",
            blocking: true,
            text: `This game needs ${emulatorName()}, which is not downloaded yet. `
              + "Go online (Settings → Network) to download it.",
          };
        }
        return {
          key: "engine-missing",
          blocking: true,
          text: `This game needs ${emulatorName()}, which is not downloaded yet.`,
          action: {
            label: `Download emulator (${formatBytes(pack.size_bytes)})`,
            onClick: () => {
              startContentPackInstall(col, pack.id, pack.display_name).catch((e) => {
                showToast(`Couldn't start the ${pack.display_name} download`, "error", {
                  detail: String(e),
                });
              });
            },
          },
        };
      }
      // No pack to offer. On Windows the engine comes out of the shared
      // support payload, so report THAT state instead of a bare "not found"
      // while the 2.5 GB is still on its way; elsewhere the advice below is
      // actionable right now and outranks watching a download that cannot
      // provide the emulator.
      const support = supportStatus();
      if (isWindows) {
        if (!support) { return null; } // still probing - don't flash "not found"
        if (support.phase === "failed") { return supportFailedNote(); }
        if (support.phase === "downloading") { return supportProgressNote(support, true); }
        if (support.phase === "missing" && !selectedInstalled()) {
          return {
            key: "win9x-support-size",
            text: `${emulatorName()} and the shared Windows 9x OS images download automatically `
              + `with this game${support.total_bytes ? ` (one-time ${formatBytes(support.total_bytes)})` : ""}.`,
          };
        }
        // "ready" with the emulator gone, or "missing" for an installed
        // game: a real fault, not a pending download.
        return {
          key: "engine-missing",
          blocking: true,
          text: "The emulator this game needs was not found in the Windows 9x support files "
            + "(eXo\\emulators inside your library folder). Restore that folder, or delete it "
            + "and download any Windows 9x game to fetch it again.",
        };
      }
      return {
        key: "engine-missing",
        blocking: true,
        text: v === "x98"
          ? "The emulator this game needs was not found on this system. Install DOSBox-X via "
            + "your package manager or Flatpak (com.dosbox_x.DOSBox-X)."
          : "The emulator this game needs was not found on this system. Re-run the installer "
            + "or place 86Box on your PATH.",
      };
    }
    // Engine resolves, but the shared payload may still be on its way (the
    // parent OS images are data every platform needs): without this, a game
    // that installed before the 2.5 GB finished shows Play and fails bare.
    const support = supportStatus();
    if (support?.phase === "failed") { return supportFailedNote(); }
    if (support?.phase === "downloading") { return supportProgressNote(support, false); }
    // The download button quotes the game's own size, but the FIRST Win9x
    // game also pulls the shared support payload - say so before the click,
    // or 520 MB quietly becomes 3 GB.
    if (support?.phase === "missing" && !selectedInstalled() && !selectedDownloading()) {
      return {
        key: "win9x-support-size",
        text: "Downloading this game also fetches the shared Windows 9x support files"
          + `${support.total_bytes ? ` (one-time ${formatBytes(support.total_bytes)})` : ""} - `
          + "every Windows 9x game uses them.",
      };
    }
    if (printingUnavailable()) {
      return {
        key: "printing",
        text: "This game can print to a (virtual) printer, which the bundled DOSBox Staging "
          + "does not support yet. The game runs, but its printing features are unavailable "
          + "for now.",
      };
    }
    const mp = mpInfo();
    if (mp?.multiplayer && mp.state === "needs_wired") {
      return {
        key: "mp-wired",
        text: "This game can play online, but that needs a wired network connection - a Wi-Fi "
          + "link cannot carry the emulated network card's own hardware address, on any "
          + "system. Single player works either way.",
      };
    }
    if (mp?.multiplayer && mp.state === "needs_permission") {
      return {
        key: "mp-permission",
        text: "This game can play online once you allow it in Settings → Network. Single "
          + "player works either way.",
      };
    }
    if (v?.startsWith("ece") && engineInfo() && !runsUnderEce()) {
      // Three ways to end up on Staging, and they are not the user's fault in
      // equal measure: an override they chose, a build that has not been
      // extracted yet (Windows), or a platform ECE was never built for.
      const chosen = engineInfo()?.ece_available === true;
      return {
        key: "ece",
        text: chosen
          ? "This game is tuned for DOSBox ECE, but you set it to run under DOSBox Staging - "
            + "the experience may vary slightly."
          : isWindows
            ? "This game is tuned for DOSBox ECE, which Exodium has not unpacked yet. It runs "
              + "under DOSBox Staging until then - the experience may vary slightly."
            : "This game is tuned for DOSBox ECE, which only exists on Windows. Exodium runs "
              + "it with DOSBox Staging - the experience may vary slightly.",
      };
    }
    if (v === "x98") {
      return {
        key: "x98-boot",
        text: "This game boots Windows 98 inside DOSBox-X - the first start takes noticeably "
          + "longer than a DOS game.",
      };
    }
    if (v?.startsWith("86box")) {
      return {
        key: "86box-perf",
        text: "This game runs under 86Box, a full PC hardware emulator - startup is slower and "
          + "the system requirements are higher than for other games.",
      };
    }
    // Last, because it is the least about THIS game: without a working
    // GStreamer audio sink the webview freezes the moment a video mounts, so
    // previews are disabled wholesale and this says why.
    if (videoPlaybackUnsupported()) {
      return {
        key: "no-gstreamer",
        text: "Preview videos are turned off: this system is missing GStreamer plugins. "
          + "Install gstreamer1.0-plugins-good and gstreamer1.0-libav (names vary by "
          + "distribution), then restart Exodium.",
      };
    }
    return null;
  };

  /** The note actually rendered. Dismissal is remembered per note KIND, so
   *  answering "tuned for ECE" once silences it on all ~2,000 ECE titles -
   *  per game it would be busywork rather than a setting. */
  const note = (): PanelNote | null => {
    const n = rawNote();
    if (!n || n.blocking) { return n; }
    return dismissedNotesLoaded() && !isNoteDismissed(n.key) ? n : null;
  };
  let heroVideoRef: HTMLVideoElement | undefined;
  /** Whether the overflow control has anything to offer. GameActionsMenu gates
   *  every entry on the game's own state, so without this the button could
   *  open an empty popup - which reads as broken. Mirrors that gating:
   *  Settings and Reset need an install, Uninstall also accepts in_library,
   *  and Playlist only needs an id. */
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
  // The panel always describes exactly ONE row. Multi-language cards are a
  // merged group, so the user picks which language everything below the title
  // refers to - actions, description, manual, screenshots. Before this, the
  // header described the EN row while the Versions list acted on variant rows,
  // and nothing said which one the description or the manual belonged to.
  const [selectedId, setSelectedId] = createSignal<number | null>(null);
  let launchTimer: number | undefined;
  onCleanup(() => { if (launchTimer) { clearTimeout(launchTimer); } });

  const langEntries = () => props.game ? parseLangEntries(props.game) : [];
  const isMultiLang = () => langEntries().length > 1;


  // ── Selected variant ───────────────────────────────────────────────────
  // Single-language games have exactly one row (the game itself), so every
  // rule below collapses to it - the panel has ONE rendering path, which is
  // what previously drifted apart (the Manual button existed only on the
  // single-language branch).
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

  /** Which row's description we're showing, and whether that's a fallback.
   *  Only 98 of 648 German rows have their own text; Polish and Spanish have
   *  none - so saying "English text, no German available" beats silently
   *  showing English under a DE badge. */
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
  // Download progress used to be echoed here too; the action bar now renders
  // it for the selected variant and the chips show it for the others, so this
  // line is only for launch/uninstall messages.
  const currentStatus = () => status();


  // ── Settle gate ──────────────────────────────────────────────────────────
  // The panel slides in over 260ms, and opening it also kicks off the metadata
  // scan (a strip of thumbnails to decode) plus the Win9x probes - all landing
  // inside that window. Linux may paint the animation on WebKit's fallback
  // renderer (CLAUDE.md §17), where that burst is visible as a stutter, so the
  // work that is neither cheap nor layout-defining waits for the slide-in to
  // end. `animationend` on the panel is the signal; the timeout covers the
  // cases where it never fires (reduced motion, a hidden window). Variants and
  // the game's own fields stay immediate - they decide the panel's layout, and
  // holding them back would only trade a stutter for a visible pop-in.
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

  // Reset media state only when the DISPLAYED GAME changes - background
  // library refreshes (install/uninstall completing) replace the game object
  // with a fresh one for the same id, and resetting on those made the cover
  // image and media strip flicker on every state change.
  let lastGameId: number | null | undefined = undefined;
  let lastMetaKey: string | null = null;
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
    setVideoPlaying(false);
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
    setWin9xEngineMissing(false);
    setMpInfo(null);
    if (isWin9x(g)) {
      const id = g.id;
      win9xEngineAvailable(g.dosbox_variant ?? null)
        .then((ok) => { if (props.game?.id === id) { setWin9xEngineMissing(!ok); } })
        .catch(() => {});
      if (id != null) {
        win9xMultiplayerInfo(id)
          .then((info) => { if (props.game?.id === id) { setMpInfo(info); } })
          .catch(() => {});
      }
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

  // Re-probe the engine when a pack install lands (installedPacks changes):
  // the "downloading emulator" note must clear itself without the panel being
  // closed and reopened.
  createEffect(() => {
    installedPacks();
    if (!panelSettled()) { return; }
    const g = props.game;
    if (!isWin9x(g)) { return; }
    const id = g?.id;
    win9xEngineAvailable(g?.dosbox_variant ?? null)
      .then((ok) => { if (props.game?.id === id) { setWin9xEngineMissing(!ok); } })
      .catch(() => {});
  });

  // Watch the shared support payload for every open Win9x game: an
  // uninstalled one shows what the one-time download costs, a fetching one
  // shows live progress (on Windows the emulators ARE the support files).
  // Deliberately NOT gated on win9xEngineMissing()/selectedInstalled() -
  // the progress matters most when the game installed first and Play would
  // otherwise fail bare, and reading selectedInstalled() here would couple
  // the poller to the downloads store, which replaces its record every
  // second during any download. Once "ready" arrives the engine is
  // re-probed so a pending note clears without the panel being reopened;
  // "failed" is terminal until a restart re-arms the watcher.
  createEffect(() => {
    const g = props.game;
    if (!isWin9x(g) || isOffline()) { setSupportStatus(null); return; }
    // Polling can wait for the slide-in - the note it feeds is secondary.
    if (!panelSettled()) { return; }
    const variant = g?.dosbox_variant ?? null;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const probe = () => {
      getWin9xSupportStatus(variant)
        .then((s) => {
          if (cancelled) { return; }
          setSupportStatus(s);
          if (s.phase === "failed") { return; }
          if (s.phase === "ready") {
            if (win9xEngineMissing()) {
              const id = props.game?.id;
              win9xEngineAvailable(variant)
                .then((ok) => {
                  if (!cancelled && props.game?.id === id) { setWin9xEngineMissing(!ok); }
                })
                .catch(() => {});
            }
            return;
          }
          // Active download wants a live bar; a steady "missing" only needs
          // to notice a download started elsewhere eventually.
          timer = setTimeout(probe, s.phase === "downloading" ? 3000 : 10000);
        })
        .catch(() => {});
    };
    probe();
    onCleanup(() => {
      cancelled = true;
      if (timer != null) { clearTimeout(timer); }
    });
  });

  // Metadata (screenshots + manual) belongs to the SELECTED variant, not to
  // the group: an LP metadata pack can ship its own screenshots, and the
  // manual differs per language where one exists. Keyed on id+source+manual so
  // the background variant refresh (same rows, new objects) doesn't refetch.
  createEffect(() => {
    const v = selected();
    const row = manualRow();
    if (!v?.title || !v.torrent_source) { return; }
    const key = `${v.id}:${v.torrent_source}:${row?.manual_path ?? ""}`;
    if (key === lastMetaKey) { return; }
    // Held until the slide-in ends: the scan returns a strip of thumbnails to
    // decode, which is the heaviest thing an open kicks off. The loading flag
    // is set anyway, so the Manual button looks exactly as it did before -
    // busy from the first frame rather than briefly inert.
    if (!panelSettled()) { setMetadataLoading(true); return; }
    lastMetaKey = key;
    setMetadata(null);
    setBrokenImages(new Set<number>());
    // The previous variant's cover may have 404'd; the new one gets a fresh
    // chance rather than inheriting the placeholder. (The walk itself resets
    // via the keyed effect above - doing it here too restarted a completed
    // fallback at settle time.)
    setImgError(false);
    setMetadataLoading(true);
    loadGameMetadata(v.torrent_source, v.title, v.shortcode ?? null, row?.manual_path ?? null)
      .then((m) => { if (selected()?.id === v.id) { setMetadata(m); } })
      .finally(() => setMetadataLoading(false));
  });

  // Refresh variant list when a download transitions to installed so
  // badges/buttons stay current. Tracks per-id transitions via the store's
  // `installed` flag (its documented contract - status text keeps changing
  // during the extras phase, and re-matching it on every poll tick both
  // missed that phase and refetched variants redundantly).
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
    if (!g?.shortcode || !isMultiLang()) { return; }
    const shortcode = g.shortcode;
    loadVariants(g, true).then((v) => {
      if (props.game?.shortcode === shortcode) { setVariants(v); }
    }).catch(() => {});
  });

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") { return; }
    // A stacked overlay handles this Escape itself - closing the panel
    // underneath in the same press would yank the user two levels at once.
    // The actions menu counts: it stays mounted while the dialogs it opened
    // are up, so one signal covers menu, playlist and game settings.
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

  // LP rows usually inherit the EN thumbnail_key, but some carry a key with no
  // file behind it (own-title hash from an old DB). A null key falls through to
  // the primary row's candidates; a WRONG key only reveals itself as a 404, so
  // the <img onError> walks this list instead of giving up on the first miss.
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
    startGameDownload(gameId, title ?? props.game?.title);
  };

  // ── Preview video ──────────────────────────────────────────────────────
  const videoState = () => {
    const id = selected()?.id;
    videos(); // subscribe
    return id != null ? getVideoState(id) : undefined;
  };
  const videoReady = () => videoState()?.phase === "ready" && !!videoState()?.path;
  // Finding out whether a game has a video means reading the archive index over
  // the torrent, which can take tens of seconds. Staying silent through that
  // just looks broken, so each stage says what it is - including the negative
  // answer, which then fades out rather than lingering.
  const videoConfirmed = () => (videoState()?.total_bytes ?? 0) > 0;
  const videoProbing = () => videoState()?.phase === PHASE_PROBING;
  const videoFetching = () => videoState()?.phase === "fetching" && videoConfirmed();
  const videoQueued = () => videoState()?.phase === PHASE_QUEUED;
  const videoFailed = () => videoState()?.phase === "error";

  // There is deliberately no "no video for this game" pill. Most DOS titles
  // have no trailer, so the honest answer is also the useless one: it told the
  // reader nothing they could act on and drew the eye to the absence of a
  // feature. The progress states below still speak, because those describe
  // work in flight the reader is waiting on.
  // WebKitGTK cannot play media through the asset protocol (Linux answers a
  // localhost HTTP URL from media_url; macOS/Windows answer null and keep
  // convertFileSrc). Async, so the URL lives in a signal the effect fills.
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
  // Same beat as the video: the archive is asked a moment after the panel
  // settles. With autoplay on the theme becomes the player's wanted track;
  // off, it is only fetched so the row below can offer it.
  ensureMusicAutoplayLoaded();
  /** The theme belongs to the GROUP, not the selected variant: extras live in
   *  the EN archive only (every LP row has a NULL gamedata index, §14), so
   *  switching the language chip must neither restart nor re-request it. */
  const themeOwner = () => variants().find((v) => v.language === "EN") ?? props.game ?? selected();
  createEffect(() => {
    const g = themeOwner();
    const id = g?.id;
    if (g == null || id == null || musicUnsupported()) { return; }
    const timer = window.setTimeout(() => {
      // Already the active track (e.g. started from the Browse list, or the
      // owner resolved while playing): leave the player alone - re-issuing
      // playTheme would restart it and yank a list queue into theme mode.
      if (currentTrack()?.gameId === id || wantedTrack()?.gameId === id) { return; }
      if (musicAutoplay()) { playTheme(g); } else { void requestTheme(id); }
    }, 400);
    onCleanup(() => clearTimeout(timer));
  });
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

  // A preview with sound is the foreground; the music yields to it and comes
  // back when it ends. Switching games or closing the panel takes the video
  // away without an `ended`, so both withdraw the reason too.
  createEffect(on(() => selected()?.id, () => resumeFrom("video"), { defer: true }));
  createEffect(() => { if (!props.game) { resumeFrom("video"); } });
  onCleanup(() => resumeFrom("video"));

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

  // Autoplay as soon as it lands, with sound. Autoplay policies only grant
  // that once the document has seen a user gesture, and opening this panel is
  // one - but a preview is worth more than its audio, so a rejected unmuted
  // play retries muted rather than leaving the cover sitting there.
  // Latched on the row, NOT re-armed per effect run: `videoState()` reads the
  // whole videos store, which every background fetch rewrites once per poll.
  // Re-running the timer on those writes replayed a running preview from 0:00
  // every 700 ms (and, with the old fixed delay, postponed it forever).
  let autoplayTimer: number | undefined;
  let autoplayFor: number | null | undefined;
  onCleanup(() => { if (autoplayTimer) { clearTimeout(autoplayTimer); } });
  createEffect(() => {
    const id = selected()?.id;
    if (id !== autoplayFor) {
      // Row changed - drop a start still pending for the previous one.
      if (autoplayTimer) { clearTimeout(autoplayTimer); autoplayTimer = undefined; }
      autoplayFor = undefined;
    }
    if (!videoReady() || id == null || id === autoplayFor) { return; }
    autoplayFor = id;
    // Let the cover have the panel first. Opening a game and being met by a
    // trailer mid-motion reads as an ad; two seconds is long enough to take in
    // the box art, and the fade then belongs to the video rather than to the
    // panel opening. Cleared on close and on switching games.
    autoplayTimer = window.setTimeout(() => {
      autoplayTimer = undefined;
      const el = heroVideoRef;
      if (!el) { return; }
      try {
        el.currentTime = 0;
        el.muted = previewMuted();
        const started = el.play();
        // Older WebKit returns undefined here instead of a promise. Calling
        // .then on that throws INSIDE the effect, and Solid propagates the
        // exception back to whoever set the signal - which made the video
        // store record a fetch error for a video that had arrived fine.
        if (started && typeof started.then === "function") {
          started.then(() => setVideoPlaying(true)).catch(() => {
            // Autoplay with sound needs a user gesture the webview may not
            // have seen. A silent preview beats no preview - but do NOT write
            // that back to the preference: the user did not choose it.
            el.muted = true;
            const retry = el.play();
            if (retry && typeof retry.then === "function") {
              retry.then(() => setVideoPlaying(true)).catch(() => setVideoPlaying(false));
            } else {
              setVideoPlaying(true);
            }
          });
        } else {
          setVideoPlaying(true);
        }
      } catch {
        setVideoPlaying(false);
      }
    }, videoJustFetched() ? 0 : VIDEO_START_DELAY_MS);
  });

  /** Toggling mute mid-playback also un-mutes a fallback-muted video, since
   *  the click is itself the gesture autoplay was missing. */
  const toggleMute = () => {
    const next = !previewMuted();
    void setPreviewMuted(next);
    if (heroVideoRef) {
      heroVideoRef.muted = next;
      if (!next && heroVideoRef.paused) { void heroVideoRef.play(); }
      // Silent, the preview no longer needs the speakers; with sound, it does.
      if (next) { resumeFrom("video"); } else if (!heroVideoRef.paused) { pauseFor("video"); }
    }
  };

  // The lightbox plays the same preview with its own controls, and both have
  // sound now - so the hero has to step aside or the trailer runs twice over
  // itself. Pausing also cross-fades the cover back in, which is what should
  // be behind the lightbox anyway.
  createEffect(() => {
    if (lightboxOpen()) { heroVideoRef?.pause(); }
  });

  // ...and that pause hands the speakers back to the theme, while the lightbox
  // goes on playing the same trailer with sound from its OWN <video>. So the
  // reason is held here for as long as that one is the foreground. Entry 0 is
  // the video, and only the hero video's own click opens there - a lightbox
  // started on a screenshot is silent and must not silence the theme.
  const lightboxHoldsAudio = () =>
    lightboxOpen() && !!videoSrc() && !previewMuted() && lightboxStart() === 0;

  // Only a hold-to-no-hold transition withdraws the reason: an unconditional
  // resume would undo the hero's pauseFor the moment the mute preference
  // changes.
  createEffect((wasHolding: boolean) => {
    const holding = lightboxHoldsAudio();
    if (holding) { pauseFor("video"); } else if (wasHolding) { resumeFrom("video"); }
    return holding;
  }, false);

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
      setMpInfo(info);
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
      // One failure does NOT mean nothing is running: launch_game refuses a
      // second start of a live game with "'<title>' is already running."
      // (games.rs). That game's claim on the speakers belongs to the launch
      // that succeeded, and withdrawing it here started the theme over a
      // running emulator. Every other failure spawned nothing, so its claim
      // goes back.
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

  // Manual: shown iff the catalogue lists one for the selected variant or, as
  // a fallback, for the English row - in which case the label says so, because
  // "Manual" on a DE selection silently opening the English PDF is exactly the
  // ambiguity this panel is meant to remove. Unresolved = its GameData ZIP is
  // still downloading; clicking retries the lookup, so it self-heals.
  /** True once the file behind the catalogue's promise actually exists. */
  /** Favourite state of the CARD's row, not the selected variant.
   *
   *  The panel is scoped to one variant everywhere else (§12), but favourites
   *  are not: the grid stars `props.game`, and the Favorites shelf lists that
   *  row. Starring the DE variant here would leave the card it was opened from
   *  showing an empty star and put a second entry on the shelf. */
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

  // Shared "Play" button - same disabled+spinner UX whether it's the main
  // single-language action or one row of the multi-language variant list.
  const PlayButton = (p: { id: number; class?: string; disabled?: boolean }) => (
    <Button
      variant="action"
      class={p.class}
      onClick={() => handleLaunch(p.id)}
      disabled={p.disabled}
      loading={launchingId() === p.id}
      loadingLabel="Starting…"
    >
      ▶ Play
    </Button>
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
            <Show when={videoSrc()}>
              <video
                ref={heroVideoRef}
                class={`game-detail-hero-video${videoPlaying() ? " is-visible" : ""}`}
                src={videoSrc()!}
                playsinline
                preload="auto"
                onEnded={() => { setVideoPlaying(false); resumeFrom("video"); }}
                // A paused preview is not the foreground either: without this
                // the speakers stay claimed by a video nobody hears - opening
                // the lightbox pauses the hero, and closing it left silence.
                // Unless the lightbox is the one holding them: `pause()` only
                // QUEUES this event, so it lands after the effect above has
                // taken the reason over and would withdraw it again - the
                // theme then played over the lightbox's trailer.
                onPause={() => {
                  setVideoPlaying(false);
                  if (!lightboxHoldsAudio()) { resumeFrom("video"); }
                }}
                onPlay={(e) => { setVideoPlaying(true); if (!e.currentTarget.muted) { pauseFor("video"); } }}
                onClick={() => { setLightboxStart(0); setLightboxOpen(true); }}
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

            {/* A failed fetch must not look like "this game has no video" -
                a stalled torrent read is worth retrying, a missing video is not. */}
            <Show when={videoFailed()}>
              <button
                class="game-detail-video-status game-detail-video-retry"
                title={videoState()?.error ?? undefined}
                onClick={() => { const id = selected()?.id; if (id != null) { requestVideo(id); } }}
              >↻ Video retry</button>
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
                onClick={() => {
                  // A click is the gesture autoplay may have lacked, so a
                  // fallback-muted video gets its sound back here - but only
                  // if the user has not asked for silence. Forcing muted=false
                  // outright made Replay override the mute button.
                  if (heroVideoRef) { heroVideoRef.muted = previewMuted(); }
                  heroVideoRef?.play();
                }}
              >▶</button>
            </Show>
            </div>

            {/* Title, then who made it, then what kind of thing it is. The
                three sit in that order because that is the order they answer
                "what am I looking at" - and none of them competes with the
                description for width any more. */}
            <div class="game-detail-hero-info">
              <div class="game-detail-title">{selected()?.title ?? props.game!.title}</div>

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
            <Show when={note()}>
              {(n) => (
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
              )}
            </Show>

            {/* Language switcher: picking a chip re-points the whole panel -
                actions, description, manual and screenshots all follow it. */}
            <Show when={isMultiLang()}>
              <div class="variant-switcher" role="group" aria-label="Language versions">
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
                      <Show when={src().fallbackFrom}>
                        <div class="game-detail-fallback-note">
                          English description - the catalogue has no{" "}
                          {languageName(src().fallbackFrom)} text for this game.
                        </div>
                      </Show>
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
                      >↻ Theme retry</Button>
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
