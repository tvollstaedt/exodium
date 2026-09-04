import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { Game } from "../api/tauri";

const playFromList = vi.fn();
const togglePlay = vi.fn();

/** The store's answer to "is this row worth a play button" - the codec probe
 *  feeds into it, so the row must ask rather than look at `music_file`.
 *  `defaultHint` lives inside the hoisted block because the mock factory runs
 *  before this file's own consts; a second copy outside it drifted. */
const hint = vi.hoisted(() => {
  const defaultHint = (g: { id: number | null; music_file: string | null }) =>
    g.id != null && g.music_file != null && g.music_file.toLowerCase().endsWith(".mp3");
  return { defaultHint, playable: defaultHint };
});

// The real store polls the backend and owns the <audio> port; the row only
// needs the four readers and the two actions it calls.
vi.mock("../stores/music", () => ({
  playableHint: (g: { id: number | null; music_file: string | null }) => hint.playable(g),
  playFromList: (...args: unknown[]) => playFromList(...args),
  togglePlay: () => togglePlay(),
  currentTrack: () => null,
  musicPlaying: () => false,
  musicCached: () => new Set<number>(),
  getMusicState: () => undefined,
}));

const { GameRow } = await import("./GameRow");

const mockInvoke = vi.mocked(invoke);

function makeGame(over: Partial<Game> = {}): Game {
  return {
    id: 1, title: "Descent", sort_title: "Descent", platform: "MS-DOS",
    developer: null, publisher: null, release_date: null, year: 1995,
    genre: "Action", series: null, play_mode: null, rating: null,
    description: null, notes: null, source: null, application_path: null,
    dosbox_conf: null, status: null, region: null, max_players: null,
    language: "EN", shortcode: "DESCENT", torrent_source: "eXoDOS",
    in_library: false, installed: false, game_torrent_index: 1,
    gamedata_torrent_index: null, download_size: 120_000_000,
    has_thumbnail: true, dosbox_variant: null, favorited: false,
    thumbnail_key: "key1", manual_path: null, last_played: null,
    music_file: null, available_languages: null, ...over,
  } as Game;
}

function mount(game: Game) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(() => <GameRow game={game} onDetail={() => {}} />, host);
  return { host, dispose };
}

describe("GameRow play button", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue([]);
    playFromList.mockReset();
    togglePlay.mockReset();
    hint.playable = hint.defaultHint;
  });
  afterEach(() => { document.body.innerHTML = ""; });

  it("offers a play button for a row with a theme track", () => {
    const { host, dispose } = mount(makeGame({ music_file: "Descent.mp3" }));
    expect(host.querySelector(".row-play-btn")).not.toBeNull();
    dispose(); host.remove();
  });

  it("keeps the cell but no button when the game has no theme", () => {
    const { host, dispose } = mount(makeGame());
    expect(host.querySelector(".row-play")).not.toBeNull();
    expect(host.querySelector(".row-play-btn")).toBeNull();
    dispose(); host.remove();
  });

  // The system may not decode the container the catalogue names; the store
  // knows, the row does not, so the button follows the store's answer.
  it("hides the button when the store says the track cannot be played", () => {
    hint.playable = () => false;
    const { host, dispose } = mount(makeGame({ music_file: "Descent.mp3" }));
    expect(host.querySelector(".row-play-btn")).toBeNull();
    dispose(); host.remove();
  });

  it("starts the list queue on click without opening the detail panel", () => {
    const onDetail = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const game = makeGame({ music_file: "Descent.mp3" });
    const dispose = render(() => <GameRow game={game} onDetail={onDetail} />, host);
    const btn = host.querySelector<HTMLButtonElement>(".row-play-btn")!;
    btn.click();
    expect(playFromList).toHaveBeenCalledWith(game);
    expect(togglePlay).not.toHaveBeenCalled();
    dispose(); host.remove();
  });
});
