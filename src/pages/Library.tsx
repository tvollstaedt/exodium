import { onMount, onCleanup, For, Show, createSignal, createMemo } from "solid-js";
import {
  games, loading, error, hasMore, totalGames,
  fetchGames, fetchMoreGames,
  genreFilter, setGenreFilter,
  sortBy, setSortBy,
  collectionFilter, setCollectionFilter,
  getFavoriteGames,
  updateGameFavorited,
} from "../stores/games";
import { getGenres, getInstalledGames, getConfig, getAvailableCollections, type Game } from "../api/tauri";
import { GameCard } from "../components/GameCard";
import { Select } from "../components/Select";

export function Library() {
  let sentinelRef: HTMLDivElement | undefined;
  const [genres, setGenres] = createSignal<string[]>([]);
  const [installedGames, setInstalledGames] = createSignal<Game[]>([]);
  const [favoriteGames, setFavoriteGames] = createSignal<Game[]>([]);
  const [collections, setCollections] = createSignal<{id: string, label: string}[]>([]);

  const genreOptions = createMemo(() => [
    { value: "", label: "All Genres" },
    ...genres().map((g) => ({ value: g, label: g })),
  ]);

  const sortOptions = [
    { value: "title", label: "Title A\u2013Z" },
    { value: "title_desc", label: "Title Z\u2013A" },
    { value: "year_desc", label: "Newest first" },
    { value: "year_asc", label: "Oldest first" },
    { value: "rating", label: "Top rated" },
  ];

  const refreshInstalled = async () => {
    try {
      const installed = await getInstalledGames();
      setInstalledGames(installed);
    } catch {}
  };

  const refreshFavorites = async () => {
    try { setFavoriteGames(await getFavoriteGames()); } catch {}
  };

  const handleFavoriteChanged = (id: number, favorited: boolean) => {
    updateGameFavorited(id, favorited);
    if (!favorited) {
      // Remove immediately — no async round-trip needed
      setFavoriteGames(prev => prev.filter(g => g.id !== id));
    } else {
      // Adding: try to source the game object from the current page; fall back to a fetch
      const game = games().find(g => g.id === id);
      if (game) {
        setFavoriteGames(prev => [...prev, { ...game, favorited: true }]);
      } else {
        refreshFavorites();
      }
    }
  };

  onMount(async () => {
    refreshInstalled();
    refreshFavorites();

    try {
      const gens = await getGenres();
      setGenres(gens);
    } catch {}

    try {
      const [colStr, available] = await Promise.all([
        getConfig("collections"),
        getAvailableCollections(),
      ]);
      if (colStr) {
        const labelMap: Record<string, string> = {};
        for (const c of available) {
          labelMap[c.id] = c.display_name;
        }
        const cols = colStr.split(",").map((id) => ({ id, label: labelMap[id] || id }));
        setCollections(cols);
        // Auto-select first collection so the default view is never the merged "All" view
        if (cols.length > 0 && !collectionFilter()) {
          setCollectionFilter(cols[0].id);
        }
      }
    } catch {}

    fetchGames();

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && hasMore() && !loading()) {
          fetchMoreGames();
        }
      },
      { rootMargin: "400px" }
    );

    if (sentinelRef) observer.observe(sentinelRef);

    const interval = setInterval(() => { refreshInstalled(); refreshFavorites(); }, 5000);
    onCleanup(() => clearInterval(interval));
  });

  const applyFilter = (setter: (v: string) => void) => (value: string) => {
    setter(value);
    fetchGames();
  };

  const switchCollection = (id: string) => {
    setCollectionFilter(id);
    fetchGames();
  };

  return (
    <div class="library">
      <Show when={collections().length > 1}>
        <div class="collection-bar">
          <For each={collections()}>
            {(col) => (
              <button
                class={`collection-btn ${collectionFilter() === col.id ? "active" : ""}`}
                onClick={() => switchCollection(col.id)}
              >{col.label}</button>
            )}
          </For>
        </div>
      </Show>
      <div class={`library-toolbar ${collections().length > 1 ? "has-collection-bar" : ""}`}>
        <Show when={genres().length > 1}>
          <Select
            options={genreOptions()}
            value={genreFilter()}
            onChange={applyFilter(setGenreFilter)}
            placeholder="All Genres"
          />
        </Show>
        <Select
          options={sortOptions}
          value={sortBy()}
          onChange={applyFilter(setSortBy)}
          placeholder="Sort by"
        />
      </div>

      <Show when={error()}>
        <div class="error">{error()}</div>
      </Show>

      <Show when={favoriteGames().length > 0}>
        <div class="library-section">
          <h2 class="section-title">Favorites ({favoriteGames().length})</h2>
          <div class="game-grid">
            <For each={favoriteGames()}>
              {(game) => <GameCard game={game} onFavoriteChanged={handleFavoriteChanged} />}
            </For>
          </div>
        </div>
      </Show>

      <Show when={installedGames().length > 0}>
        <div class="library-section">
          <h2 class="section-title">Installed ({installedGames().length})</h2>
          <div class="game-grid">
            <For each={installedGames()}>
              {(game) => <GameCard game={game} onFavoriteChanged={handleFavoriteChanged} showFavoriteBtn={false} />}
            </For>
          </div>
        </div>
      </Show>

      <div class="library-section">
        <Show when={installedGames().length > 0 || favoriteGames().length > 0}>
          <h2 class="section-title">All Games</h2>
        </Show>
        <div class="game-grid">
          <For each={games()}>
            {(game) => <GameCard game={game} onFavoriteChanged={handleFavoriteChanged} />}
          </For>
        </div>
      </div>

      <div ref={sentinelRef} class="scroll-sentinel">
        <Show when={loading()}>
          <div class="loading">Loading...</div>
        </Show>
        <Show when={!hasMore() && games().length > 0}>
          <div class="loading">{games().length} / {totalGames()} games</div>
        </Show>
      </div>
    </div>
  );
}
