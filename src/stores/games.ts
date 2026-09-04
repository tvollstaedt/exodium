import { createSignal } from "solid-js";
import type { Game, GameList } from "../api/tauri";
import { getGames, toggleFavorite } from "../api/tauri";

export { toggleFavorite };

export function updateGameFavorited(id: number, value: boolean) {
  setGames(prev => prev.map(g => g.id === id ? { ...g, favorited: value } : g));
}

export async function getFavoriteGames(): Promise<Game[]> {
  const result = await getGames(1, 500, "", "", "title", "", true, null, false);
  return result.games;
}

// Bumped with the gameId whenever a game's installed/in_library state changes
// (download-install complete, uninstall). Consumers (Library shelves, detail
// panel) watch this to refresh derived views that come from separate DB
// queries. The value is `{ id, ts }` so Solid treats every change as distinct
// even when the same game is uninstalled-then-reinstalled in rapid succession.
const [lastGameLibraryChange, setLastGameLibraryChange] =
  createSignal<{ id: number; ts: number } | null>(null);
export { lastGameLibraryChange };
export function notifyGameLibraryChanged(id: number) {
  setLastGameLibraryChange({ id, ts: Date.now() });
}

const [games, setGames] = createSignal<Game[]>([]);
const [totalGames, setTotalGames] = createSignal(0);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<string | null>(null);
const [searchQuery, setSearchQuery] = createSignal("");
const [genreFilter, setGenreFilter] = createSignal("");
const [sortBy, setSortBy] = createSignal("title");
const [collectionFilter, setCollectionFilter] = createSignal("");
const [playlistFilter, setPlaylistFilter] = createSignal<number | null>(null);
const [currentPage, setCurrentPage] = createSignal(1);
const [hasMore, setHasMore] = createSignal(true);
// "Only games with a theme track". Session-only on purpose: it narrows the
// catalogue hard, and finding Browse still filtered after a restart would
// read as missing games rather than as a filter.
const [withMusic, setWithMusic] = createSignal(false);

const PER_PAGE = 100;

// Monotonic epoch: every list mutation bumps it before awaiting and discards
// its response if another fetch started meanwhile - a slow refreshLoadedGames
// response must not overwrite a newer filter result or drop an appended page.
let fetchEpoch = 0;
const [hasFetched, setHasFetched] = createSignal(false);
export { hasFetched };

export {
  games, totalGames, loading, error, hasMore,
  searchQuery, setSearchQuery,
  genreFilter, setGenreFilter,
  sortBy, setSortBy,
  collectionFilter, setCollectionFilter,
  playlistFilter, setPlaylistFilter,
  withMusic, setWithMusic,
};

/// Fetch the first page (resets the list).
export async function fetchGames() {
  const epoch = ++fetchEpoch;
  setLoading(true);
  setError(null);
  setCurrentPage(1);
  try {
    const result: GameList = await getGames(
      1, PER_PAGE, searchQuery(), genreFilter(), sortBy(), collectionFilter(), false, playlistFilter(), withMusic()
    );
    if (epoch !== fetchEpoch) { return; }
    setGames(result.games);
    setTotalGames(result.total);
    setHasMore(result.games.length < result.total);
  } catch (e) {
    if (epoch === fetchEpoch) { setError(e instanceof Error ? e.message : String(e)); }
  } finally {
    setHasFetched(true);
    if (epoch === fetchEpoch) { setLoading(false); }
  }
}

/// Re-fetch every already-loaded row in one request and swap the list in
/// place. For background changes (install finished, uninstall) - a plain
/// fetchGames() would reset infinite scroll to page 1 and yank the user's
/// Browse position while they're reading.
export async function refreshLoadedGames() {
  const count = games().length;
  if (count === 0) {
    return fetchGames();
  }
  if (loading()) {
    // A page fetch is in flight; replacing the list now could drop or
    // duplicate rows. The next library change will refresh again.
    return;
  }
  const epoch = ++fetchEpoch;
  try {
    const result: GameList = await getGames(
      1, Math.max(count, PER_PAGE), searchQuery(), genreFilter(), sortBy(), collectionFilter(), false, playlistFilter(), withMusic()
    );
    if (epoch !== fetchEpoch) { return; }
    setGames(result.games);
    setTotalGames(result.total);
    setHasMore(result.games.length < result.total);
  } catch (e) {
    // Background refresh - don't surface an error banner over a working list.
    console.error("[games] background refresh failed:", e);
  }
}

/// Fetch the next page and append to existing list.
export async function fetchMoreGames() {
  if (loading() || !hasMore()) return;
  const epoch = ++fetchEpoch;
  setLoading(true);
  const nextPage = currentPage() + 1;
  try {
    const result: GameList = await getGames(
      nextPage, PER_PAGE, searchQuery(), genreFilter(), sortBy(), collectionFilter(), false, playlistFilter(), withMusic()
    );
    if (epoch !== fetchEpoch) { return; }
    setGames((prev) => [...prev, ...result.games]);
    setCurrentPage(nextPage);
    setHasMore(games().length < result.total);
  } catch (e) {
    if (epoch === fetchEpoch) { setError(e instanceof Error ? e.message : String(e)); }
  } finally {
    if (epoch === fetchEpoch) { setLoading(false); }
  }
}

/// Load all games at once - used by jumpToSection when the target section isn't rendered yet.
export async function fetchAllGames() {
  if (loading()) { return; }
  const epoch = ++fetchEpoch;
  setLoading(true);
  try {
    const result: GameList = await getGames(1, totalGames() || 9999, searchQuery(), genreFilter(), sortBy(), collectionFilter(), false, playlistFilter(), withMusic());
    if (epoch !== fetchEpoch) { return; }
    setGames(result.games);
    setHasMore(false);
  } catch (e) {
    if (epoch === fetchEpoch) { setError(e instanceof Error ? e.message : String(e)); }
  } finally {
    if (epoch === fetchEpoch) { setLoading(false); }
  }
}
