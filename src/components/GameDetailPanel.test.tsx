import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { Game } from "../api/tauri";
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

  /** The preview claims the speakers on play, so it has to give them back the
   *  moment it stops using them. It used to hold the reason until the video
   *  ended or the panel closed, and the lightbox pauses the hero on open - so
   *  looking at a screenshot left the theme silent with nothing playing. */
  it("hands the speakers back when the preview is paused", async () => {
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

    video!.dispatchEvent(new Event("play"));
    expect(music.pauseFor).toHaveBeenCalledWith("video");

    music.resumeFrom.mockClear();
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

    music.pauseFor.mockClear();
    music.resumeFrom.mockClear();

    // Clicking the hero opens the lightbox on entry 0 - the video itself.
    (video as HTMLVideoElement).click();
    await vi.advanceTimersByTimeAsync(0);
    expect(music.pauseFor).toHaveBeenCalledWith("video");

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
