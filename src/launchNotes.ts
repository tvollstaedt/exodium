import type {
  AppleIigsInfo, ContentPackStatus, Game, GameEngineInfo, IigsEngine, ScummVmEngineInfo, Win9xMultiplayerInfo,
  Win9xSupportStatus,
} from "./api/tauri";
import type { ContentPackJobState } from "./stores/contentPacks";
import { formatBytes } from "./util";

/** The one note the detail panel shows above the action bar. `blocking`
 *  notes describe a launch that cannot work and are never dismissable. */
export interface PanelNote {
  key: string;
  text: string;
  blocking?: boolean;
  /** The block is temporary and Exodium is working on it (a pack or support
   *  download in flight); Play shows a spinner instead of failing. */
  pending?: boolean;
  /** A remedy the app can perform itself, rendered as a button in the note. */
  action?: { label: string; onClick: () => void };
}

/** Everything the note logic reads, snapshotted from the panel's signals. */
export interface NoteContext {
  game: Game | null;
  isWindows: boolean;
  offline: boolean;
  installed: boolean;
  downloading: boolean;
  /** Null while the backend is still answering. */
  svmEngine: ScummVmEngineInfo | null;
  /** eXo's note.txt for an installed ScummVM game. */
  svmNote: string | null;
  /** Null while the backend is still answering, and for other collections. */
  iigs: AppleIigsInfo | null;
  engineInfo: GameEngineInfo | null;
  win9xEngineMissing: boolean;
  support: Win9xSupportStatus | null;
  mp: Win9xMultiplayerInfo | null;
  printingUnavailable: boolean;
  videoUnsupported: boolean;
  /** Content pack that could supply the missing Win9x emulator, if any. */
  emulatorPack: ContentPackStatus | null;
  /** That pack's running install, if any. */
  packJob: ContentPackJobState | null;
  installPack: (pack: ContentPackStatus) => void;
}

/** Win9x variant slugs only exist in the eXoWin9x catalogue, so they double
 *  as the collection test. */
export const isWin9x = (g: Game | null): boolean => {
  const v = g?.dosbox_variant;
  return v === "x98" || v === "pcbox" || (v?.startsWith("86box") ?? false);
};

/** eXoScummVM rows carry build names as variant slugs, so the collection id
 *  is the marker. */
export const isScummVm = (g: Game | null): boolean => g?.torrent_source === "eXoScummVM";

export const isAppleIigs = (g: Game | null): boolean => g?.torrent_source === "eXoAppleIIGS";

const IIGS_ENGINE_NAMES: Record<IigsEngine, string> = { gsplus: "GSplus", gsport: "GSport", mame: "MAME" };

/** "GSplus" on macOS and Linux, eXo's own pick on Windows. */
export function iigsEngineName(info: AppleIigsInfo | null, isWindows: boolean): string {
  const e = info?.engine ?? (info && isWindows ? info.exo_engine : "gsplus");
  return IIGS_ENGINE_NAMES[e];
}

/** The keys that differ per emulator, from eXo's manual and GSplus 1.38's
 *  readme: Open-Apple and Option, and the control panel a few games save and
 *  load through (Ctrl+Open-Apple+Esc on the real machine). */
export function iigsControls(info: AppleIigsInfo, isWindows: boolean): string {
  const engine = info.engine ?? (isWindows ? info.exo_engine : "gsplus");
  if (engine === "gsplus" && !isWindows) {
    return "F1 = Open-Apple, F2 = Option, F3 = Esc, F4 = settings and disk swap, "
      + "F11 = fullscreen, Ctrl+F12 = reset. Control panel: Ctrl+F1+F3.";
  }
  if (engine === "gsplus") {
    return "F1 or Alt = Open-Apple, F2 or Windows key = Option, F11 = fullscreen, F12 = reset. "
      + "Control panel: hold Shift+Esc, then press left Ctrl and left Alt.";
  }
  return "F1 or left Alt = Open-Apple, F2 or right Alt = Option, F3 = Esc, F12 = reset. "
    + "Control panel: left Ctrl+left Alt+F3.";
}

/** What will actually run the game; the backend's own resolver decides
 *  ECE vs Staging (`runsUnderEce`), the variant slug the rest. */
export function emulatorName(
  g: Game | null,
  svmEngine: ScummVmEngineInfo | null,
  runsUnderEce: boolean,
  iigs: AppleIigsInfo | null = null,
  isWindows = false,
): string {
  if (isAppleIigs(g)) { return iigsEngineName(iigs, isWindows); }
  if (isScummVm(g)) {
    const pinned = svmEngine?.pinned_version;
    return pinned ? `ScummVM ${pinned}` : "ScummVM";
  }
  const v = g?.dosbox_variant;
  if (v === "x98") { return "DOSBox-X"; }
  if (v === "pcbox") { return "PCBox (not shipped)"; }
  if (v?.startsWith("86box")) { return "86Box"; }
  if (v?.startsWith("ece")) { return runsUnderEce ? "DOSBox ECE" : "DOSBox Staging"; }
  return "DOSBox Staging";
}

/** The part of a ScummVM variant folder worth a chip: "Maniac Mansion
 *  (DOS v1)" -> "DOS v1"; a name without parentheses stays whole. */
export function svmVariantLabel(name: string): string {
  const m = /\(([^()]+)\)\s*$/.exec(name);
  return m ? m[1] : name;
}

/** Backend's `emulator_pack_for_variant`, mirrored. */
export function emulatorPackId(variant: string | null | undefined): string | null {
  if (variant?.startsWith("86box")) { return "86box"; }
  if (variant === "pcbox") { return null; }
  return "dosbox-x";
}

type SupportFamily = "Windows 9x" | "ScummVM" | "Apple IIGS";
const familyKey = (family: SupportFamily) =>
  family === "ScummVM" ? "scummvm" : family === "Apple IIGS" ? "iigs" : "win9x";

const supportProgressNote = (s: Win9xSupportStatus, family: SupportFamily, what: string): PanelNote => {
  const pct = Math.round(s.progress * 100);
  return {
    key: `${familyKey(family)}-support-progress`,
    blocking: true,
    pending: true,
    text: pct >= 100
      ? `Setting up the ${family} support files (${what})…`
      : `Downloading the ${family} support files (${what})… ${pct}%`,
  };
};

const supportFailedNote = (family: SupportFamily): PanelNote => ({
  key: `${familyKey(family)}-support-failed`,
  blocking: true,
  text: `Setting up the ${family} support files failed - make sure the library drive has `
    + "enough free space, then restart Exodium to retry.",
});

const oneTime = (bytes: number) => (bytes ? ` (one-time ${formatBytes(bytes)})` : "");

/** The pack that supplies the missing emulator, as a note: its running
 *  install, the offline case, or the download button. Null without a pack. */
function packRemedy(ctx: NoteContext, key: string, engine: string, blocking: boolean): PanelNote | null {
  const pack = ctx.emulatorPack;
  if (!pack) { return null; }
  const job = ctx.packJob;
  if (job && !job.finished) {
    const pct = job.total_bytes > 0 ? Math.round((job.downloaded_bytes / job.total_bytes) * 100) : 0;
    return {
      key,
      blocking,
      pending: true,
      text: job.phase === "extracting"
        ? `Installing ${pack.display_name}…`
        : `Downloading ${pack.display_name}… ${pct}%`,
    };
  }
  if (ctx.offline) {
    return {
      key,
      blocking,
      text: `This game needs ${engine}, which is not downloaded yet. `
        + "Go online (Settings → Network) to download it.",
    };
  }
  return {
    key,
    blocking,
    text: `This game needs ${engine}, which is not downloaded yet.`,
    action: {
      label: `Download emulator (${formatBytes(pack.size_bytes)})`,
      onClick: () => ctx.installPack(pack),
    },
  };
}

function scummVmNote(ctx: NoteContext, engine: string): PanelNote | null {
  const e = ctx.svmEngine;
  if (!e) { return null; }
  if (!e.available) {
    // The game's own Download button already fetches the engine with it
    // (download_game queues the pack), so an uninstalled game gets the price
    // rather than a second, competing action next to it.
    if (!ctx.installed && !ctx.downloading && ctx.emulatorPack) {
      return {
        key: "scummvm-engine-size",
        text: `Downloading this game also fetches ${engine}`
          + `${oneTime(ctx.emulatorPack.size_bytes)} - every game eXo pins to that build uses it.`,
      };
    }
    // Windows has no pack: eXo's own builds come out of utilSVM.zip, which
    // download_game queues with the game. The note follows that payload.
    if (ctx.isWindows && ctx.offline) {
      return {
        key: "engine-missing",
        blocking: true,
        text: `This game needs eXo's ${engine}, which downloads with the game. `
          + "Go online (Settings → Network) to fetch it.",
      };
    }
    if (ctx.isWindows && ctx.support) {
      const support = ctx.support;
      if (support.phase === "failed") { return supportFailedNote("ScummVM"); }
      if (support.phase === "downloading") { return supportProgressNote(support, "ScummVM", engine); }
      if (support.phase === "missing" && !ctx.installed) {
        return {
          key: "scummvm-engine-size",
          text: `Downloading this game also fetches eXo's ${engine}`
            + `${oneTime(support.total_bytes)} - every game eXo pins to that build uses it.`,
        };
      }
      return {
        key: "engine-missing",
        blocking: true,
        text: `${engine} was not found in the ScummVM support files (eXo\\emulators\\scmvm `
          + "inside your library folder). Restore that folder, or delete it and download "
          + "any ScummVM game to fetch it again.",
      };
    }
    return packRemedy(ctx, "engine-missing", engine, true) ?? {
      key: "engine-missing",
      blocking: true,
      text: `This game runs under ${engine}, which was not found on this system. `
        + "Install ScummVM from scummvm.org"
        + (ctx.isWindows ? "" : " (Linux: the Flatpak org.scummvm.ScummVM works too)")
        + " and try again.",
    };
  }
  if (ctx.svmNote) {
    return { key: "svm-note", text: ctx.svmNote };
  }
  if (e.source === "path" || e.source === "flatpak") {
    // A system ScummVM runs the game, but not the build eXo tested it with;
    // the pack fixes that, so the note offers it rather than only warning.
    const remedy = packRemedy(ctx, "scummvm-version", engine, false);
    const text = `eXo pins this game to ${engine}. Your system's ScummVM will run it `
      + "instead, which may behave differently.";
    if (remedy?.action) {
      return { ...remedy, text, action: { ...remedy.action, label: `Download ${engine} (${formatBytes(ctx.emulatorPack!.size_bytes)})` } };
    }
    return remedy ?? { key: "scummvm-version", text };
  }
  return null;
}

const SUBSTITUTE_TEXT: Record<string, (exo: string, note: string | null) => string> = {
  untested: (exo) => `eXo runs this game with ${exo}. Here GSplus runs it, and that has not `
    + "been tested yet.",
  issues: (exo, note) => `eXo runs this game with ${exo}. Under GSplus it runs with problems`
    + (note ? `: ${note}` : "."),
  broken: (exo, note) => `eXo runs this game with ${exo}, and under GSplus it does not run `
    + `properly${note ? `: ${note}` : "."} On Windows Exodium uses eXo's ${exo}.`,
};

/** An eXoAppleIIGS game: the engine and the ROMs come first (Play cannot
 *  work without them), then one note that carries what eXo says about the
 *  game plus what differs from eXo here. */
function appleIigsNote(ctx: NoteContext): PanelNote | null {
  const info = ctx.iigs;
  if (!info) { return null; }
  const engine = iigsEngineName(info, ctx.isWindows);
  if (info.engine == null) {
    if (!ctx.isWindows) {
      if (!ctx.installed && !ctx.downloading && ctx.emulatorPack) {
        return {
          key: "iigs-engine-size",
          text: `Downloading this game also fetches GSplus${oneTime(ctx.emulatorPack.size_bytes)}, `
            + "which runs every Apple IIGS game.",
        };
      }
      return packRemedy(ctx, "engine-missing", "GSplus", true) ?? {
        key: "engine-missing",
        blocking: true,
        text: "This game needs GSplus, which was not found. Install GSplus 1.38 "
          + "(github.com/digarok/gsplus) so that gsplus is on your PATH, then try again.",
      };
    }
  }
  const support = ctx.support;
  if (support?.phase === "failed") { return supportFailedNote("Apple IIGS"); }
  if (support?.phase === "downloading") {
    return supportProgressNote(support, "Apple IIGS", ctx.isWindows ? "ROMs + emulators" : "ROMs");
  }
  if (support?.phase === "missing" && !ctx.downloading) {
    return ctx.installed
      ? {
        key: "iigs-support-missing",
        text: "The Apple IIGS support files (ROMs) are not on disk yet. Play fetches them first"
          + `${oneTime(support.total_bytes)}.`,
      }
      : {
        key: "iigs-support-size",
        text: "Downloading this game also fetches the Apple IIGS support files"
          + `${oneTime(support.total_bytes)} - every Apple IIGS game uses them.`,
      };
  }
  if (info.engine == null) {
    return {
      key: "engine-missing",
      blocking: true,
      text: `${engine} was not found in the Apple IIGS support files (eXo\\emulators inside `
        + "your library folder). Press Play to set them up again.",
    };
  }
  const parts: string[] = [];
  const sub = info.substitute && info.substitute !== "ok" ? SUBSTITUTE_TEXT[info.substitute] : null;
  if (sub) { parts.push(sub(IIGS_ENGINE_NAMES[info.exo_engine], info.substitute_note)); }
  if (info.note) { parts.push(info.note); }
  if (info.capslock) { parts.push("eXo starts this game with Caps Lock on - turn it on before you play."); }
  if (info.printing_unavailable) {
    parts.push("Printing only works on Windows, where eXo's GSplus 0.14 prints into the game's folder.");
  } else if (info.printing) {
    parts.push("Printouts are saved in the game's folder inside your library.");
  }
  if (parts.length === 0) { return null; }
  return { key: `iigs:${ctx.game?.shortcode ?? ""}`, text: parts.join("\n") };
}

/** The Win9x emulator does not resolve. A pack that can supply it is the
 *  remedy; otherwise Windows reports the shared payload's state (the engine
 *  comes out of it), and elsewhere the advice is an install hint. */
function engineMissingNote(ctx: NoteContext, engine: string): PanelNote | null {
  const v = ctx.game?.dosbox_variant;
  const remedy = packRemedy(ctx, "engine-missing", engine, true);
  if (remedy) { return remedy; }
  if (ctx.isWindows) {
    const support = ctx.support;
    if (!support) { return null; }
    if (support.phase === "failed") { return supportFailedNote("Windows 9x"); }
    if (support.phase === "downloading") { return supportProgressNote(support, "Windows 9x", "OS images + emulators"); }
    if (support.phase === "missing" && !ctx.installed) {
      return {
        key: "win9x-support-size",
        text: `${engine} and the shared Windows 9x OS images download automatically `
          + `with this game${oneTime(support.total_bytes)}.`,
      };
    }
    // "ready" with the emulator gone, or "missing" for an installed game.
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

/** Most actionable first: a launch that cannot work, then a missing feature,
 *  then what merely differs from a DOS game. Null when there is nothing to
 *  say. */
export function launchNote(ctx: NoteContext): PanelNote | null {
  const g = ctx.game;
  const v = g?.dosbox_variant;
  const runsUnderEce = ctx.engineInfo?.uses_ece ?? ctx.isWindows;
  const engine = emulatorName(g, ctx.svmEngine, runsUnderEce);

  // This localized variant is a patch; the English game installs with it and
  // stays playable on its own. Informational, and only while it would still
  // cost something - once installed the note has nothing left to announce.
  if (g?.requires_base && !ctx.installed && !ctx.downloading) {
    return {
      key: "lp-requires-base",
      text: "This translation is a patch for the English game, so the English "
        + "version downloads and installs with it. Both appear in My Games.",
    };
  }
  // The English row of a group whose translation needed it. Removing it is
  // safe - the translation owns a real copy - and saying so is the only
  // place that relationship is visible.
  if (g?.installed_with && ctx.installed) {
    return {
      key: "lp-installed-with",
      text: `Installed together with ${g.installed_with}, which needed it. `
        + "Removing this one keeps that version working - it has its own copy.",
    };
  }
  if (isScummVm(g)) { return scummVmNote(ctx, engine); }
  if (isAppleIigs(g)) { return appleIigsNote(ctx); }
  if (v === "pcbox") {
    return {
      key: "pcbox",
      blocking: true,
      text: "This game needs PCBox, a Windows-only emulator Exodium does not ship yet - "
        + "launching it will fail for now.",
    };
  }
  if (ctx.win9xEngineMissing) { return engineMissingNote(ctx, engine); }

  // Engine resolves, but the shared payload (parent OS images, needed on
  // every platform) may still be on its way.
  const support = ctx.support;
  if (support?.phase === "failed") { return supportFailedNote("Windows 9x"); }
  // On Windows the payload is also where the emulators come from.
  if (support?.phase === "downloading") {
    return supportProgressNote(support, "Windows 9x", ctx.isWindows ? "OS images + emulators" : "OS images");
  }
  if (support?.phase === "missing" && !ctx.installed && !ctx.downloading) {
    return {
      key: "win9x-support-size",
      text: "Downloading this game also fetches the shared Windows 9x support files"
        + `${oneTime(support.total_bytes)} - every Windows 9x game uses them.`,
    };
  }
  if (ctx.printingUnavailable) {
    return {
      key: "printing",
      text: "This game can print to a (virtual) printer, which the bundled DOSBox Staging "
        + "does not support yet. The game runs, but its printing features are unavailable "
        + "for now.",
    };
  }
  const mp = ctx.mp;
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
  if (v?.startsWith("ece") && ctx.engineInfo && !runsUnderEce) {
    // An override the user chose, a build not yet extracted (Windows), or a
    // platform ECE was never built for.
    const chosen = ctx.engineInfo.ece_available === true;
    return {
      key: "ece",
      text: chosen
        ? "This game is tuned for DOSBox ECE, but you set it to run under DOSBox Staging - "
          + "the experience may vary slightly."
        : ctx.isWindows
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
  // Last, because it is the least about THIS game.
  if (ctx.videoUnsupported) {
    return {
      key: "no-gstreamer",
      text: "Preview videos are turned off: this system is missing GStreamer plugins. "
        + "Install gstreamer1.0-plugins-good and gstreamer1.0-libav (names vary by "
        + "distribution), then restart Exodium.",
    };
  }
  return null;
}
