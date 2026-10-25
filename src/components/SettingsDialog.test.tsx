import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import { SettingsDialog } from "./SettingsDialog";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

async function flush() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return render(
    () => (
      <SettingsDialog
        open
        onOpenChange={() => {}}
        section="general"
        onSectionChange={() => {}}
        gameFolderPath="/games"
        onChangeDataDir={() => {}}
        layoutSkipped={false}
        migrating={false}
        onMergeLayout={() => {}}
        onFactoryReset={() => {}}
        resetError=""
        onWentOnline={() => {}}
      />
    ),
    host,
  );
}

const previewSound = () => document.getElementById("preview-sound") as HTMLInputElement;

/** The speaker button on the preview and this switch are one preference
 *  (`preview_muted`), stored as "muted", shown as "sound". */
describe("SettingsDialog preview sound", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows the stored mute preference inverted and writes it back", async () => {
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_config" && (args as { key: string }).key === "preview_muted") { return "1"; }
      return null;
    });
    const dispose = mount();
    await flush();

    expect(previewSound().checked).toBe(false);

    previewSound().click();
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "preview_muted", value: "0" });
    expect(previewSound().checked).toBe(true);
    dispose();
  });
});

const keyboardLayout = () => document.getElementById("keyboard-layout") as HTMLSelectElement;

/** One global `keyboard_layout` key; empty means "leave the game's conf alone". */
describe("SettingsDialog keyboard layout", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows the stored layout and writes a new choice back", async () => {
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_config" && (args as { key: string }).key === "keyboard_layout") { return "gr"; }
      return null;
    });
    const dispose = mount();
    await flush();

    expect(keyboardLayout().value).toBe("gr");

    keyboardLayout().value = "fr";
    keyboardLayout().dispatchEvent(new Event("change"));
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "keyboard_layout", value: "fr" });
    dispose();
  });

  it("reads the stored layout on open and shows the game default when none is stored", async () => {
    mockInvoke.mockImplementation(async () => null);
    const dispose = mount();
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("get_config", { key: "keyboard_layout" });
    expect(keyboardLayout().value).toBe("");
    dispose();
  });
});

const midiGm = () => document.getElementById("midi-gm") as HTMLSelectElement;
const midiMt32 = () => document.getElementById("midi-mt32") as HTMLSelectElement;
const button = (id: string) => document.getElementById(id) as HTMLButtonElement;

/** Two outputs, stored as `port:<name>` or a synth id; the ROM folder is a
 *  plain path. A stored port that is unplugged stays selectable. */
describe("SettingsDialog MIDI outputs", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.mocked(openFolderDialog).mockReset();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  const stored: Record<string, string> = {
    midi_gm_output: "port:loopMIDI Port A",
    midi_mt32_output: "port:Roland UM-ONE",
    midi_rom_dir: "/Users/me/roms",
  };

  function mockStored(ports: string[]) {
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "list_midi_outputs") { return ports; }
      if (cmd === "get_config") { return stored[(args as { key: string }).key] ?? null; }
      return null;
    });
  }

  it("lists the host's ports as devices and keeps an unplugged stored one", async () => {
    mockStored(["loopMIDI Port A"]);
    const dispose = mount();
    await flush();

    const gmLabels = Array.from(midiGm().options).map((o) => o.textContent);
    expect(gmLabels).toEqual(["Standard", "FluidSynth", "Device: loopMIDI Port A"]);
    expect(midiGm().value).toBe("port:loopMIDI Port A");

    const mt32Labels = Array.from(midiMt32().options).map((o) => o.textContent);
    expect(mt32Labels).toEqual(["Standard", "Device: loopMIDI Port A", "Device: Roland UM-ONE"]);
    expect(midiMt32().value).toBe("port:Roland UM-ONE");
    dispose();
  });

  it("writes a new output back under its key", async () => {
    mockStored([]);
    const dispose = mount();
    await flush();

    midiGm().value = "fluidsynth";
    midiGm().dispatchEvent(new Event("change"));
    midiMt32().value = "";
    midiMt32().dispatchEvent(new Event("change"));
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "midi_gm_output", value: "fluidsynth" });
    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "midi_mt32_output", value: "" });
    dispose();
  });

  it("picks the ROM folder with the system dialog and clears it", async () => {
    mockStored([]);
    vi.mocked(openFolderDialog).mockResolvedValue("/Volumes/roms");
    const dispose = mount();
    await flush();

    expect(document.body.textContent).toContain("/Users/me/roms");
    button("midi-rom-change").click();
    await flush();
    expect(openFolderDialog).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "midi_rom_dir", value: "/Volumes/roms" });

    button("midi-rom-clear").click();
    await flush();
    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "midi_rom_dir", value: "" });
    expect(document.body.textContent).toContain("Not set");
    dispose();
  });
});
