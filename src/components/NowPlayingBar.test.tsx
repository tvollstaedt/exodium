import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { NowPlayingBar } from "./NowPlayingBar";
import { playTheme, hidePlayer } from "../stores/music";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("../stores/games", () => ({
  games: () => [],
  hasMore: () => false,
  fetchMoreGames: vi.fn(async () => {}),
}));
vi.mock("../stores/network", () => ({
  isOffline: () => false,
  networkMode: () => "live",
}));

const mockInvoke = vi.mocked(invoke);
const PROBING = { phase: "probing", progress: 0, total_bytes: 0, path: null, error: null };
const game = (id: number) => ({ id, title: `Game ${id}`, torrent_source: "eXoDOS", thumbnail_key: `k${id}` });

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return { host, dispose: render(() => <NowPlayingBar />, host) };
}

const barOpen = () => document.body.querySelector(".now-playing-bar") != null;
/** The class the detail panel and the backdrop read to leave room for the bar:
 *  toggling it resizes the panel, so it is the thing that must not move. */
const roomReserved = () => document.body.classList.contains("has-player");

describe("NowPlayingBar", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
    // Every probe stays pending, so nothing ever loads and jsdom is never
    // asked to play audio - the wait itself is what this file is about.
    mockInvoke.mockImplementation(async () => PROBING);
  });

  afterEach(() => {
    hidePlayer();
    document.body.innerHTML = "";
    document.body.classList.remove("has-player");
    vi.useRealTimers();
  });

  /** Opening a game panel probes for a theme, and about half the catalogue has
   *  none (§14). The bar used to open on that probe and close again on the
   *  answer, and since the panel leaves 56 px of room for it, the whole panel -
   *  its screenshot strip most visibly - jumped up and back down. Measured on
   *  Linux/WebKitGTK 2026-09-20: gallery top 703 -> 663 -> 703 px in 170 ms. */
  it("stays shut while a panel's autoplay probes for a theme", async () => {
    const { dispose, host } = mount();

    playTheme(game(1), { auto: true });
    await vi.advanceTimersByTimeAsync(50);

    expect(barOpen(), "an autoplay probe must not open the bar").toBe(false);
    expect(roomReserved(), "and must not resize the panel").toBe(false);

    dispose(); host.remove();
  });

  /** The other half of the rule: a track the listener asked for shows up right
   *  away, because a first shuffle pick can take a minute and a click with no
   *  feedback reads as a dead button. */
  it("opens for a track the listener asked for, before its bytes arrive", async () => {
    const { dispose, host } = mount();

    playTheme(game(2));
    await vi.advanceTimersByTimeAsync(50);

    expect(barOpen(), "a clicked track opens the bar while it fetches").toBe(true);
    expect(roomReserved()).toBe(true);

    dispose(); host.remove();
  });
});
