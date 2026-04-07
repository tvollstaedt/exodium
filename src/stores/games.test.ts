import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

const mockInvoke = vi.mocked(invoke);

function makeGame(overrides: Partial<{ id: number; title: string; favorited: boolean; language: string }> = {}) {
  return {
    id: overrides.id ?? 1,
    title: overrides.title ?? "Doom",
    sort_title: null,
    platform: "MS-DOS",
    developer: null,
    publisher: null,
    release_date: null,
    year: 1993,
    genre: "Action;Shooter",
    series: null,
    play_mode: null,
    rating: null,
    description: null,
    notes: null,
    source: null,
    application_path: null,
    dosbox_conf: null,
    status: null,
    region: null,
    max_players: null,
    language: overrides.language ?? "EN",
    shortcode: null,
    available_languages: null,
    torrent_source: "eXoDOS",
    in_library: false,
    installed: false,
    favorited: overrides.favorited ?? false,
    game_torrent_index: null,
    gamedata_torrent_index: null,
    download_size: null,
    has_thumbnail: false,
    dosbox_variant: null,
  };
}

beforeEach(() => {
  mockInvoke.mockReset();
  mockInvoke.mockResolvedValue({ games: [], total: 0 });
});

describe("games store", () => {
  it("fetchGames passes current filter state to invoke", async () => {
    mockInvoke.mockResolvedValue({ games: [makeGame()], total: 1 });

    const { fetchGames, setSearchQuery, setGenreFilter, setSortBy, setCollectionFilter } =
      await import("./games");

    setSearchQuery("doom");
    setGenreFilter("Action");
    setSortBy("year_desc");
    setCollectionFilter("eXoDOS");

    await fetchGames();

    expect(mockInvoke).toHaveBeenCalledWith(
      "get_games",
      expect.objectContaining({
        query: "doom",
        genre: "Action",
        sortBy: "year_desc",
        collection: "eXoDOS",
      })
    );
  });

  it("fetchGames updates games and totalGames signals", async () => {
    const game = makeGame({ id: 42, title: "Space Quest V" });
    mockInvoke.mockResolvedValue({ games: [game], total: 1 });

    const { fetchGames, games, totalGames, setSearchQuery, setGenreFilter, setSortBy, setCollectionFilter } =
      await import("./games");

    // Reset filters so we don't inherit values from previous test
    setSearchQuery("");
    setGenreFilter("");
    setSortBy("title");
    setCollectionFilter("");

    await fetchGames();

    expect(games().length).toBeGreaterThanOrEqual(1);
    expect(games().some(g => g.title === "Space Quest V")).toBe(true);
    expect(totalGames()).toBeGreaterThanOrEqual(1);
  });

  it("updateGameFavorited flips the favorited flag in local signal", async () => {
    const game = makeGame({ id: 77, title: "Quake", favorited: false });
    mockInvoke.mockResolvedValue({ games: [game], total: 1 });

    const { fetchGames, games, updateGameFavorited, setSearchQuery, setGenreFilter, setSortBy, setCollectionFilter } =
      await import("./games");

    setSearchQuery("Quake");
    setGenreFilter("");
    setSortBy("title");
    setCollectionFilter("");

    await fetchGames();

    const before = games().find(g => g.id === 77);
    expect(before).toBeDefined();

    updateGameFavorited(77, true);

    const after = games().find(g => g.id === 77);
    expect(after?.favorited).toBe(true);
  });

  it("getFavoriteGames calls invoke with favoritesOnly=true", async () => {
    mockInvoke.mockResolvedValue({ games: [], total: 0 });

    const { getFavoriteGames } = await import("./games");
    await getFavoriteGames();

    expect(mockInvoke).toHaveBeenCalledWith(
      "get_games",
      expect.objectContaining({ favoritesOnly: true })
    );
  });
});
