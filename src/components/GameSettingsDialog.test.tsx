import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { GameSettingsDialog } from "./GameSettingsDialog";

const mockInvoke = vi.mocked(invoke);

const NO_SETTINGS = { glshader: null, fullscreen: null, cycles: null, custom_conf: null };

/** Two invokes resolve before the dialog settles; macrotask turns beat
 *  counting microtasks. */
async function flush() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

/** The dialog has several selects; find one by the label of its row. */
function selectFor(label: string): HTMLSelectElement | undefined {
  const rows = [...document.querySelectorAll(".game-settings-row")];
  const row = rows.find(
    (r) => r.querySelector(".game-settings-label")?.textContent?.trim() === label,
  );
  return row?.querySelector("select") ?? undefined;
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(
    () => (
      <GameSettingsDialog gameId={1} gameTitle="DOOM II: Hell on Earth" open onClose={() => {}} />
    ),
    host,
  );
  return { dispose };
}

/** The shader control is the one setting that silently does nothing under
 *  eXo's DOSBox ECE build (Windows). Offering it without saying so is what
 *  sent a user hunting for a config file that does not exist. */
describe("GameSettingsDialog", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("disables the shader control and explains why for an ECE game", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_settings") { return NO_SETTINGS; }
      if (cmd === "game_engine_info") { return { ece_available: true, uses_ece: true }; }
      return null;
    });
    const { dispose } = mount();
    await flush();

    expect(selectFor("CRT Shader")!.disabled).toBe(true);
    expect(selectFor("Emulator")).toBeDefined();
    expect(document.body.textContent).toMatch(/DOSBox ECE, which has no shader support/);
    dispose();
  });

  it("leaves the control alone when the game runs under Staging", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_settings") { return NO_SETTINGS; }
      if (cmd === "game_engine_info") { return { ece_available: false, uses_ece: false }; }
      return null;
    });
    const { dispose } = mount();
    await flush();

    expect(selectFor("CRT Shader")!.disabled).toBe(false);
    expect(selectFor("Emulator")).toBeUndefined();
    expect(document.querySelector(".game-settings-note")).toBeNull();
    dispose();
  });

  /** The two controls must not contradict each other: picking Staging has to
   *  take the "no shader support" warning away before saving, not after. */
  it("frees the shader control as soon as the emulator is switched", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_settings") { return NO_SETTINGS; }
      if (cmd === "game_engine_info") { return { ece_available: true, uses_ece: true }; }
      return null;
    });
    const { dispose } = mount();
    await flush();

    const engine = selectFor("Emulator")!;
    engine.value = "staging";
    engine.dispatchEvent(new Event("change"));
    await flush();

    expect(selectFor("CRT Shader")!.disabled).toBe(false);
    expect(document.body.textContent).not.toMatch(/no shader support/);
    dispose();
  });

  /** Regression: the control used to ask what ACTUALLY runs, which already had
   *  the override applied - so choosing Staging hid the row on the next open
   *  and there was no way back to eXo's choice. */
  it("keeps offering the choice once Staging is already stored", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_settings") {
        return { ...NO_SETTINGS, engine: "staging" };
      }
      if (cmd === "game_engine_info") { return { ece_available: true, uses_ece: false }; }
      return null;
    });
    const { dispose } = mount();
    await flush();

    const engine = selectFor("Emulator");
    expect(engine).toBeDefined();
    expect(engine!.value).toBe("staging");
    expect(selectFor("CRT Shader")!.disabled).toBe(false);
    dispose();
  });

  /** An eXoScummVM game has no DOSBox controls; the dialog shows the menus
   *  from the game's own variant tree and saves them as ScummVM options. */
  it("shows the ScummVM menus instead of the DOSBox controls", async () => {
    const tree = {
      variants: [
        { name: "Maniac Mansion (Amiga)", platform: "amiga", subs: [], sounds: [], has_subtitles: false },
        {
          name: "Maniac Mansion (DOS v1)", platform: null, sounds: ["Sound Blaster", "Tandy"], has_subtitles: false,
          subs: [
            { name: "Maniac Mansion CGA", sounds: [], has_subtitles: false },
            { name: "Maniac Mansion EGA", sounds: [], has_subtitles: true },
          ],
        },
      ],
      note: null,
      selected: { variant: "Maniac Mansion (DOS v1)", sub: "Maniac Mansion CGA", sound: "Sound Blaster", subtitles: false, aspect: true },
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_settings") { return NO_SETTINGS; }
      if (cmd === "game_engine_info") { return { ece_available: false, uses_ece: false }; }
      if (cmd === "scummvm_variants") { return tree; }
      return null;
    });
    const { dispose } = mount();
    await flush();

    expect(selectFor("CRT Shader")).toBeUndefined();
    expect(selectFor("CPU Cycles")).toBeUndefined();
    expect(selectFor("Fullscreen")).toBeDefined();
    const edition = selectFor("Edition")!;
    expect([...edition.options].map((o) => o.value)).toEqual(["Maniac Mansion CGA", "Maniac Mansion EGA"]);
    expect([...selectFor("Sound")!.options].map((o) => o.value)).toEqual(["Sound Blaster", "Tandy"]);
    expect(selectFor("Subtitles")).toBeUndefined();

    edition.value = "Maniac Mansion EGA";
    edition.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    expect(selectFor("Subtitles")).toBeDefined();

    const save = [...document.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Save")!;
    save.click();
    await flush();
    const call = mockInvoke.mock.calls.find(([cmd]) => cmd === "set_scummvm_options");
    expect(call?.[1]).toEqual({
      id: 1,
      options: { variant: "Maniac Mansion (DOS v1)", sub: "Maniac Mansion EGA", sound: "Sound Blaster", subtitles: false, aspect: true },
    });
    dispose();
  });
});
