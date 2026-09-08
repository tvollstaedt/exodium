import type {
  ContentPackStatus, Game, GameEngineInfo, ScummVmEngineInfo, Win9xMultiplayerInfo, Win9xSupportStatus,
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

/** What will actually run the game; the backend's own resolver decides
 *  ECE vs Staging (`runsUnderEce`), the variant slug the rest. */
export function emulatorName(g: Game | null, svmEngine: ScummVmEngineInfo | null, runsUnderEce: boolean): string {
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

const supportProgressNote = (s: Win9xSupportStatus, family: "Windows 9x" | "ScummVM", what: string): PanelNote => {
  const pct = Math.round(s.progress * 100);
  return {
    key: `${family === "ScummVM" ? "scummvm" : "win9x"}-support-progress`,
    blocking: true,
    pending: true,
    text: pct >= 100
      ? `Setting up the ${family} support files (${what})…`
      : `Downloading the ${family} support files (${what})… ${pct}%`,
  };
};

const supportFailedNote = (family: "Windows 9x" | "ScummVM"): PanelNote => ({
  key: `${family === "ScummVM" ? "scummvm" : "win9x"}-support-failed`,
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

  if (isScummVm(g)) { return scummVmNote(ctx, engine); }
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
