import { describe, it, expect, vi } from "vitest";
import { launchNote, emulatorName, emulatorPackId, svmVariantLabel, type NoteContext } from "./launchNotes";
import type { Game, ContentPackStatus } from "./api/tauri";

const game = (over: Partial<Game> = {}): Game =>
  ({ id: 1, title: "T", torrent_source: "eXoDOS", dosbox_variant: null, ...over }) as unknown as Game;

const ctx = (over: Partial<NoteContext> = {}): NoteContext => ({
  game: game(),
  isWindows: false,
  offline: false,
  installed: false,
  downloading: false,
  svmEngine: null,
  svmNote: null,
  engineInfo: null,
  win9xEngineMissing: false,
  support: null,
  mp: null,
  printingUnavailable: false,
  videoUnsupported: false,
  emulatorPack: null,
  packJob: null,
  installPack: () => {},
  ...over,
});

const pack: ContentPackStatus = {
  id: "dosbox-x", display_name: "DOSBox-X", description: "", size_bytes: 50_000_000,
  version: 1, supersedes: [], available: true, installed: false,
};

const key = (c: NoteContext) => launchNote(c)?.key ?? null;

describe("launchNote", () => {
  it("says nothing for a plain DOS game", () => {
    expect(launchNote(ctx())).toBeNull();
  });

  describe("ScummVM", () => {
    const svm = (over: Partial<NoteContext> = {}) =>
      ctx({ game: game({ torrent_source: "eXoScummVM", dosbox_variant: "2.9.0" }), ...over });

    it("stays quiet while the engine probe is open", () => {
      expect(launchNote(svm())).toBeNull();
    });
    it("blocks when no ScummVM resolves, with the Flatpak hint off Windows", () => {
      const n = launchNote(svm({ svmEngine: { available: false, pinned_version: "2.9.0", source: null, pack_id: null } }));
      expect(n?.key).toBe("engine-missing");
      expect(n?.blocking).toBe(true);
      expect(n?.text).toContain("ScummVM 2.9.0");
      expect(n?.text).toContain("org.scummvm.ScummVM");
      const win = launchNote(svm({ isWindows: true, svmEngine: { available: false, pinned_version: "2.9.0", source: null, pack_id: null } }));
      expect(win?.text).not.toContain("Flatpak");
    });
    it("an uninstalled game states the engine's cost instead of a second download button", () => {
      const missing = { available: false, pinned_version: "2.8.0", source: null, pack_id: "scummvm-2.8.0" };
      const svmPack = { ...pack, id: "scummvm-2.8.0", display_name: "ScummVM 2.8.0", size_bytes: 127_745_114 };
      const n = launchNote(svm({ svmEngine: missing, emulatorPack: svmPack }));
      expect(n?.key).toBe("scummvm-engine-size");
      expect(n?.blocking).toBeUndefined();
      expect(n?.action).toBeUndefined();
      expect(n?.text).toContain("one-time 127.7 MB");
      // Installed, or already downloading: the blocking offer is the only fix.
      expect(key(svm({ svmEngine: missing, emulatorPack: svmPack, installed: true }))).toBe("engine-missing");
      expect(key(svm({ svmEngine: missing, emulatorPack: svmPack, downloading: true }))).toBe("engine-missing");
      // No pack to offer at all: the scummvm.org advice stands either way.
      expect(launchNote(svm({ svmEngine: missing }))?.text).toContain("scummvm.org");
    });
    it("offers the pinned build's pack instead of scummvm.org, and on a system ScummVM too", () => {
      const missing = { available: false, pinned_version: "2.9.0", source: null, pack_id: "scummvm-2.9.0" };
      const svmPack = { ...pack, id: "scummvm-2.9.0", display_name: "ScummVM 2.9.0" };
      const install = vi.fn();
      // Installed game: nothing else can fix it, so the offer is the note.
      const n = launchNote(svm({ svmEngine: missing, emulatorPack: svmPack, installed: true, installPack: install }));
      expect(n?.key).toBe("engine-missing");
      expect(n?.blocking).toBe(true);
      expect(n?.text).not.toContain("scummvm.org");
      n?.action?.onClick();
      expect(install).toHaveBeenCalledWith(svmPack);
      const job = { phase: "downloading", progress: 0.2, downloaded_bytes: 20, total_bytes: 100, finished: false, installed: false, error: null };
      expect(launchNote(svm({ svmEngine: missing, emulatorPack: svmPack, installed: true, packJob: job }))?.text).toBe("Downloading ScummVM 2.9.0… 20%");
      const system = { ...missing, available: true, source: "path" as const };
      const v = launchNote(svm({ svmEngine: system, emulatorPack: svmPack }));
      expect(v?.key).toBe("scummvm-version");
      expect(v?.blocking).toBe(false);
      expect(v?.text).toContain("eXo pins this game");
      expect(v?.action?.label).toBe("Download ScummVM 2.9.0 (50.0 MB)");
      expect(launchNote(svm({ svmEngine: system }))?.action).toBeUndefined();
    });
    it("eXo's note outranks the version warning, never the missing engine", () => {
      const ok = { available: true, pinned_version: "2.9.0", source: "path" as const, pack_id: null };
      const n = launchNote(svm({ svmEngine: ok, svmNote: "Audio is off in this port." }));
      expect(n?.key).toBe("svm-note");
      expect(n?.blocking).toBeUndefined();
      expect(key(svm({ svmEngine: { ...ok, available: false }, svmNote: "x" }))).toBe("engine-missing");
    });
    it("warns when a system ScummVM ignores the pin, not for eXo's or the pack's build", () => {
      expect(key(svm({ svmEngine: { available: true, pinned_version: "2.9.0", source: "path", pack_id: null } }))).toBe("scummvm-version");
      expect(key(svm({ svmEngine: { available: true, pinned_version: "2.9.0", source: "flatpak", pack_id: null } }))).toBe("scummvm-version");
      expect(key(svm({ svmEngine: { available: true, pinned_version: "2.9.0", source: "pack", pack_id: null } }))).toBeNull();
      expect(key(svm({ svmEngine: { available: true, pinned_version: "2.9.0", source: "exo", pack_id: null } }))).toBeNull();
    });
  });

  describe("Win9x engine missing", () => {
    const x98 = (over: Partial<NoteContext> = {}) =>
      ctx({ game: game({ torrent_source: "eXoWin9x", dosbox_variant: "x98" }), win9xEngineMissing: true, ...over });

    it("pcbox blocks regardless of anything else", () => {
      expect(key(ctx({ game: game({ dosbox_variant: "pcbox" }) }))).toBe("pcbox");
    });
    it("reports the running pack install with its phase", () => {
      const job = { phase: "downloading", progress: 0.4, downloaded_bytes: 40, total_bytes: 100, finished: false, installed: false, error: null };
      expect(launchNote(x98({ emulatorPack: pack, packJob: job }))?.text).toBe("Downloading DOSBox-X… 40%");
      expect(launchNote(x98({ emulatorPack: pack, packJob: { ...job, phase: "extracting" } }))?.text).toBe("Installing DOSBox-X…");
    });
    it("offers the pack download online and points at Settings offline", () => {
      const install = vi.fn();
      const n = launchNote(x98({ emulatorPack: pack, installPack: install }));
      expect(n?.blocking).toBe(true);
      expect(n?.action?.label).toBe("Download emulator (50.0 MB)");
      n?.action?.onClick();
      expect(install).toHaveBeenCalledWith(pack);
      const off = launchNote(x98({ emulatorPack: pack, offline: true }));
      expect(off?.action).toBeUndefined();
      expect(off?.text).toContain("Go online");
    });
    it("without a pack: Windows reports the support payload, Linux an install hint", () => {
      expect(launchNote(x98({ isWindows: true }))).toBeNull();
      expect(key(x98({ isWindows: true, support: { phase: "downloading", progress: 0.5, total_bytes: 1 } }))).toBe("win9x-support-progress");
      expect(key(x98({ isWindows: true, support: { phase: "failed", progress: 1, total_bytes: 1 } }))).toBe("win9x-support-failed");
      expect(key(x98({ isWindows: true, support: { phase: "missing", progress: 0, total_bytes: 1 } }))).toBe("win9x-support-size");
      expect(launchNote(x98({ isWindows: true, installed: true, support: { phase: "missing", progress: 0, total_bytes: 1 } }))?.text).toContain("Restore that folder");
      expect(launchNote(x98())?.text).toContain("com.dosbox_x.DOSBox-X");
      expect(launchNote(x98({ game: game({ dosbox_variant: "86box" }) }))?.text).toContain("86Box on your PATH");
    });
  });

  describe("Win9x engine present", () => {
    const x98 = (over: Partial<NoteContext> = {}) =>
      ctx({ game: game({ torrent_source: "eXoWin9x", dosbox_variant: "x98" }), ...over });

    it("announces the one-time payload before the first download only", () => {
      const missing = { phase: "missing" as const, progress: 0, total_bytes: 2_500_000_000 };
      const n = launchNote(x98({ support: missing }));
      expect(n?.key).toBe("win9x-support-size");
      expect(n?.text).toContain("one-time 2.5 GB");
      expect(key(x98({ support: missing, installed: true }))).toBe("x98-boot");
      expect(key(x98({ support: missing, downloading: true }))).toBe("x98-boot");
    });
    it("multiplayer notes outrank the boot note", () => {
      expect(key(x98({ mp: { multiplayer: true, state: "needs_wired", prompt: false } }))).toBe("mp-wired");
      expect(key(x98({ mp: { multiplayer: true, state: "needs_permission", prompt: false } }))).toBe("mp-permission");
      expect(key(x98({ mp: { multiplayer: true, state: "ready", prompt: false } }))).toBe("x98-boot");
      expect(key(x98({ game: game({ dosbox_variant: "86box" }) }))).toBe("86box-perf");
    });
  });

  describe("DOS", () => {
    const ece = (over: Partial<NoteContext> = {}) => ctx({ game: game({ dosbox_variant: "ece4230" }), ...over });

    it("printing outranks the ECE note", () => {
      expect(key(ece({ printingUnavailable: true, engineInfo: { ece_available: false, uses_ece: false } }))).toBe("printing");
    });
    it("explains Staging on an ECE game by cause", () => {
      expect(launchNote(ece())).toBeNull();
      expect(launchNote(ece({ engineInfo: { ece_available: true, uses_ece: true } }))).toBeNull();
      expect(launchNote(ece({ engineInfo: { ece_available: true, uses_ece: false } }))?.text).toContain("you set it");
      expect(launchNote(ece({ isWindows: true, engineInfo: { ece_available: false, uses_ece: false } }))?.text).toContain("not unpacked yet");
      expect(launchNote(ece({ engineInfo: { ece_available: false, uses_ece: false } }))?.text).toContain("only exists on Windows");
    });
    it("the GStreamer note comes last", () => {
      expect(key(ctx({ videoUnsupported: true }))).toBe("no-gstreamer");
      expect(key(ece({ videoUnsupported: true, engineInfo: { ece_available: false, uses_ece: false } }))).toBe("ece");
    });
  });
});

describe("svmVariantLabel", () => {
  it("keeps the parenthesised part, or the whole name", () => {
    expect(svmVariantLabel("Maniac Mansion (DOS v1)")).toBe("DOS v1");
    expect(svmVariantLabel("Maniac Mansion (NES, English, USA)")).toBe("NES, English, USA");
    expect(svmVariantLabel("Other Languages")).toBe("Other Languages");
  });
});

describe("emulatorName", () => {
  it("names the engine from the variant, ScummVM from the pin", () => {
    expect(emulatorName(game(), null, false)).toBe("DOSBox Staging");
    expect(emulatorName(game({ dosbox_variant: "ece4230" }), null, true)).toBe("DOSBox ECE");
    expect(emulatorName(game({ dosbox_variant: "ece4230" }), null, false)).toBe("DOSBox Staging");
    expect(emulatorName(game({ dosbox_variant: "x98" }), null, false)).toBe("DOSBox-X");
    expect(emulatorName(game({ dosbox_variant: "86boxME" }), null, false)).toBe("86Box");
    expect(emulatorName(game({ torrent_source: "eXoScummVM" }), null, false)).toBe("ScummVM");
    expect(emulatorName(game({ torrent_source: "eXoScummVM" }), { available: true, pinned_version: "2.8.0", source: "pack", pack_id: null }, false)).toBe("ScummVM 2.8.0");
  });
  it("maps variants to emulator packs like the backend", () => {
    expect(emulatorPackId("x98")).toBe("dosbox-x");
    expect(emulatorPackId("86boxME")).toBe("86box");
    expect(emulatorPackId("pcbox")).toBeNull();
  });
});
