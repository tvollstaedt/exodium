import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { JSX } from "solid-js";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { Game } from "../api/tauri";
import { KidsScreen } from "./KidsScreen";
import { KidsPinDialog } from "./KidsPinDialog";
import { setKidsStatus } from "../stores/kids";

const mockInvoke = vi.mocked(invoke);
const flush = () => new Promise((r) => setTimeout(r, 0));

function mount(node: () => JSX.Element) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return render(node, host);
}

describe("KidsScreen", () => {
  beforeEach(() => { mockInvoke.mockReset(); });
  afterEach(() => { document.body.innerHTML = ""; });

  it("asks for an adult when the playlist has nothing installed", async () => {
    mockInvoke.mockResolvedValue([]);
    const dispose = mount(() => <KidsScreen onUnlocked={() => {}} />);
    await flush();
    expect(document.querySelector("[data-testid=kids-empty]")).not.toBeNull();
    dispose();
  });

  it("starts a game with one click", async () => {
    const keen = { id: 5, title: "Commander Keen 4", torrent_source: "eXoDOS", thumbnail_key: null } as Game;
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === "kids_games" ? [keen] : "Launched"));
    const dispose = mount(() => <KidsScreen onUnlocked={() => {}} />);
    await flush();
    document.querySelector<HTMLButtonElement>("[data-testid=kids-card]")!.click();
    await flush();
    expect(mockInvoke).toHaveBeenCalledWith("launch_game", { id: 5 });
    dispose();
  });
});

describe("KidsPinDialog", () => {
  beforeEach(() => { mockInvoke.mockReset(); });
  afterEach(() => { document.body.innerHTML = ""; });

  it("says a PIN was wrong and counts the wait down", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === "kids_unlock"
      ? { ok: false, retry_in: 30 }
      : { enabled: true, locked: true, playlist_id: 1, language: "EN", retry_in: 30 }));
    const unlocked = vi.fn();
    const dispose = mount(() => <KidsPinDialog open onClose={() => {}} onUnlocked={unlocked} />);
    const input = document.querySelector<HTMLInputElement>("[data-testid=kids-pin-input]")!;
    input.value = "0000";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    document.querySelector<HTMLButtonElement>("[data-testid=kids-pin-submit]")!.click();
    await flush();
    await flush();
    expect(unlocked).not.toHaveBeenCalled();
    expect(document.querySelector("[data-testid=kids-pin-error]")?.textContent).toMatch(/try again in \d+ s/);
    expect(input.disabled).toBe(true);
    dispose();
  });

  it("opens ready for a PIN when no wait is pending, however late the clock ticks", async () => {
    setKidsStatus(null);
    let t = 1_000_000;
    const clock = vi.spyOn(Date, "now").mockImplementation(() => (t += 7));
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === "kids_unlock"
      ? { ok: true, retry_in: 0 }
      : { enabled: true, locked: false, playlist_id: 1, language: "EN", retry_in: 0 }));
    const unlocked = vi.fn();
    const dispose = mount(() => <KidsPinDialog open onClose={() => {}} onUnlocked={unlocked} />);
    expect(document.querySelector("[data-testid=kids-pin-error]")).toBeNull();
    const input = document.querySelector<HTMLInputElement>("[data-testid=kids-pin-input]")!;
    input.value = "1234";
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
    document.querySelector<HTMLButtonElement>("[data-testid=kids-pin-submit]")!.click();
    await flush();
    await flush();
    expect(unlocked).toHaveBeenCalled();
    clock.mockRestore();
    dispose();
  });
});
