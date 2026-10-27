import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { Game } from "../api/tauri";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { GameDetailPanel } from "./GameDetailPanel";

const mockInvoke = vi.mocked(invoke);

/** Only the four pause/resume entry points are stubbed - the rest of the store
 *  is the real thing, so the panel keeps rendering the player it always did.
 *  `vi.hoisted` because the mock factory runs before this file's own consts. */
const music = vi.hoisted(() => ({
  pauseFor: vi.fn(),
  resumeFrom: vi.fn(),
  pauseForGame: vi.fn(),
  resumeFromGame: vi.fn(),
}));
vi.mock("../stores/music", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../stores/music")>()),
  ...music,
}));

/** Minimal row shaped like the merged card the grid hands to the panel. */
function makeGame(over: Partial<Game> = {}): Game {
  return {
    id: 1,
    title: "Magic Carpet Plus",
    sort_title: "Magic Carpet Plus",
    platform: "MS-DOS",
    developer: "Bullfrog Productions, Ltd.",
    publisher: "Electronic Arts, Inc.",
    release_date: null,
    year: 1995,
    genre: "Action;Flight Simulator",
    series: "Magic Carpet series",
    play_mode: "Single Player",
    rating: 5,
    description: "English description text.",
    notes: null,
    source: null,
    application_path: null,
    dosbox_conf: null,
    status: null,
    region: null,
    max_players: 8,
    language: "EN",
    shortcode: "MagCarp",
    torrent_source: "eXoDOS",
    in_library: false,
    installed: false,
    game_torrent_index: 10,
    gamedata_torrent_index: null,
    download_size: 268_000_000,
    has_thumbnail: true,
    dosbox_variant: null,
    favorited: false,
    thumbnail_key: "abc123",
    manual_path: "Manuals\\MS-DOS\\Magic Carpet Plus (1995).pdf",
    last_played: null,
    music_file: null,
    available_languages: null,
    ...over,
  } as Game;
}

const EMPTY_META = { manual_path: null, manual_kind: null, images: [], thumbnails: [] };
const VIDEO_READY = {
  phase: "ready", progress: 1, total_bytes: 2_000_000,
  path: "/data/content/videocache/eXoDOS_1.mp4", error: null,
};

/** Render into a detached container and return it plus a disposer. Solid's
 *  render() flushes effects, so anything that throws at effect time (a helper
 *  used before its `const` is initialised, say) surfaces here - which is
 *  exactly what type-checking cannot catch. */
function mount(game: Game) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(() => <GameDetailPanel game={game} onClose={() => {}} />, host);
  return { host, dispose };
}

describe("GameDetailPanel", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      return null;
    });
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  /** One Escape closes one layer: the panel marks the keypress consumed so
   *  the selection's window handler does not exit the selection as well. */
  it("marks the Escape it closes on as handled", async () => {
    const onClose = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={makeGame()} onClose={onClose} />, host);
    await new Promise((r) => setTimeout(r, 0));
    const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true, bubbles: true });
    window.dispatchEvent(escape);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(escape.defaultPrevented).toBe(true);
    dispose();
  });

  it("leaves Escape alone while no game is open", async () => {
    const onClose = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={null} onClose={onClose} />, host);
    await new Promise((r) => setTimeout(r, 0));
    const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true, bubbles: true });
    window.dispatchEvent(escape);
    expect(onClose).not.toHaveBeenCalled();
    expect(escape.defaultPrevented).toBe(false);
    dispose();
  });

  /** The footprint row lives in the Information grid: a separate block under
   *  it rendered its label and no field in the live webview. */
  it("shows what an installed game costs on disk", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "game_disk_usage") { return { game_bytes: 463_000, archive_bytes: 0, save_bytes: 0 }; }
      return null;
    });
    const { dispose } = mount(makeGame({ installed: true, in_library: true }));
    await new Promise((r) => setTimeout(r, 50));
    const grid = document.body.querySelector(".game-detail-fields");
    expect(grid?.textContent).toContain("On disk");
    expect(grid?.textContent).toContain("463 KB (game)");
    dispose();
  });

  // The panel asks for a video 400ms after settling on a game and then lets
  // the cover hold the hero for another two seconds before playing it.
  // Reproduces "no video plays at all".
  it("shows the preview video once the backend reports it ready", async () => {
    vi.useFakeTimers();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "start_game_video") { return VIDEO_READY; }
      if (cmd === "get_video_status") { return VIDEO_READY; }
      return null;
    });

    const { host, dispose } = mount(makeGame({ id: 42, shortcode: "VID42" }));
    await vi.advanceTimersByTimeAsync(3200);

    const video = host.ownerDocument.querySelector("video.game-detail-hero-video");
    expect(video, "the hero video element should be mounted").not.toBeNull();
    expect(video?.getAttribute("src") ?? "").toContain("videocache");
    // The cover crossfades out only once playback actually started.
    expect(video?.className).toContain("is-visible");
    // Previews carry sound. Re-adding `muted` to buy back autoplay would take
    // it away silently - the muted retry in the effect is the fallback path.
    expect((video as HTMLVideoElement | null)?.muted).toBe(false);
    dispose(); host.remove();
  });

  /** The preview claims the speakers as soon as an unmuted start is
   *  scheduled (the theme would otherwise start inside the cover beat), and
   *  gives them back the moment it stops using them - a pause, the lightbox
   *  aside. */
  it("claims the speakers for an unmuted preview and hands them back when it pauses", async () => {
    vi.useFakeTimers();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "start_game_video") { return VIDEO_READY; }
      if (cmd === "get_video_status") { return VIDEO_READY; }
      return null;
    });
    music.pauseFor.mockClear();
    music.resumeFrom.mockClear();

    const { host, dispose } = mount(makeGame({ id: 43, shortcode: "VID43" }));
    await vi.advanceTimersByTimeAsync(3200);

    const video = host.ownerDocument.querySelector("video.game-detail-hero-video");
    expect(video, "the hero video element should be mounted").not.toBeNull();
    expect(music.pauseFor).toHaveBeenCalledWith("video");
    expect(music.resumeFrom).not.toHaveBeenCalled();

    video!.dispatchEvent(new Event("play"));
    video!.dispatchEvent(new Event("pause"));
    expect(music.resumeFrom).toHaveBeenCalledWith("video");

    dispose(); host.remove();
  });

  /** Opening the lightbox pauses the hero so the trailer does not run twice,
   *  and the lightbox's own <video> takes the speakers over. But `pause()` only
   *  QUEUES the event: it arrives after the hold effect has claimed the reason
   *  and used to withdraw it again, so the theme played over the trailer. */
  it("keeps the speakers while the lightbox plays the same preview", async () => {
    vi.useFakeTimers();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "start_game_video") { return VIDEO_READY; }
      if (cmd === "get_video_status") { return VIDEO_READY; }
      return null;
    });

    const { host, dispose } = mount(makeGame({ id: 44, shortcode: "VID44" }));
    await vi.advanceTimersByTimeAsync(3200);
    const video = host.ownerDocument.querySelector("video.game-detail-hero-video");
    expect(video, "the hero video element should be mounted").not.toBeNull();

    // The unmuted preview holds the speakers since it was scheduled.
    expect(music.pauseFor).toHaveBeenCalledWith("video");
    music.resumeFrom.mockClear();
    video!.dispatchEvent(new Event("play"));

    // Clicking the hero opens the lightbox on entry 0 - the video itself,
    // which inherits the claim...
    (video as HTMLVideoElement).click();
    await vi.advanceTimersByTimeAsync(0);

    // ...and only now does the hero's own pause event land.
    video!.dispatchEvent(new Event("pause"));
    expect(music.resumeFrom).not.toHaveBeenCalled();

    const backdrop = document.querySelector(".lightbox-backdrop") as HTMLElement | null;
    expect(backdrop, "the lightbox should be open").not.toBeNull();
    backdrop!.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(music.resumeFrom).toHaveBeenCalledTimes(1);
    expect(music.resumeFrom).toHaveBeenCalledWith("video");

    dispose(); host.remove();
  });

  /** A refused second launch does not end the first one. The claim on the
   *  speakers belongs to the launch that succeeded, so giving it back here
   *  started the theme over a running emulator. */
  it("keeps the game's claim when a launch is refused as already running", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "launch_game") { throw new Error("'Magic Carpet Plus' is already running."); }
      return null;
    });
    music.pauseForGame.mockClear();
    music.resumeFromGame.mockClear();

    const { host, dispose } = mount(makeGame({ installed: true }));
    await Promise.resolve();
    const play = host.ownerDocument.querySelector("button.btn-play") as HTMLButtonElement | null;
    expect(play, "an installed game should offer Play").toBeTruthy();

    play!.click();
    await new Promise((r) => setTimeout(r, 0));

    expect(music.pauseForGame).toHaveBeenCalledWith(1);
    expect(music.resumeFromGame).not.toHaveBeenCalled();
    dispose(); host.remove();
  });

  it("gives the claim back when a launch fails for any other reason", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "launch_game") { throw new Error("DOSBox binary not found"); }
      return null;
    });
    music.pauseForGame.mockClear();
    music.resumeFromGame.mockClear();

    const { host, dispose } = mount(makeGame({ installed: true }));
    await Promise.resolve();
    const play = host.ownerDocument.querySelector("button.btn-play") as HTMLButtonElement | null;
    play!.click();
    await new Promise((r) => setTimeout(r, 0));

    expect(music.resumeFromGame).toHaveBeenCalledWith(1);
    dispose(); host.remove();
  });

  it("renders a single-language game without throwing", async () => {
    const { host, dispose } = mount(makeGame());
    await Promise.resolve();
    const text = document.body.textContent ?? "";
    expect(text).toContain("Magic Carpet Plus");
    expect(text).toContain("English description text.");
    expect(text).toContain("Bullfrog Productions, Ltd.");
    dispose();
    host.remove();
  });

  it("offers Download for an uninstalled game and Play once installed", async () => {
    const a = mount(makeGame());
    await Promise.resolve();
    expect(document.body.textContent).toContain("Download");
    a.dispose(); a.host.remove();
    document.body.innerHTML = "";

    const b = mount(makeGame({ installed: true }));
    await Promise.resolve();
    expect(document.body.textContent).toContain("Play");
    b.dispose(); b.host.remove();
  });

  /** The action bar carries only the primary action and the manual; the rest
   *  moved behind the ⋯ control, so every menu item is reached through it. */
  const openMore = async (host: HTMLElement) => {
    const more = [...host.ownerDocument.querySelectorAll("button")]
      .find((b) => b.className.includes("btn-more"));
    expect(more, "the overflow control should be offered").toBeTruthy();
    more!.click();
    await Promise.resolve();
  };

  const menuItem = (host: HTMLElement, text: string) =>
    [...host.ownerDocument.querySelectorAll("button.context-menu-item")]
      .find((b) => (b.textContent ?? "").includes(text)) as HTMLButtonElement | undefined;

  // Reset throws away savegames, so a single stray click must not do it.
  it("only resets game data on the second click", async () => {
    const { host, dispose } = mount(makeGame({ installed: true }));
    await Promise.resolve();
    await openMore(host);

    const button = menuItem(host, "Reset game data");
    expect(button, "installed games should offer Reset").toBeTruthy();

    button!.click();
    await Promise.resolve();
    expect(mockInvoke).not.toHaveBeenCalledWith("reset_game_data", expect.anything());
    expect(menuItem(host, "Discard all game data?")).toBeTruthy();

    menuItem(host, "Discard all game data?")!.click();
    await Promise.resolve();
    expect(mockInvoke).toHaveBeenCalledWith("reset_game_data", { id: 1 });
    dispose(); host.remove();
  });

  it("does not offer Reset for a game that is not installed", async () => {
    const { host, dispose } = mount(makeGame({ installed: false }));
    await Promise.resolve();
    await openMore(host);
    expect(menuItem(host, "Reset game data")).toBeUndefined();
    dispose(); host.remove();
  });

  /// Favouriting is frequent and reversible, so it belongs in the bar - it
  /// was reachable from the grid but nowhere in the panel.
  it("offers a favourite toggle in the action bar", async () => {
    const { host, dispose } = mount(makeGame({ installed: true }));
    await Promise.resolve();
    const star = [...host.ownerDocument.querySelectorAll("button")]
      .find((b) => b.className.includes("btn-fav"));
    expect(star, "the panel should offer a favourite toggle").toBeTruthy();

    star!.click();
    await Promise.resolve();
    expect(mockInvoke).toHaveBeenCalledWith("toggle_favorite", { id: 1 });
    dispose(); host.remove();
  });

  /// The bar is down to the primary action plus the manual. Reset and
  /// Uninstall sitting next to Play is what made it a wall of five.
  it("keeps destructive actions out of the action bar", async () => {
    const { host, dispose } = mount(makeGame({ installed: true }));
    await Promise.resolve();
    const bar = host.ownerDocument.querySelector(".game-detail-actions");
    const labels = [...(bar?.querySelectorAll("button") ?? [])]
      .map((b) => b.textContent ?? "").join(" | ");
    expect(labels).not.toContain("Uninstall");
    expect(labels).not.toContain("Reset");
    expect(labels).not.toContain("Playlist");
    dispose(); host.remove();
  });

  // The header names the row every button acts on. PL/ES variants carry
  // genuinely different titles, so showing the English one while DE is
  // selected would misidentify what Play/Uninstall would touch.
  it("titles the panel after the selected variant", async () => {
    const variants: Game[] = [
      makeGame({ id: 1, shortcode: "OFFICE", language: "EN", title: "The Office", installed: false }),
      makeGame({ id: 2, shortcode: "OFFICE", language: "DE", title: "Das Amt", installed: true }),
    ];
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_variants") { return variants; }
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      return null;
    });

    const { host, dispose } = mount(makeGame({
      shortcode: "OFFICE", title: "The Office", available_languages: "EN:0,DE:2",
    }));
    await new Promise((r) => setTimeout(r, 0));

    expect(host.ownerDocument.querySelector(".game-detail-title")?.textContent).toBe("Das Amt");
    dispose();
    host.remove();
  });

  it("shows one chip per language and selects the installed variant", async () => {
    const variants: Game[] = [
      makeGame({ id: 1, language: "EN", installed: false }),
      makeGame({ id: 2, language: "DE", installed: true, description: null, manual_path: null,
                 torrent_source: "eXoDOS_GLP", developer: null, publisher: null }),
    ];
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_variants") { return variants; }
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      return null;
    });

    const { host, dispose } = mount(makeGame({ available_languages: "EN:0,DE:2" }));
    // Variants arrive from an awaited invoke, so let the microtask queue drain.
    await new Promise((r) => setTimeout(r, 0));

    const chips = host.ownerDocument.querySelectorAll(".variant-chip");
    expect(chips.length).toBe(2);
    const selectedChip = host.ownerDocument.querySelector(".variant-chip.is-selected");
    // DE is installed, so it wins the default selection over the EN row.
    expect(selectedChip?.textContent).toContain("DE");

    const text = document.body.textContent ?? "";
    // DE has no text of its own - the English one is shown, and labelled.
    expect(text).toContain("English description text.");
    expect(text).toContain("no German text");
    // Fields fall back to the English row rather than rendering blank.
    expect(text).toContain("Bullfrog Productions, Ltd.");
    dispose();
    host.remove();
  });

  /** The grid hands the panel a FRESH object for the same row on every library
   *  refresh, and the panel's effects key on ids for exactly that reason - an
   *  effect hanging on `props.game` reloads the media and takes the strip down
   *  with it. Four fixes have each moved one trigger onto an id (68a65cfc,
   *  8b3fabb0, DECISIONS 2026-09-11); this is the guard none of them left
   *  behind. */
  it("keeps the same gallery elements when the library refreshes the row", async () => {
    vi.useFakeTimers();
    const META = {
      manual_path: null, manual_kind: null,
      images: ["/pack/Magic Carpet-01.png", "/pack/Magic Carpet-02.png"],
      thumbnails: ["/cache/1.png", "/cache/2.png"],
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "game_disk_usage") { return { game_bytes: 463_000, archive_bytes: 0, save_bytes: 0 }; }
      return null;
    });

    const [game, setGame] = createSignal(makeGame({ installed: true, in_library: true }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={game()} onClose={() => {}} />, host);
    await vi.advanceTimersByTimeAsync(3200);

    const before = Array.from(document.body.querySelectorAll("img.gallery-thumb"));
    expect(before.length, "the strip renders one img per image").toBe(2);
    const metaCalls = mockInvoke.mock.calls.filter((c) => c[0] === "get_game_metadata").length;

    // What Library.tsx does on `lastGameLibraryChange`: same id, new object.
    setGame(makeGame({ installed: true, in_library: true }));
    await vi.advanceTimersByTimeAsync(3200);

    const after = Array.from(document.body.querySelectorAll("img.gallery-thumb"));
    expect(after.length).toBe(2);
    // Identity, not count: a remounted strip decodes its images again, which
    // is the flash this guards.
    expect(after[0], "the gallery must not be rebuilt").toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(
      mockInvoke.mock.calls.filter((c) => c[0] === "get_game_metadata").length,
      "and its media must not be fetched again",
    ).toBe(metaCalls);

    dispose();
    host.remove();
  });

  /** Fullscreen is a class on the same nodes (§12): the strip must survive
   *  the toggle in both directions with its elements intact. */
  it("keeps the same gallery elements when fullscreen is toggled", async () => {
    vi.useFakeTimers();
    const META = {
      manual_path: null, manual_kind: null,
      images: ["/pack/Magic Carpet-01.png", "/pack/Magic Carpet-02.png"],
      thumbnails: ["/cache/1.png", "/cache/2.png"],
    };
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return META; }
      if (cmd === "get_game_variants") { return []; }
      return null;
    });
    const { dispose } = mount(makeGame({ installed: true, in_library: true }));
    await vi.advanceTimersByTimeAsync(3200);

    const panel = document.body.querySelector<HTMLElement>('[data-testid="game-detail"]')!;
    const toggle = document.body.querySelector<HTMLButtonElement>('[data-testid="game-detail-fullscreen"]')!;
    const before = Array.from(document.body.querySelectorAll("img.gallery-thumb"));
    expect(before.length).toBe(2);

    toggle.click();
    await vi.advanceTimersByTimeAsync(50);
    expect(panel.dataset.fullscreen).toBe("true");
    expect(panel.classList.contains("is-fullscreen")).toBe(true);
    const during = Array.from(document.body.querySelectorAll("img.gallery-thumb"));
    expect(during[0]).toBe(before[0]);
    expect(during[1]).toBe(before[1]);

    toggle.click();
    await vi.advanceTimersByTimeAsync(50);
    expect(panel.dataset.fullscreen).toBe("false");
    const after = Array.from(document.body.querySelectorAll("img.gallery-thumb"));
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    dispose();
  });

  /** The wide tier (main.css) queries the frame and lays out the layout, head
   *  and side groups. All four are static: a refresh or the fullscreen toggle
   *  must find the same nodes, or the strip under them remounts. */
  it("keeps the layout wrappers across a row refresh and the fullscreen toggle", async () => {
    vi.useFakeTimers();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") {
        return { manual_path: null, manual_kind: null, images: ["/pack/a.png"], thumbnails: ["/cache/a.png"] };
      }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "game_disk_usage") { return { game_bytes: 463_000, archive_bytes: 0, save_bytes: 0 }; }
      return null;
    });
    const [game, setGame] = createSignal(makeGame({ installed: true, in_library: true }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={game()} onClose={() => {}} />, host);
    await vi.advanceTimersByTimeAsync(3200);

    const panel = document.body.querySelector<HTMLElement>('[data-testid="game-detail"]')!;
    const frame = panel.querySelector<HTMLElement>(":scope > .game-detail-frame");
    expect(frame, "the query container is a child of the panel, not the panel").not.toBeNull();
    const layout = frame!.querySelector(":scope > .game-detail-layout")!;
    const parts = () => ({
      hero: layout.querySelector(":scope > .game-detail-hero"),
      head: layout.querySelector(":scope > .game-detail-body > .game-detail-head"),
      side: layout.querySelector(":scope > .game-detail-body > .game-detail-scroll > .game-detail-side"),
      media: layout.querySelector(":scope > .game-detail-media"),
    });
    const before = parts();
    expect(before.hero).not.toBeNull();
    expect(before.head?.querySelector(".game-detail-title")).not.toBeNull();
    expect(before.head?.querySelector(".game-detail-actions")).not.toBeNull();
    expect(before.side?.querySelector(".game-detail-info")).not.toBeNull();
    expect(before.media).not.toBeNull();

    setGame(makeGame({ installed: true, in_library: true }));
    await vi.advanceTimersByTimeAsync(3200);
    expect(panel.querySelector(".game-detail-frame")).toBe(frame);
    const after = parts();
    for (const key of Object.keys(before) as (keyof typeof before)[]) {
      expect(after[key], `${key} must not be rebuilt`).toBe(before[key]);
    }

    document.body.querySelector<HTMLButtonElement>('[data-testid="game-detail-fullscreen"]')!.click();
    await vi.advanceTimersByTimeAsync(50);
    expect(panel.classList.contains("is-fullscreen")).toBe(true);
    expect(panel.querySelector(".game-detail-layout")).toBe(layout);
    const during = parts();
    expect(during.head).toBe(before.head);
    expect(during.media).toBe(before.media);

    document.body.querySelector<HTMLButtonElement>('[data-testid="game-detail-fullscreen"]')!.click();
    await vi.advanceTimersByTimeAsync(50);
    dispose();
    host.remove();
  });

  it("Escape with the panel closed leaves the fullscreen preference alone", async () => {
    const { setPanelFullscreen, panelFullscreen } = await import("../stores/detailPanel");
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={null} onClose={() => {}} />, host);
    await setPanelFullscreen(true);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(panelFullscreen()).toBe(true);
    await setPanelFullscreen(false);
    dispose();
    host.remove();
  });

  it("Escape leaves fullscreen first and closes the panel second", async () => {
    vi.useFakeTimers();
    const onClose = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={makeGame()} onClose={onClose} />, host);
    await vi.advanceTimersByTimeAsync(3200);

    const panel = document.body.querySelector<HTMLElement>('[data-testid="game-detail"]')!;
    document.body.querySelector<HTMLButtonElement>('[data-testid="game-detail-fullscreen"]')!.click();
    await vi.advanceTimersByTimeAsync(50);
    expect(panel.dataset.fullscreen).toBe("true");

    const escape = () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    escape();
    expect(panel.dataset.fullscreen).toBe("false");
    expect(onClose).not.toHaveBeenCalled();
    // The preference is written, so the next open is not fullscreen either.
    expect(mockInvoke.mock.calls.some((c) => c[0] === "set_config"
      && (c[1] as { key: string; value: string }).key === "detail_panel_fullscreen"
      && (c[1] as { key: string; value: string }).value === "0")).toBe(true);

    escape();
    expect(onClose).toHaveBeenCalledTimes(1);
    dispose();
    host.remove();
  });
});

/** A closed panel stays mounted for its exit animation, showing the last game
 *  as it was, and takes no more input; reduced motion skips the exit. */
describe("GameDetailPanel closing", () => {
  const META = {
    manual_path: null, manual_kind: null,
    images: ["/pack/Magic Carpet-01.png"], thumbnails: ["/cache/1.png"],
  };

  beforeEach(() => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "start_game_video") { return VIDEO_READY; }
      if (cmd === "get_video_status") { return VIDEO_READY; }
      return null;
    });
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  async function open(onClose: () => void = () => {}) {
    const [game, setGame] = createSignal<Game | null>(makeGame({ id: 51, shortcode: "CLOSE51" }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={game()} onClose={onClose} />, host);
    await vi.advanceTimersByTimeAsync(3200);
    const panel = document.body.querySelector<HTMLElement>('[data-testid="game-detail"]')!;
    return { setGame, panel, dispose };
  }
  const current = () => document.body.querySelector<HTMLElement>('[data-testid="game-detail"]');
  const metaCalls = () => mockInvoke.mock.calls.filter((c) => c[0] === "get_game_metadata").length;

  it("keeps the last game mounted, unchanged and inert, for the exit, then unmounts it", async () => {
    const onClose = vi.fn();
    const { setGame, panel, dispose } = await open(onClose);
    const title = panel.querySelector('[data-testid="game-detail-title"]')!;
    const thumb = panel.querySelector("img.gallery-thumb");
    expect(thumb, "the gallery is up before closing").not.toBeNull();
    const fetched = metaCalls();

    setGame(null);
    await vi.advanceTimersByTimeAsync(0);
    expect(current()).toBe(panel);
    expect(panel.classList.contains("is-closing")).toBe(true);
    expect(document.body.querySelector(".game-detail-backdrop")!.classList.contains("is-closing")).toBe(true);
    expect(panel.querySelector('[data-testid="game-detail-title"]')).toBe(title);
    expect(title.textContent).toBe("Magic Carpet Plus");
    expect(panel.querySelector("img.gallery-thumb")).toBe(thumb);

    // Already closed: neither Escape nor a stray click closes it again.
    const escape = new KeyboardEvent("keydown", { key: "Escape", cancelable: true });
    window.dispatchEvent(escape);
    expect(escape.defaultPrevented).toBe(false);
    panel.querySelector<HTMLButtonElement>(".game-detail-close")!.click();
    document.body.querySelector<HTMLElement>(".game-detail-backdrop")!.click();
    expect(onClose).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(150);
    expect(current(), "still animating out").toBe(panel);
    expect(metaCalls(), "nothing reloads while closing").toBe(fetched);
    await vi.advanceTimersByTimeAsync(100);
    expect(current()).toBeNull();
    expect(document.body.querySelector(".game-detail-backdrop")).toBeNull();
    dispose();
  });

  it("stops the preview the moment the panel starts closing", async () => {
    const { setGame, panel, dispose } = await open();
    const video = panel.querySelector<HTMLVideoElement>("video.game-detail-hero-video")!;
    expect(video.getAttribute("src") ?? "").toContain("videocache");
    music.resumeFrom.mockClear();

    setGame(null);
    await vi.advanceTimersByTimeAsync(0);
    expect(current()).toBe(panel);
    expect(video.hasAttribute("src"), "unloaded, not left to play on").toBe(false);
    expect(video.className).not.toContain("is-visible");
    expect(music.resumeFrom).toHaveBeenCalledWith("video");
    dispose();
  });

  /** The video and theme beats used to die with the unmount; a closing panel
   *  is still mounted, so the closing state itself has to stop them. */
  it("asks for neither video nor theme once the panel is closing", async () => {
    const musicStore = await import("../stores/music");
    const request = vi.spyOn(musicStore, "requestTheme");
    const play = vi.spyOn(musicStore, "playTheme");
    const [game, setGame] = createSignal<Game | null>(makeGame({ id: 55 }));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={game()} onClose={() => {}} />, host);
    try {
      // Both 400 ms beats then fall inside the 200 ms exit.
      await vi.advanceTimersByTimeAsync(300);
      setGame(null);
      await vi.advanceTimersByTimeAsync(1000);
      expect(request).not.toHaveBeenCalled();
      expect(play).not.toHaveBeenCalled();
      expect(mockInvoke.mock.calls.some((c) => c[0] === "start_game_video")).toBe(false);
    } finally {
      request.mockRestore();
      play.mockRestore();
      dispose();
    }
  });

  it("cancels the exit when another game opens meanwhile", async () => {
    const { setGame, panel, dispose } = await open();
    setGame(null);
    await vi.advanceTimersByTimeAsync(100);

    setGame(makeGame({ id: 52, title: "Syndicate", shortcode: "SYND" }));
    await vi.advanceTimersByTimeAsync(0);
    expect(current()).toBe(panel);
    expect(panel.classList.contains("is-closing")).toBe(false);
    expect(panel.querySelector('[data-testid="game-detail-title"]')!.textContent).toBe("Syndicate");
    await vi.advanceTimersByTimeAsync(500);
    expect(current(), "the stale exit timer must not unmount the new game").toBe(panel);
    dispose();
  });

  it("cancels the exit when the same game opens again, without rebuilding it", async () => {
    const onClose = vi.fn();
    const { setGame, panel, dispose } = await open(onClose);
    const thumb = panel.querySelector("img.gallery-thumb");
    setGame(null);
    await vi.advanceTimersByTimeAsync(100);

    setGame(makeGame({ id: 51, shortcode: "CLOSE51" }));
    await vi.advanceTimersByTimeAsync(500);
    expect(current()).toBe(panel);
    expect(panel.classList.contains("is-closing")).toBe(false);
    expect(panel.querySelector("img.gallery-thumb")).toBe(thumb);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(onClose, "open again, so it closes again").toHaveBeenCalledTimes(1);
    dispose();
  });

  it("unmounts at once under prefers-reduced-motion", async () => {
    const { setGame, dispose } = await open();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (query: string) => ({ matches: query.includes("reduce"), media: query }),
    });
    try {
      setGame(null);
      await vi.advanceTimersByTimeAsync(0);
      expect(current()).toBeNull();
    } finally {
      delete (window as { matchMedia?: unknown }).matchMedia;
      dispose();
    }
  });

  /** The entry is picked when the panel opens: keyed on the fullscreen class,
   *  every toggle would replay it. */
  it("fades a fullscreen panel in and keeps that entry across toggles", async () => {
    const { setPanelFullscreen } = await import("../stores/detailPanel");
    const [game, setGame] = createSignal<Game | null>(null);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={game()} onClose={() => {}} />, host);
    await vi.advanceTimersByTimeAsync(0);
    try {
      await setPanelFullscreen(true);
      setGame(makeGame({ id: 53 }));
      await vi.advanceTimersByTimeAsync(400);
      const panel = current()!;
      expect(panel.classList.contains("enters-zoom")).toBe(true);
      panel.querySelector<HTMLButtonElement>('[data-testid="game-detail-fullscreen"]')!.click();
      await vi.advanceTimersByTimeAsync(50);
      expect(panel.classList.contains("is-fullscreen")).toBe(false);
      expect(panel.classList.contains("enters-zoom")).toBe(true);

      setGame(null);
      await vi.advanceTimersByTimeAsync(300);
      setGame(makeGame({ id: 54 }));
      await vi.advanceTimersByTimeAsync(400);
      expect(current()!.classList.contains("enters-zoom"), "a normal open slides").toBe(false);
    } finally {
      await setPanelFullscreen(false);
      dispose();
    }
  });
});

/** jsdom has no container queries, so the two rules that keep the wide tier
 *  honest are read from the stylesheet: it never reaches fullscreen, which has
 *  its own grid, and the container is the frame - an element cannot restyle
 *  itself from its own query. Read from disk: vitest blanks CSS imports,
 *  `?raw` included. */
describe("GameDetailPanel wide tier stylesheet", () => {
  const css = readFileSync(resolve(__dirname, "../styles/main.css"), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "");

  const blockAfter = (marker: string): string => {
    const open = css.indexOf("{", css.indexOf(marker));
    let depth = 0;
    for (let i = open; i < css.length; i++) {
      if (css[i] === "{") { depth++; }
      if (css[i] === "}" && --depth === 0) { return css.slice(open + 1, i); }
    }
    throw new Error(`unterminated block after ${marker}`);
  };

  it("scopes every rule of the tier away from fullscreen", () => {
    expect(css).toContain("@container detail-panel (min-width: 760px)");
    const selectors = [...blockAfter("@container detail-panel (min-width: 760px)").matchAll(/([^{}]+)\{[^{}]*\}/g)]
      .flatMap((m) => m[1].split(",").map((s) => s.trim()));
    expect(selectors.length).toBeGreaterThan(5);
    for (const s of selectors) {
      expect(s.startsWith(".game-detail-panel:not(.is-fullscreen) "), s).toBe(true);
    }
  });

  it("makes the frame the only query container", () => {
    const owners = [...css.matchAll(/([^{}]+)\{[^{}]*\bcontainer(?:-type|-name)?\s*:[^{}]*\}/g)]
      .map((m) => m[1].trim());
    expect(owners).toEqual([".game-detail-frame"]);
  });
});

describe("GameDetailPanel articles", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });
  afterEach(() => {
    document.body.innerHTML = "";
  });

  const ARTICLES = [
    { kind: "Review", page: 42, issue_key: "mag:en", issue_title: "PC Gamer 1995-02", publication: "PC Gamer", year: 1995, language: "EN" },
    { kind: "Preview", page: 12, issue_key: "mag:en2", issue_title: "PC Gamer 1994-12", publication: "PC Gamer", year: 1994, language: "EN" },
    { kind: "Review", page: 30, issue_key: "mag:de", issue_title: "Power Play 1995-03", publication: "Power Play (DE)", year: 1995, language: "DE" },
  ];

  // A German player picks the German variant, so the German reviews are the
  // ones to see first; the order within a language is eXo's and stays.
  it("leads with German articles when the DE variant is selected", async () => {
    const variants: Game[] = [
      makeGame({ id: 1, language: "EN", installed: false }),
      makeGame({ id: 2, language: "DE", installed: true, torrent_source: "eXoDOS_GLP" }),
    ];
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_variants") { return variants; }
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "game_articles") { return ARTICLES; }
      return null;
    });

    const { host, dispose } = mount(makeGame({ available_languages: "EN:0,DE:2" }));
    await new Promise((r) => setTimeout(r, 0));

    expect(host.ownerDocument.querySelector(".variant-chip.is-selected")?.textContent).toContain("DE");
    const rows = [...host.ownerDocument.querySelectorAll(".game-detail-article")];
    expect(rows.map((r) => r.getAttribute("data-language"))).toEqual(["DE", "EN", "EN"]);
    expect(rows[0].querySelector(".badge")?.textContent).toBe("DE");
    expect(rows[1].querySelector(".badge")).toBeNull();
    expect(rows.map((r) => r.querySelector(".game-detail-article-issue")?.textContent))
      .toEqual(["Power Play 1995-03DE", "PC Gamer 1995-02", "PC Gamer 1994-12"]);
    dispose(); host.remove();
  });

  it("keeps eXo's order for a single-language game", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "game_articles") { return ARTICLES; }
      return null;
    });

    const { host, dispose } = mount(makeGame());
    await new Promise((r) => setTimeout(r, 0));

    const rows = [...host.ownerDocument.querySelectorAll(".game-detail-article")];
    expect(rows.map((r) => r.getAttribute("data-language"))).toEqual(["EN", "EN", "DE"]);
    dispose(); host.remove();
  });
});

describe("GameDetailPanel theme row", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.useFakeTimers();
  });
  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  function withMusic(status: any) {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "start_game_music") { return status; }
      if (cmd === "music_playback_supported") { return { mp3: true, ogg: true }; }
      return null;
    });
  }

  it("says nothing about a game without a theme", async () => {
    withMusic({ phase: "none", progress: 0, total_bytes: 0, path: null, error: null });
    const { dispose } = mount(makeGame({ id: 41 }));
    await vi.advanceTimersByTimeAsync(600);
    expect(document.body.querySelector(".game-detail-music")).toBeNull();
    expect(document.body.textContent).not.toMatch(/no theme|no music/i);
    dispose();
  });

  it("shows the row while the theme is still coming in", async () => {
    withMusic({ phase: "fetching", progress: 0.4, total_bytes: 3_000_000, path: null, error: null });
    const { dispose } = mount(makeGame({ id: 42 }));
    await vi.advanceTimersByTimeAsync(600);
    const row = document.body.querySelector(".game-detail-music");
    expect(row, "a fetch in flight is worth a line").not.toBeNull();
    expect(row!.textContent).toContain("Loading theme 40%");
    dispose();
  });
});

describe("GameDetailPanel - Apple IIGS", () => {
  const INFO = {
    exo_engine: "gsplus", engine: "gsplus", source: "pack", pack_id: "gsplus", note: null,
    options: ["Cribbage King", "Gin King"], selected: "Cribbage King",
    substitute: null, substitute_note: null, printing_unavailable: false, capslock: false,
    extras: [{ name: "Quick_Reference-Card.pdf", size: 1_200_000, cached: false }],
  };
  const iigsGame = () => makeGame({
    id: 77, title: "Cribbage King and Gin King", torrent_source: "eXoAppleIIGS",
    shortcode: "Cribbage King and Gin King (1990)", installed: true, in_library: true,
  });

  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "appleiigs_info") { return INFO; }
      return null;
    });
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.useRealTimers();
  });

  /** The ROM probe repeats while the support files are missing, and a library
   *  refresh hands over a new row object - neither may refetch the info. */
  it("does not refetch its info on support probes or library refreshes", async () => {
    vi.useFakeTimers();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "get_game_metadata") { return EMPTY_META; }
      if (cmd === "get_game_variants") { return []; }
      if (cmd === "appleiigs_info") { return INFO; }
      if (cmd === "get_appleiigs_support_status") { return { phase: "missing", progress: 0, total_bytes: 148_000_000 }; }
      return null;
    });
    const calls = (cmd: string) => mockInvoke.mock.calls.filter((c) => c[0] === cmd).length;
    const [game, setGame] = createSignal(iigsGame());
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <GameDetailPanel game={game()} onClose={() => {}} />, host);
    await vi.advanceTimersByTimeAsync(3200);
    const settled = calls("appleiigs_info");

    setGame(iigsGame());
    await vi.advanceTimersByTimeAsync(25_000);
    expect(calls("get_appleiigs_support_status"), "the probe keeps running").toBeGreaterThan(2);
    expect(calls("appleiigs_info")).toBe(settled);

    dispose();
    host.remove();
  });

  it("offers one chip per program and stores the pick", async () => {
    const { dispose } = mount(iigsGame());
    await new Promise((r) => setTimeout(r, 30));
    const chips = [...document.body.querySelectorAll("[data-testid=iigs-options] .variant-chip")];
    expect(chips.map((c) => c.textContent)).toEqual(["Cribbage King", "Gin King"]);
    expect(chips[0].className).toContain("is-selected");
    (chips[1] as HTMLButtonElement).click();
    await new Promise((r) => setTimeout(r, 0));
    expect(mockInvoke).toHaveBeenCalledWith("set_appleiigs_option", { id: 77, option: "Gin King" });
    dispose();
  });

  it("names the engine, lists the extras and the controls", async () => {
    const { dispose } = mount(iigsGame());
    await new Promise((r) => setTimeout(r, 30));
    const text = document.body.textContent ?? "";
    expect(text).toContain("GSplus");
    expect(text).toContain("Controls");
    const extras = document.body.querySelector("[data-testid=iigs-extras]");
    expect(extras?.textContent).toContain("Quick Reference - Card");
    dispose();
  });
});
