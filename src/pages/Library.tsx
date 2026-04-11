import { onMount, onCleanup, For, Show, createSignal, createMemo, createEffect } from "solid-js";
import { Portal } from "solid-js/web";
import {
  games, loading, error, hasMore, totalGames,
  fetchGames, fetchMoreGames, fetchAllGames,
  searchQuery,
  genreFilter, setGenreFilter,
  sortBy, setSortBy,
  collectionFilter, setCollectionFilter,
  getFavoriteGames,
  updateGameFavorited,
} from "../stores/games";
import { getGenres, getInstalledGames, getConfig, getAvailableCollections, getSectionKeys, type Game } from "../api/tauri";
import { GameCard } from "../components/GameCard";
import { GameDetailPanel } from "../components/GameDetailPanel";
import { Select } from "../components/Select";

type Tab = "library" | "browse";

const gameIds = (games: Game[]) => games.map(g => g.id).sort().join(",");

type Section = { label: string; games: Game[]; index: number };

const sortOptions = [
  { value: "title", label: "Title A\u2013Z" },
  { value: "title_desc", label: "Title Z\u2013A" },
  { value: "year_desc", label: "Newest first" },
  { value: "year_asc", label: "Oldest first" },
  { value: "rating", label: "Top rated" },
  { value: "genre", label: "Genre A\u2013Z" },
];

export function Library() {
  let sentinelRef: HTMLDivElement | undefined;
  let libraryRef: HTMLDivElement | undefined;
  const [sectionLabels, setSectionLabels] = createSignal<string[]>([]);
  const [activeTab, setActiveTab] = createSignal<Tab>("browse");
  const [genres, setGenres] = createSignal<string[]>([]);
  const [installedGames, setInstalledGames] = createSignal<Game[]>([]);
  const [favoriteGames, setFavoriteGames] = createSignal<Game[]>([]);
  const [collections, setCollections] = createSignal<{id: string, label: string}[]>([]);
  const [detailGame, setDetailGame] = createSignal<Game | null>(null);

  // Keep detailGame in sync with the games store so installed/in_library flags stay current
  createEffect(() => {
    const dg = detailGame();
    if (!dg?.id) { return; }
    const updated = games().find(g => g.id === dg.id);
    if (updated && (updated.installed !== dg.installed || updated.in_library !== dg.in_library)) {
      setDetailGame(updated);
    }
  });

  const scrollToGame = (gameId: number) => {
    requestAnimationFrame(() => {
      const el = document.querySelector<HTMLElement>(`[data-game-id="${gameId}"]`);
      el?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  };

  // Compute separator label for a game based on current sort
  const groupKey = (game: Game): string => {
    switch (sortBy()) {
      case "title":
      case "title_desc": {
        const first = (game.sort_title ?? game.title)[0]?.toUpperCase() ?? "";
        return /[A-Z]/.test(first) ? first : "#";
      }
      case "year_asc":
      case "year_desc":
        return game.year != null ? String(game.year) : "Unknown";
      case "rating": {
        if (game.rating == null) { return "Unrated"; }
        const n = Math.round(game.rating);
        return "★".repeat(Math.max(0, n)) + "☆".repeat(Math.max(0, 5 - n));
      }
      case "genre":
        return game.genre ?? "Unknown";
      default:
        return "";
    }
  };

  // Group games into labelled sections; recomputes when games() or sortBy() changes
  const sections = createMemo<Section[]>(() => {
    const result: Section[] = [];
    let current: Section | null = null;
    for (const g of games()) {
      const key = groupKey(g);
      if (current === null || key !== current.label) {
        current = { label: key, games: [], index: result.length };
        result.push(current);
      }
      current.games.push(g);
    }
    return result;
  });

  // Sticky top for separators: tab bar (40px) + toolbar (60px) [+ collection bar (40px) if visible]
  const separatorTop = () => collections().length > 1 ? "140px" : "100px";

  const refreshSectionKeys = async () => {
    try {
      const keys = await getSectionKeys(sortBy(), searchQuery(), genreFilter(), collectionFilter(), false);
      if (keys.length > 0) { setSectionLabels(keys); }
    } catch (e) {
      console.warn("[sectionKeys] failed:", e);
    }
  };

  // Jump bar labels: prefer backend-supplied (all keys, deduplicated), fall back to loaded sections
  const jumpBarLabels = createMemo(() => {
    const backend = sectionLabels();
    if (backend.length > 0) { return backend; }
    return [...new Set(sections().map(s => s.label).filter(Boolean))];
  });

  const jumpToSection = async (label: string) => {
    const scroll = () => {
      const el = document.querySelector<HTMLElement>(`[data-section-label="${CSS.escape(label)}"]`);
      if (!el || !libraryRef) { return; }
      const rect = el.getBoundingClientRect();
      const containerRect = libraryRef.getBoundingClientRect();
      libraryRef.scrollBy({ top: rect.top - containerRect.top - (parseInt(separatorTop()) || 100), behavior: "smooth" });
    };
    const el = document.querySelector(`[data-section-label="${CSS.escape(label)}"]`);
    if (el) {
      scroll();
    } else {
      await fetchAllGames();
      requestAnimationFrame(scroll);
    }
  };

  const genreOptions = createMemo(() => [
    { value: "", label: "All Genres" },
    ...genres().map((g) => ({ value: g, label: g })),
  ]);

  const refreshInstalled = async () => {
    try {
      const installed = await getInstalledGames();
      if (gameIds(installed) !== gameIds(installedGames())) { setInstalledGames(installed); }
    } catch {}
  };

  const refreshFavorites = async () => {
    try {
      const favs = await getFavoriteGames();
      if (gameIds(favs) !== gameIds(favoriteGames())) { setFavoriteGames(favs); }
    } catch {}
  };

  const handleFavoriteChanged = (id: number, favorited: boolean) => {
    updateGameFavorited(id, favorited);
    if (!favorited) {
      setFavoriteGames(prev => prev.filter(g => g.id !== id));
    } else {
      const game = games().find(g => g.id === id);
      if (game) {
        setFavoriteGames(prev => [...prev, { ...game, favorited: true }]);
      } else {
        refreshFavorites();
      }
    }
  };

  const refreshGenres = async () => {
    try {
      setGenres(await getGenres(collectionFilter() || ""));
    } catch {}
  };

  onMount(async () => {
    refreshInstalled();
    refreshFavorites();

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
        const cols = colStr.split(",")
          .map((id) => ({ id, label: labelMap[id] || id }))
          .sort((a, b) => a.id === "eXoDOS" ? -1 : b.id === "eXoDOS" ? 1 : 0);
        setCollections(cols);
        if (cols.length > 0 && !collectionFilter()) {
          setCollectionFilter(cols[0].id);
        }
      }
    } catch {}

    refreshGenres();
    fetchGames();
    refreshSectionKeys();

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting && hasMore() && !loading() && activeTab() === "browse") {
          fetchMoreGames();
        }
      },
      { rootMargin: "400px" }
    );

    if (sentinelRef) { observer.observe(sentinelRef); }

    const interval = setInterval(() => { refreshInstalled(); refreshFavorites(); }, 5000);
    onCleanup(() => { clearInterval(interval); observer.disconnect(); });
  });

  const applyFilter = (setter: (v: string) => void) => (value: string) => {
    setter(value);
    fetchGames();
    refreshSectionKeys();
  };

  const switchCollection = (id: string) => {
    setCollectionFilter(id);
    refreshGenres();
    fetchGames();
    refreshSectionKeys();
  };

  return (
    <div class="library" ref={libraryRef}>
      {/* ── Tab bar ── */}
      <div class="lib-tabs">
        <button
          class={`lib-tab ${activeTab() === "browse" ? "active" : ""}`}
          onClick={() => setActiveTab("browse")}
        >
          Browse
          <Show when={totalGames() > 0}>
            <span class="lib-tab-count">{totalGames().toLocaleString()}</span>
          </Show>
        </button>
        <button
          class={`lib-tab ${activeTab() === "library" ? "active" : ""}`}
          onClick={() => setActiveTab("library")}
        >
          My Library
          <Show when={installedGames().length > 0}>
            <span class={`lib-tab-count ${activeTab() === "library" ? "active" : ""}`}>{installedGames().length} installed</span>
          </Show>
        </button>
      </div>

      {/* ── Browse tab ── */}
      <Show when={activeTab() === "browse"}>
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
              class="select-wide"
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
          <Show when={totalGames() > 0}>
            <span class="results-count">{totalGames().toLocaleString()} games</span>
          </Show>
        </div>

        <Show when={error()}>
          <div class="error">{error()}</div>
        </Show>

        <div class="sections-list">
          <For each={sections()}>
            {(section) => (
              <div class="game-section">
                <Show when={section.label}>
                  <div
                    id={`sep-${section.index}`}
                    data-section-label={section.label}
                    class="grid-separator"
                    style={{ top: separatorTop() }}
                  >
                    {section.label}
                  </div>
                </Show>
                <div class="game-grid">
                  <For each={section.games}>
                    {(game) => (
                      <GameCard
                        game={game}
                        onFavoriteChanged={handleFavoriteChanged}
                        onDetail={setDetailGame}
                      />
                    )}
                  </For>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* ── My Library tab ── */}
      <Show when={activeTab() === "library"}>
        <Show
          when={favoriteGames().length > 0 || installedGames().length > 0}
          fallback={
            <div class="lib-empty">
              <div class="lib-empty-icon">🎮</div>
              <div class="lib-empty-text">No games yet</div>
              <div class="lib-empty-sub">Switch to Browse to find and download games</div>
              <button class="lib-empty-btn" onClick={() => setActiveTab("browse")}>Browse games</button>
            </div>
          }
        >
          <Show when={favoriteGames().length > 0}>
            <div class="library-section">
              <h2 class="section-title">Favorites <span class="section-count">{favoriteGames().length}</span></h2>
              <div class="game-grid">
                <For each={favoriteGames()}>
                  {(game) => <GameCard game={game} onFavoriteChanged={handleFavoriteChanged} onDetail={setDetailGame} />}
                </For>
              </div>
            </div>
          </Show>

          <Show when={installedGames().length > 0}>
            <div class="library-section">
              <h2 class="section-title">Installed <span class="section-count">{installedGames().length}</span></h2>
              <div class="game-grid">
                <For each={installedGames()}>
                  {(game) => <GameCard game={game} onFavoriteChanged={handleFavoriteChanged} showFavoriteBtn={false} onDetail={setDetailGame} />}
                </For>
              </div>
            </div>
          </Show>
        </Show>
      </Show>

      {/* Infinite scroll sentinel — always mounted */}
      <div ref={sentinelRef} class="scroll-sentinel">
        <Show when={loading()}>
          <div class="loading">Loading...</div>
        </Show>
        <Show when={activeTab() === "browse" && !hasMore() && games().length > 0}>
          <div class="loading">{games().length} / {totalGames()} games</div>
        </Show>
      </div>

      <Show when={activeTab() === "browse" && jumpBarLabels().length > 1}>
        <Portal>
          <div class="jump-bar">
            <For each={jumpBarLabels()}>
              {(label) => (
                <button class="jump-bar-item" title={label} onClick={() => jumpToSection(label)}>
                  {label}
                </button>
              )}
            </For>
          </div>
        </Portal>
      </Show>

      <GameDetailPanel game={detailGame()} onClose={() => setDetailGame(null)} onDownloadStart={scrollToGame} />
    </div>
  );
}
