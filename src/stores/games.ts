import { createSignal } from "solid-js";
import type { Game, GameList } from "../api/tauri";
import { getGames, toggleFavorite } from "../api/tauri";

export { toggleFavorite };

export function updateGameFavorited(id: number, value: boolean) {
  setGames(prev => prev.map(g => g.id === id ? { ...g, favorited: value } : g));
}

export async function getFavoriteGames(): Promise<Game[]> {
  const result = await getGames(1, 500, "", "", "title", "", true);
  return result.games;
}

const [games, setGames] = createSignal<Game[]>([]);
const [totalGames, setTotalGames] = createSignal(0);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<string | null>(null);
const [searchQuery, setSearchQuery] = createSignal("");
const [genreFilter, setGenreFilter] = createSignal("");
const [sortBy, setSortBy] = createSignal("title");
const [collectionFilter, setCollectionFilter] = createSignal("");
const [currentPage, setCurrentPage] = createSignal(1);
const [hasMore, setHasMore] = createSignal(true);

const PER_PAGE = 100;

export {
  games, totalGames, loading, error, hasMore,
  searchQuery, setSearchQuery,
  genreFilter, setGenreFilter,
  sortBy, setSortBy,
  collectionFilter, setCollectionFilter,
};

/// Fetch the first page (resets the list).
export async function fetchGames() {
  setLoading(true);
  setError(null);
  setCurrentPage(1);
  try {
    const result: GameList = await getGames(
      1, PER_PAGE, searchQuery(), genreFilter(), sortBy(), collectionFilter()
    );
    setGames(result.games);
    setTotalGames(result.total);
    setHasMore(result.games.length < result.total);
  } catch (e) {
    setError(e instanceof Error ? e.message : String(e));
  } finally {
    setLoading(false);
  }
}

/// Fetch the next page and append to existing list.
export async function fetchMoreGames() {
  if (loading() || !hasMore()) return;
  setLoading(true);
  const nextPage = currentPage() + 1;
  try {
    const result: GameList = await getGames(
      nextPage, PER_PAGE, searchQuery(), genreFilter(), sortBy(), collectionFilter()
    );
    setGames((prev) => [...prev, ...result.games]);
    setCurrentPage(nextPage);
    setHasMore(games().length < result.total);
  } catch (e) {
    setError(e instanceof Error ? e.message : String(e));
  } finally {
    setLoading(false);
  }
}

/// Load all games at once — used by jumpToSection when the target section isn't rendered yet.
export async function fetchAllGames() {
  if (loading()) { return; }
  setLoading(true);
  try {
    const result: GameList = await getGames(1, totalGames() || 9999, searchQuery(), genreFilter(), sortBy(), collectionFilter());
    setGames(result.games);
    setHasMore(false);
  } catch (e) {
    setError(e instanceof Error ? e.message : String(e));
  } finally {
    setLoading(false);
  }
}
