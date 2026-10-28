import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { KidsStatus, Playlist } from "../api/tauri";
import { KidsSettings } from "./KidsSettings";
import { SettingsDialog } from "./SettingsDialog";
import { kidsLocked, requestKidsPlaylist, setKidsStatus } from "../stores/kids";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const mockInvoke = vi.mocked(invoke);

async function flush() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

const PLAYLISTS: Playlist[] = [
  { id: 1, name: "Rainy days", kind: "user", description: null, game_count: 3 },
  { id: 2, name: "Bedtime", kind: "user", description: null, game_count: 2 },
];

const OFF: KidsStatus = { enabled: false, locked: false, playlist_id: null, language: "EN", retry_in: 0, pin_length: null };
const ON: KidsStatus = { enabled: true, locked: false, playlist_id: 1, language: "EN", retry_in: 0, pin_length: 4 };

/** Answers like the backend with kids mode in `state`; `pinOk` decides every
 *  PIN check, a wrong one costing `wait` seconds. */
function backend(state: KidsStatus, pinOk = true, wait = 0) {
  let current = state;
  mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    switch (cmd) {
      case "get_playlists": return PLAYLISTS;
      case "kids_status": return current;
      case "kids_update": return { ...current, playlist_id: (args as { playlistId: number }).playlistId };
      case "kids_enable": {
        const a = args as { playlistId: number | null; language: string };
        current = { ...current, enabled: true, playlist_id: a.playlistId ?? 9, language: a.language };
        return current;
      }
      case "kids_lock": current = { ...current, locked: current.enabled }; return current;
      case "kids_change_pin": return { ok: pinOk, retry_in: pinOk ? 0 : wait };
      case "kids_disable":
        if (pinOk) { current = OFF; }
        return { ok: pinOk, retry_in: pinOk ? 0 : wait };
      default: return null;
    }
  });
}

function mount(onLock: () => void = () => {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return render(() => <KidsSettings onLock={onLock} />, host);
}

const byTestId = <T extends HTMLElement = HTMLElement>(id: string) => document.querySelector<T>(`[data-testid=${id}]`);
const click = (id: string) => byTestId<HTMLButtonElement>(id)!.click();
const type = (id: string, value: string) => {
  const input = byTestId<HTMLInputElement>(id)!;
  input.value = value;
  input.dispatchEvent(new InputEvent("input", { bubbles: true }));
};
const playlistSelect = () => byTestId<HTMLSelectElement>("kids-playlist")!;

describe("KidsSettings: a playlist chosen from its own menu", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    setKidsStatus(null);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("is preselected while kids mode is off", async () => {
    backend(OFF);
    requestKidsPlaylist(2);
    const dispose = mount();
    await flush();

    expect(playlistSelect().value).toBe("2");
    expect(mockInvoke).not.toHaveBeenCalledWith("kids_update", expect.anything());
    dispose();
  });

  it("becomes the kids playlist while kids mode is on, like picking it in the select", async () => {
    backend(ON);
    requestKidsPlaylist(2);
    const dispose = mount();
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("kids_update", { playlistId: 2, language: "EN" });
    expect(playlistSelect().value).toBe("2");
    dispose();
  });

  it("is taken once: the next opening shows the stored playlist", async () => {
    backend(ON);
    requestKidsPlaylist(2);
    const first = mount();
    await flush();
    first();
    document.body.innerHTML = "";
    mockInvoke.mockClear();

    const dispose = mount();
    await flush();
    expect(mockInvoke).not.toHaveBeenCalledWith("kids_update", expect.anything());
    expect(playlistSelect().value).toBe("1");
    dispose();
  });
});

describe("KidsSettings: status and PIN dialogs", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    setKidsStatus(null);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows Off with Turn on, On with Lock now, Turn off and Change PIN", async () => {
    backend(OFF);
    let dispose = mount();
    await flush();
    expect(byTestId("kids-status")?.textContent).toBe("Off");
    expect(byTestId("kids-turn-on")).not.toBeNull();
    expect(byTestId("kids-lock-now")).toBeNull();
    expect(byTestId("kids-change-pin")).toBeNull();
    dispose();

    backend(ON);
    dispose = mount();
    await flush();
    expect(byTestId("kids-status")?.textContent).toBe("On");
    expect(byTestId("kids-turn-on")).toBeNull();
    expect(byTestId("kids-lock-now")).not.toBeNull();
    expect(byTestId("kids-turn-off")).not.toBeNull();
    expect(byTestId("kids-change-pin")).not.toBeNull();
    dispose();
  });

  it("turns on only with the PIN typed twice the same", async () => {
    backend(OFF);
    const onLock = vi.fn();
    const dispose = mount(onLock);
    await flush();

    click("kids-turn-on");
    expect(byTestId("kids-turn-on-dialog")).not.toBeNull();
    expect(byTestId<HTMLButtonElement>("kids-enable")!.disabled).toBe(true);
    type("kids-pin-new", "1234");
    type("kids-pin-repeat", "1243");
    click("kids-enable");
    await flush();

    expect(byTestId("kids-pin-error")?.textContent).toBe("The two PINs differ");
    expect(byTestId<HTMLInputElement>("kids-pin-repeat")!.value).toBe("");
    expect(mockInvoke).not.toHaveBeenCalledWith("kids_enable", expect.anything());

    type("kids-pin-new", "1234");
    type("kids-pin-repeat", "1234");
    click("kids-enable");
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("kids_enable", { pin: "1234", playlistId: null, language: "EN" });
    expect(onLock).toHaveBeenCalledTimes(1);
    expect(byTestId("kids-turn-on-dialog")).toBeNull();
    dispose();
  });

  it("changes the PIN only with the new one repeated", async () => {
    backend(ON);
    const dispose = mount();
    await flush();

    click("kids-change-pin");
    type("kids-pin-current", "1234");
    type("kids-pin-new", "5678");
    type("kids-pin-repeat", "5679");
    click("kids-change-pin-submit");
    await flush();
    expect(byTestId("kids-pin-error")?.textContent).toBe("The two new PINs differ");
    expect(byTestId<HTMLInputElement>("kids-pin-current")!.value).toBe("1234");
    expect(mockInvoke).not.toHaveBeenCalledWith("kids_change_pin", expect.anything());

    type("kids-pin-new", "5678");
    type("kids-pin-repeat", "5678");
    click("kids-change-pin-submit");
    await flush();
    expect(mockInvoke).toHaveBeenCalledWith("kids_change_pin", { oldPin: "1234", newPin: "5678" });
    expect(byTestId("kids-change-pin-dialog")).toBeNull();
    dispose();
  });

  it("shows the wait after too many wrong PINs, like the unlock dialog", async () => {
    backend(ON, false, 30);
    const dispose = mount();
    await flush();

    click("kids-turn-off");
    type("kids-pin-current", "0000");
    click("kids-disable");
    await flush();

    expect(byTestId("kids-pin-error")?.textContent).toMatch(/try again in \d+ s/);
    expect(byTestId<HTMLInputElement>("kids-pin-current")!.disabled).toBe(true);
    expect(byTestId("kids-turn-off-dialog")).not.toBeNull();
    expect(byTestId("kids-status")?.textContent).toBe("On");
    dispose();
  });

  it("says Wrong PIN while no wait is due yet", async () => {
    backend(ON, false, 0);
    const dispose = mount();
    await flush();

    click("kids-change-pin");
    type("kids-pin-current", "0000");
    type("kids-pin-new", "5678");
    type("kids-pin-repeat", "5678");
    click("kids-change-pin-submit");
    await flush();

    expect(byTestId("kids-pin-error")?.textContent).toBe("Wrong PIN");
    expect(byTestId<HTMLInputElement>("kids-pin-current")!.value).toBe("");
    dispose();
  });

  it("turns off with the right PIN", async () => {
    backend(ON);
    const dispose = mount();
    await flush();

    click("kids-turn-off");
    type("kids-pin-current", "1234");
    click("kids-disable");
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("kids_disable", { pin: "1234" });
    expect(byTestId("kids-turn-off-dialog")).toBeNull();
    expect(byTestId("kids-status")?.textContent).toBe("Off");
    dispose();
  });
});

describe("Settings → Kids mode → Turn on", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    setKidsStatus(null);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("closes Settings and locks right away", async () => {
    backend(OFF);
    const onOpenChange = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(
      () => (
        <SettingsDialog
          open
          onOpenChange={onOpenChange}
          section="kids"
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
    await flush();

    click("kids-turn-on");
    type("kids-pin-new", "1234");
    type("kids-pin-repeat", "1234");
    click("kids-enable");
    await flush();

    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(mockInvoke).toHaveBeenCalledWith("kids_lock");
    expect(kidsLocked()).toBe(true);
    dispose();
  });
});
