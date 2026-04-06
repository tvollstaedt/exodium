import { createSignal } from "solid-js";
import type { Game, GameList } from "../api/tauri";
import { getGames } from "../api/tauri";

const [games, setGames] = createSignal<Game[]>([]);
const [totalGames, setTotalGames] = createSignal(0);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<string | null>(null);
const [searchQuery, setSearchQuery] = createSignal("");
const [languageFilter, setLanguageFilter] = createSignal("");
const [genreFilter, setGenreFilter] = createSignal("");
const [sortBy, setSortBy] = createSignal("title");
const [collectionFilter, setCollectionFilter] = createSignal("");
const [currentPage, setCurrentPage] = createSignal(1);
const [hasMore, setHasMore] = createSignal(true);

const PER_PAGE = 100;

export {
  games, totalGames, loading, error, hasMore,
  searchQuery, setSearchQuery,
  languageFilter, setLanguageFilter,
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
      1, PER_PAGE, searchQuery(), languageFilter(), genreFilter(), sortBy(), collectionFilter()
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
      nextPage, PER_PAGE, searchQuery(), languageFilter(), genreFilter(), sortBy(), collectionFilter()
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
