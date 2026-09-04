import { onMount, onCleanup, For, Show, createSignal, createMemo, createEffect } from "solid-js";
import { Portal } from "solid-js/web";
import {
  games, loading, error, hasMore, totalGames,
  fetchGames, fetchMoreGames, fetchAllGames,
  searchQuery,
  hasFetched,
  genreFilter, setGenreFilter,
  sortBy, setSortBy,
  collectionFilter, setCollectionFilter,
  playlistFilter, setPlaylistFilter,
  withMusic, setWithMusic,
  getFavoriteGames,
  lastGameLibraryChange,
} from "../stores/games";
import {
  playlists, userPlaylists, curatedPlaylists, loadPlaylists,
  setPlaylistDialog, deletePlaylist,
} from "../stores/playlists";
import { PackHintBanner } from "../components/PackHintBanner";
import { getGame, getGenres, getInstalledGames, getRecentlyPlayed, getConfig, getAvailableCollections, getSectionKeys, getGames, type CollectionInfo, type Game, type Playlist } from "../api/tauri";
import { GameCard } from "../components/GameCard";
import { GameRow } from "../components/GameRow";
import { viewMode, applyViewMode, loadViewMode, setViewModeTransient, type ViewMode } from "../stores/view";
import { GameDetailPanel } from "../components/GameDetailPanel";
import { PlaylistNameDialog } from "../components/PlaylistNameDialog";
import { CollectionShelf } from "../components/CollectionShelf";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Select } from "../components/Select";
import { showToast } from "../stores/toasts";
import { openGameRequest, setOpenGameRequest, musicUnsupported, refreshMusicIndex } from "../stores/music";
import { matchesLibraryQuery } from "../util";

type Tab = "library" | "browse";

/** Merge fresh DB results into an existing shelf list, preserving the previous
 *  object reference for any game whose state flags didn't change. This keeps
 *  <For>'s reference-based keying stable: unchanged cards don't remount (no
 *  flicker), only cards whose state flipped get re-rendered with new data.
 *
 *  Previously these shelves guarded refresh on ID-set equality - that missed
 *  the case where a pre-favorited game becomes installed: same IDs, different
 *  state, no refresh, stale card. */
function mergeShelfList(prev: Game[], fresh: Game[]): Game[] {
  const prevById = new Map(prev.map(g => [g.id, g]));
  return fresh.map(f => {
    const old = prevById.get(f.id);
    // available_languages is part of the equality check because a variant
    // of a multi-lang game (e.g. DE) can transition independently of the
    // primary row (EN) - its state shows up in the primary's badges via
    // available_languages like "EN:0,DE:2". Without this, shelf cards
    // display stale language badges after installing a sibling variant.
    if (old
      && old.installed === f.installed
      && old.in_library === f.in_library
      && old.favorited === f.favorited
      && old.available_languages === f.available_languages) {
      return old;
    }
    return f;
  });
}

type Section = { label: string; games: Game[]; index: number };

const sortOptions = [
  { value: "title", label: "Title A\u2013Z" },
  { value: "title_desc", label: "Title Z\u2013A" },
  { value: "year_desc", label: "Newest first" },
  { value: "year_asc", label: "Oldest first" },
  { value: "rating", label: "Top rated" },
  { value: "genre", label: "Genre A\u2013Z" },
];

/** List-view columns (#21). `asc`/`desc` are order_clause arms; a column
 *  without `desc` sorts one way only (rating's bucket order is descending by
 *  design, genre mirrors the grid's sort select). Status is display-only. */
const listColumns: { label: string; cls: string; asc?: string; desc?: string }[] = [
  { label: "Title", cls: "row-title", asc: "title", desc: "title_desc" },
  { label: "Year", cls: "row-year", asc: "year_asc", desc: "year_desc" },
  { label: "Genre", cls: "row-genre", asc: "genre" },
  { label: "Developer", cls: "row-dev", asc: "developer", desc: "developer_desc" },
  { label: "Publisher", cls: "row-pub", asc: "publisher", desc: "publisher_desc" },
  { label: "Rating", cls: "row-rating", asc: "rating" },
  { label: "Size", cls: "row-size", asc: "size", desc: "size_desc" },
  { label: "Status", cls: "row-status" },
];

export function Library() {
  let sentinelRef: HTMLDivElement | undefined;
  let libraryRef: HTMLDivElement | undefined;
  const [sectionLabels, setSectionLabels] = createSignal<string[]>([]);
  const [activeTab, setActiveTab] = createSignal<Tab>("browse");
  // Direction of the last tab switch - "right" means new content slides in
  // from the right (forward), "left" from the left (backward). Drives the
  // CSS animation on the freshly mounted tab pane.
  const [tabSlideDir, setTabSlideDir] = createSignal<"right" | "left">("right");
  // The directional class carries `will-change`, so it has to come off once
  // the slide has finished - otherwise the compositor layer is retained for
  // the pane's whole lifetime (#24). The check on `target` matters:
  // animationend bubbles, and a child's animation ending mid-slide would
  // otherwise strip the pane's own animation while it is still moving.
  const [tabSlideDone, setTabSlideDone] = createSignal(false);
  const tabPaneClass = () => tabSlideDone() ? "tab-pane" : `tab-pane tab-pane-${tabSlideDir()}`;
  const onTabSlideEnd = (e: AnimationEvent) => {
    if (e.target === e.currentTarget) { setTabSlideDone(true); }
  };
  const TAB_ORDER: Record<Tab, number> = { browse: 0, library: 1 };
  const switchTab = (tab: Tab) => {
    if (tab === activeTab()) { return; }
    setTabSlideDir(TAB_ORDER[tab] > TAB_ORDER[activeTab()] ? "right" : "left");
    setTabSlideDone(false);
    setActiveTab(tab);
    // The scroll container is shared between tabs - without the reset a
    // back-to-top button made visible in one tab sat orphaned on the other.
    setShowBackToTop(false);
    lastScrollTop = libraryRef?.scrollTop ?? 0;
  };
  const [genres, setGenres] = createSignal<string[]>([]);
  const [recentGames, setRecentGames] = createSignal<Game[]>([]);
  const [installedGames, setInstalledGames] = createSignal<Game[]>([]);
  const [favoriteGames, setFavoriteGames] = createSignal<Game[]>([]);
  const [collections, setCollections] = createSignal<{id: string, label: string, count: number, sub?: string}[]>([]);
  const [detailGame, setDetailGame] = createSignal<Game | null>(null);

  // The player bar's cover asks for the game behind the track it plays.
  createEffect(() => {
    const id = openGameRequest();
    if (id == null) { return; }
    setOpenGameRequest(null);
    getGame(id).then((fresh) => { if (fresh) { setDetailGame(fresh); } }).catch(() => {});
  });

  // Keep detailGame in sync with the games store so installed/in_library flags stay current
  createEffect(() => {
    const dg = detailGame();
    if (!dg?.id) { return; }
    const updated = games().find(g => g.id === dg.id);
    if (updated && (updated.installed !== dg.installed || updated.in_library !== dg.in_library)) {
      setDetailGame(updated);
    }
  });

  // Whenever a game's installed/in_library state changes (download completes
  // or uninstall finishes), refresh the shelves and re-sync the detail panel.
  // The shelves come from separate DB queries, so fetchGames() alone isn't
  // enough. detailGame() can hold an object that no longer matches reality
  // - fetch the fresh row directly by id to be sure.
  createEffect(() => {
    const change = lastGameLibraryChange();
    if (!change) { return; }
    refreshRecent();
    refreshInstalled();
    refreshFavorites();
    // Playlist shelf cards show installed/in_library state too.
    refreshPlaylistShelves();
    const dg = detailGame();
    if (dg?.id === change.id) {
      getGame(change.id).then((fresh) => {
        if (fresh && detailGame()?.id === change.id) {
          setDetailGame(fresh);
        }
      }).catch(() => {});
    }
  });

  // "Back to top" appears on UPWARD scroll only, well below the (non-sticky)
  // collection shelf - scrolling up is the signal the user wants to get back
  // to something above; while reading downwards the button stays out of the
  // way. A small delta filter keeps trackpad jitter from flickering it.
  const [showBackToTop, setShowBackToTop] = createSignal(false);
  let lastScrollTop = 0;
  const onLibraryScroll = () => {
    if (!libraryRef) { return; }
    const top = libraryRef.scrollTop;
    const delta = top - lastScrollTop;
    if (Math.abs(delta) > 2) {
      setShowBackToTop(top > 600 && delta < 0);
      lastScrollTop = top;
    }
  };

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
      case "genre": {
        // genre is a semicolon-joined list ("Action;Adventure;RPG") whose
        // entries can themselves carry " / "-delimited parent/child
        // ("Sports / Baseball"). Sections + jumpbar both key on the
        // *parent* of the FIRST entry so games collapse into ~15 top-level
        // categories - matches the genre filter's tree view and what
        // get_section_keys returns server-side. split() always yields at
        // least one element, so direct [0] indexing is safe.
        const raw = game.genre ?? "";
        const first = raw.split(";")[0].trim();
        const parent = first.split(" / ")[0].trim();
        return parent || "Unknown";
      }
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

  // Sticky top for separators: tab bar (40px) + toolbar (60px). The collection
  // shelf scrolls away with the content, so it never joins the sticky stack.
  const separatorTop = () => "100px";

  const refreshSectionKeys = async () => {
    try {
      const keys = await getSectionKeys(sortBy(), searchQuery(), genreFilter(), collectionFilter(), false, playlistFilter(), withMusic());
      setSectionLabels(keys);
    } catch (e) {
      console.warn("[sectionKeys] failed:", e);
    }
  };

  // Keep the jump bar in sync with the search box: SearchBar triggers
  // fetchGames() itself, but section keys come from a separate query and
  // otherwise go stale (clicking a stale key force-loads all games and then
  // finds no section to scroll to). Runs on mount too.
  createEffect(() => {
    searchQuery();
    refreshSectionKeys();
  });

  // Jump bar labels: prefer backend-supplied (all keys, deduplicated), fall back to loaded sections
  const jumpBarLabels = createMemo(() => {
    const backend = sectionLabels();
    if (backend.length > 0) { return backend; }
    return [...new Set(sections().map(s => s.label).filter(Boolean))];
  });

  // Compact display label for the narrow jump bar. Full label is kept for
  // data-section-label matching (jumpToSection) and the title tooltip.
  const jumpBarDisplayLabel = (label: string): string => {
    // Star ratings: "★★★★☆" → "4", "Unrated" → "?"
    const stars = label.match(/^[★☆]+$/);
    if (stars) {
      return String((label.match(/★/g) || []).length);
    }
    if (label === "Unrated") { return "?"; }
    // Year: "1992" stays "1992" (4 chars fits)
    // Genre: truncate long names for jump bar - keep enough chars to
    // distinguish prefix-sharing genres ("Board Game" vs "Boats", "Puzzle"
    // vs "Puzzle-Solving"). 14 chars covers the bulk of eXoDOS taxonomy.
    if (label.length > 14) { return label.slice(0, 13) + "…"; }
    return label;
  };

  const jumpToSection = async (label: string) => {
    const scroll = () => {
      const el = document.querySelector<HTMLElement>(`[data-section-label="${CSS.escape(label)}"]`);
      if (!el || !libraryRef) { return; }
      // .grid-separator is position:sticky, so its bounding rect reports the
      // stuck position (~separatorTop) once its section has been scrolled past
      // - measuring it directly would make scrollBy a no-op. Measure the
      // adjacent .game-grid sibling (in normal flow) and back out the
      // separator's rendered height to land the letter at the sticky slot.
      const grid = el.nextElementSibling as HTMLElement | null;
      const anchor = grid ?? el;
      const rect = anchor.getBoundingClientRect();
      const containerRect = libraryRef.getBoundingClientRect();
      const sepHeight = grid ? el.offsetHeight : 0;
      const targetTop = parseInt(separatorTop()) || 100;
      libraryRef.scrollBy({
        top: rect.top - containerRect.top - targetTop - sepHeight,
        behavior: "smooth",
      });
    };
    const el = document.querySelector(`[data-section-label="${CSS.escape(label)}"]`);
    if (el) {
      scroll();
    } else {
      await fetchAllGames();
      requestAnimationFrame(scroll);
    }
  };

  // Build a hierarchical option list from the flat genre vocabulary. eXoDOS
  // uses " / " to separate parent/child genres (e.g. "Sports / Baseball").
  // We sort everything alphabetically, then for each group emit the parent
  // header (synthesizing one if all that exist are children) followed by
  // its children at depth 1, indented in the dropdown via Select's depth
  // class. Selecting a parent filters by its prefix (the existing
  // `genre LIKE '%...%'` matcher already covers the subgenre rows).
  const genreOptions = createMemo(() => {
    const flat = genres();
    type Opt = { value: string; label: string; depth?: number; triggerLabel?: string };
    const result: Opt[] = [{ value: "", label: "All Genres" }];

    // Group by first segment. Standalone genres (no " / ") still create a
    // group with an empty children array, so they render as a depth-0 row
    // with no nested entries - same shape as a parent that has children
    // but listed alone in the dropdown. If a parent header only exists via
    // its children (e.g. "Sports / Baseball" without bare "Sports") we
    // still synthesize it; selecting it works because the backend's
    // LIKE '%...%' matcher covers all subgenres.
    const groups = new Map<string, string[]>();
    for (const g of flat) {
      const idx = g.indexOf(" / ");
      if (idx < 0) {
        if (!groups.has(g)) { groups.set(g, []); }
      } else {
        const parent = g.slice(0, idx);
        const child = g.slice(idx + 3);
        const list = groups.get(parent) ?? [];
        list.push(child);
        groups.set(parent, list);
      }
    }

    const sortedParents = [...groups.keys()].sort((a, b) => a.localeCompare(b));
    for (const parent of sortedParents) {
      const children = (groups.get(parent) ?? []).slice().sort((a, b) => a.localeCompare(b));
      result.push({ value: parent, label: parent, depth: 0 });
      for (const child of children) {
        const full = `${parent} / ${child}`;
        result.push({
          value: full,
          label: child,
          depth: 1,
          // Trigger shows full path so the active filter is unambiguous;
          // dropdown row shows only the child name because the parent
          // header already supplies the context.
          triggerLabel: full,
        });
      }
    }

    return result;
  });

  // Dropdown: All Games, then user playlists, then eXo's curated lists.
  // Headers are display-only rows (skipped by keyboard nav / matching).
  const playlistOptions = createMemo(() => {
    type Opt = { value: string; label: string; triggerLabel?: string; header?: boolean };
    const result: Opt[] = [{ value: "", label: "All Games" }];
    const user = userPlaylists();
    const curated = curatedPlaylists();
    if (user.length > 0) {
      result.push({ value: "__user", label: "My Playlists", header: true });
      for (const p of user) {
        result.push({ value: String(p.id), label: `${p.name} (${p.game_count})`, triggerLabel: p.name });
      }
    }
    if (curated.length > 0) {
      result.push({ value: "__curated", label: "Curated", header: true });
      for (const p of curated) {
        result.push({ value: String(p.id), label: `${p.name} (${p.game_count})`, triggerLabel: p.name });
      }
    }
    return result;
  });

  const activePlaylist = createMemo(() =>
    playlists().find((p) => p.id === playlistFilter()) ?? null
  );

  const applyPlaylistFilter = (value: string) => {
    setPlaylistFilter(value ? Number(value) : null);
    fetchGames();
    refreshSectionKeys();
  };

  // The jukebox lives on the rows, so turning the filter on switches to the
  // list - transiently, because that is the chip's doing and not a change of
  // the user's saved preference. Turning it off only undoes the switch if the
  // chip made it: a list the user picked (or picked since) stays.
  let prevViewMode: ViewMode = "grid";
  const toggleWithMusic = () => {
    if (withMusic()) {
      setWithMusic(false);
      if (viewMode() === "list" && prevViewMode === "grid") { setViewModeTransient("grid"); }
    } else {
      prevViewMode = viewMode();
      if (viewMode() === "grid") { setViewModeTransient("list"); }
      setWithMusic(true);
      void refreshMusicIndex();
    }
    fetchGames();
    refreshSectionKeys();
  };

  // My Library: one shelf per user playlist. Fetched per-playlist (small
  // lists) and kept fresh alongside the other shelves.
  const [playlistShelves, setPlaylistShelves] = createSignal<Map<number, Game[]>>(new Map());
  const [shelfMenu, setShelfMenu] = createSignal<{ playlist: Playlist; x: number; y: number } | null>(null);
  const [confirmDelete, setConfirmDelete] = createSignal<Playlist | null>(null);

  // Epoch guard, same idea as stores/games.ts fetchEpoch: a slower
  // pre-mutation batch resolving after a post-mutation one must not
  // overwrite the fresher shelf state.
  let shelvesEpoch = 0;
  const refreshPlaylistShelves = async () => {
    const user = userPlaylists();
    const epoch = ++shelvesEpoch;
    try {
      const entries = await Promise.all(user.map(async (p) => {
        const result = await getGames(1, 500, "", "", "title", "", false, p.id);
        return [p.id, result.games] as const;
      }));
      if (epoch !== shelvesEpoch) { return; }
      setPlaylistShelves((prev) => {
        const next = new Map<number, Game[]>();
        for (const [id, fresh] of entries) {
          next.set(id, mergeShelfList(prev.get(id) ?? [], fresh));
        }
        return next;
      });
    } catch (e) {
      console.warn("[Library] refreshPlaylistShelves failed:", e);
    }
  };

  // Refetch shelves whenever the playlist list changes (reads userPlaylists()
  // synchronously, so the effect tracks the playlists signal). Membership
  // only changes through this app's own mutations - no polling needed.
  createEffect(() => {
    refreshPlaylistShelves();
  });

  // ── Search on My Library ────────────────────────────────────────────────
  // The search box lives in the app's top bar and is visible on both tabs, but
  // it only drove the Browse query - typing while on My Library did nothing.
  // These shelves are already fully in memory, so the filter is a local title
  // match instead of another round trip. Matching is on the merged card's own
  // title; a localized variant title (the German name of an EN-titled card) is
  // not searchable here, unlike Browse where the backend checks every variant.
  const librarySearch = () => searchQuery().trim();
  const filterShelf = (list: Game[]) => {
    const q = librarySearch();
    return q ? list.filter((g) => matchesLibraryQuery(g, q)) : list;
  };

  const shownRecent = createMemo(() => filterShelf(recentGames()));
  const shownFavorites = createMemo(() => filterShelf(favoriteGames()));
  const shownInstalled = createMemo(() => filterShelf(installedGames()));
  const shownPlaylistGames = (playlistId: number) =>
    filterShelf(playlistShelves().get(playlistId) ?? []);
  /** Playlist shelves render even when empty (they carry the "add games" hint),
   *  but a shelf with no search hits is noise - hide it while searching. */
  const shownPlaylists = createMemo(() =>
    librarySearch()
      ? userPlaylists().filter((p) => shownPlaylistGames(p.id).length > 0)
      : userPlaylists()
  );
  const libraryHasMatches = () =>
    shownRecent().length > 0 || shownFavorites().length > 0
    || shownInstalled().length > 0 || shownPlaylists().length > 0;

  // My Library jump bar: one entry per rendered shelf, playlist shelves
  // included - the shelf list can outgrow a screen, and scrolling past
  // three fixed shelves to reach a playlist gets old fast. Tracks the
  // search-filtered lists so it never points at a shelf that isn't there.
  const libraryShelves = createMemo<{ key: string; label: string }[]>(() => {
    const shelves: { key: string; label: string }[] = [];
    if (shownRecent().length > 0) { shelves.push({ key: "recent", label: "Recent" }); }
    if (shownFavorites().length > 0) { shelves.push({ key: "favorites", label: "Favorites" }); }
    if (shownInstalled().length > 0) { shelves.push({ key: "installed", label: "Installed" }); }
    for (const p of shownPlaylists()) {
      shelves.push({ key: `pl-${p.id}`, label: p.name });
    }
    return shelves;
  });

  const jumpToShelf = (key: string) => {
    const el = document.querySelector<HTMLElement>(`[data-shelf-key="${CSS.escape(key)}"]`);
    if (!el || !libraryRef) { return; }
    const rect = el.getBoundingClientRect();
    const containerRect = libraryRef.getBoundingClientRect();
    // Land the shelf title at its sticky slot (tab bar is 40px).
    libraryRef.scrollBy({ top: rect.top - containerRect.top - 40, behavior: "smooth" });
  };

  const handleShelfDelete = async (playlist: Playlist) => {
    setShelfMenu(null);
    try {
      await deletePlaylist(playlist.id);
      showToast(`Deleted "${playlist.name}"`, "success");
      if (playlistFilter() === playlist.id) {
        applyPlaylistFilter("");
      }
    } catch (e) {
      showToast(`Couldn't delete "${playlist.name}"`, "error", { detail: String(e) });
    }
  };

  const refreshRecent = async () => {
    try {
      const fresh = await getRecentlyPlayed(12);
      setRecentGames((prev) => mergeShelfList(prev, fresh));
    } catch (e) {
      console.warn("[Library] refreshRecent failed:", e);
    }
  };

  const refreshInstalled = async () => {
    try {
      const fresh = await getInstalledGames();
      setInstalledGames((prev) => mergeShelfList(prev, fresh));
    } catch (e) {
      console.warn("[Library] refreshInstalled failed:", e);
    }
  };

  const refreshFavorites = async () => {
    try {
      const fresh = await getFavoriteGames();
      setFavoriteGames((prev) => mergeShelfList(prev, fresh));
    } catch (e) {
      console.warn("[Library] refreshFavorites failed:", e);
    }
  };

  const handleFavoriteChanged = (id: number, favorited: boolean) => {
    // NOTE: do NOT call updateGameFavorited here. That creates a new object in
    // games() via spread, which forces <For> to unmount/remount the card whose
    // star was just clicked - visible as a flicker (thumb reloads, etc).
    // The card already tracks favorited state optimistically in its own signal;
    // games() will heal on the next refetch.
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

  onMount(() => {
    // Interval, observer, and onCleanup MUST register synchronously: after the
    // first `await` in an async onMount the reactive owner is gone, so a late
    // onCleanup never runs and the interval/observer leak (and stack up across
    // factory-reset remounts).
    const observer = new IntersectionObserver(
      (entries) => {
        // hasFetched guards the initial mount: hasMore defaults to true, and
        // the sentinel is visible in the empty grid before page 1 loads.
        if (entries[0].isIntersecting && hasFetched() && hasMore() && !loading() && activeTab() === "browse") {
          fetchMoreGames();
        }
      },
      { rootMargin: "400px" }
    );

    if (sentinelRef) { observer.observe(sentinelRef); }

    const interval = setInterval(() => { refreshRecent(); refreshInstalled(); refreshFavorites(); }, 5000);
    onCleanup(() => { clearInterval(interval); observer.disconnect(); });

    loadViewMode();

    (async () => {
      // Load recently played first - if any exist, auto-switch to My Library tab.
      const recent = await getRecentlyPlayed(12).catch(() => [] as Game[]);
      setRecentGames(recent);
      if (recent.length > 0) {
        setActiveTab("library");
      }

      refreshInstalled();
      refreshFavorites();
      loadPlaylists(); // shelf fetch follows via the playlists() effect

      try {
        const [colStr, available] = await Promise.all([
          getConfig("collections"),
          getAvailableCollections(),
        ]);
        if (colStr) {
          const infoMap: Record<string, CollectionInfo> = {};
          for (const c of available) {
            infoMap[c.id] = c;
          }
          const cols: {id: string, label: string, count: number, sub?: string}[] = colStr.split(",")
            .map((id) => ({
              id,
              label: infoMap[id]?.display_name || id,
              count: infoMap[id]?.game_count ?? 0,
            }))
            .sort((a, b) => a.id === "eXoDOS" ? -1 : b.id === "eXoDOS" ? 1 : 0);
          // "All" (empty id = backend's no-collection-filter) leads the shelf:
          // one place to search the entire catalogue across collections. No
          // game count on it - summing the per-collection row counts would
          // double-count merged language variants and disagree with the grid.
          if (cols.length > 1) {
            cols.unshift({
              id: "",
              label: "All",
              count: 0,
              sub: `${cols.length} collections`,
            });
          }
          setCollections(cols);
          if (cols.length > 0 && !collectionFilter()) {
            setCollectionFilter(cols[0].id);
          }
        }
      } catch {}

      refreshGenres();
      fetchGames();
    })();
  });

  const applyFilter = (setter: (v: string) => void) => (value: string) => {
    setter(value);
    fetchGames();
    refreshSectionKeys();
  };

  // Column-header sorting: first click sorts ascending, a second click flips
  // to descending where an arm exists. Same signal as the grid's sort select.
  const sortByColumn = (col: typeof listColumns[number]) => {
    if (!col.asc) { return; }
    const next = sortBy() === col.asc && col.desc ? col.desc : col.asc;
    // Re-clicking a one-way column (Genre, Rating) is a no-op sort change -
    // without this guard it still refetched page 1 and threw away the
    // user's scroll position.
    if (next === sortBy()) { return; }
    applyFilter(setSortBy)(next);
  };

  // The grid cannot represent the column-only sorts: its Select has no such
  // entry (Ark then renders the bare placeholder), groupKey() yields no
  // sections and the jump bar goes empty. Fall back to the default sort
  // instead of leaving the grid in an order none of its controls can show.
  const switchView = (mode: "grid" | "list") => {
    applyViewMode(mode);
    if (mode === "grid" && !sortOptions.some((o) => o.value === sortBy())) {
      applyFilter(setSortBy)("title");
    }
  };

  const columnIndicator = (col: typeof listColumns[number]) => {
    // "rating" is a descending bucket sort by design - show it as such.
    if (sortBy() === col.asc) { return col.asc === "rating" ? " ▼" : " ▲"; }
    if (col.desc && sortBy() === col.desc) { return " ▼"; }
    return "";
  };

  // One instance per toolbar, so the switch sits in the same right-edge spot
  // on both tabs.
  const ViewToggle = () => (
    <div class="view-toggle" role="group" aria-label="View mode">
      <button
        class={`view-toggle-btn ${viewMode() === "grid" ? "active" : ""}`}
        title="Grid view"
        onClick={() => switchView("grid")}
      >▦</button>
      <button
        class={`view-toggle-btn ${viewMode() === "list" ? "active" : ""}`}
        title="List view"
        onClick={() => switchView("list")}
      >☰</button>
    </div>
  );

  // Shelf body in the current view mode. No sortable header here: a shelf's
  // order is its own semantic (recency, install state, playlist order).
  const ShelfGames = (p: { games: Game[] }) => (
    <Show
      when={viewMode() === "list"}
      fallback={
        <div class="game-grid">
          <For each={p.games}>
            {(game) => <GameCard game={game} onFavoriteChanged={handleFavoriteChanged} onDetail={setDetailGame} />}
          </For>
        </div>
      }
    >
      <div class="game-list">
        <For each={p.games}>
          {(game) => <GameRow game={game} onFavoriteChanged={handleFavoriteChanged} onDetail={setDetailGame} />}
        </For>
      </div>
    </Show>
  );

  const switchCollection = (id: string) => {
    setCollectionFilter(id);
    refreshGenres();
    fetchGames();
    refreshSectionKeys();
  };


  return (
    <div class="library" ref={libraryRef} onScroll={onLibraryScroll}>
      {/* ── Tab bar ── */}
      <div class="lib-tabs">
        <button
          class={`lib-tab ${activeTab() === "browse" ? "active" : ""}`}
          onClick={() => switchTab("browse")}
        >
          Browse
          <Show when={totalGames() > 0}>
            <span class="lib-tab-count">{totalGames().toLocaleString()}</span>
          </Show>
        </button>
        <button
          class={`lib-tab ${activeTab() === "library" ? "active" : ""}`}
          onClick={() => switchTab("library")}
        >
          My Library
          <Show when={installedGames().length > 0}>
            <span class={`lib-tab-count ${activeTab() === "library" ? "active" : ""}`}>{installedGames().length} installed</span>
          </Show>
        </button>
      </div>

      {/* ── Browse tab ── */}
      <Show when={activeTab() === "browse"}>
        <div class={tabPaneClass()} onAnimationEnd={onTabSlideEnd}>
        <Show when={collections().length > 1}>
          <div class="collection-bar">
            <CollectionShelf
              collections={collections()}
              active={collectionFilter()}
              onSelect={switchCollection}
            />
          </div>
        </Show>
        <div class="library-toolbar">
          <Show when={genres().length > 1}>
            <Select
              class="select-wide"
              options={genreOptions()}
              value={genreFilter()}
              onChange={applyFilter(setGenreFilter)}
              placeholder="All Genres"
            />
          </Show>
          <Show when={playlists().length > 0}>
            <Select
              class="select-wide"
              options={playlistOptions()}
              value={playlistFilter() != null ? String(playlistFilter()) : ""}
              onChange={applyPlaylistFilter}
              placeholder="Playlists"
            />
          </Show>
          <Show when={!musicUnsupported()}>
            <button
              class={`filter-chip ${withMusic() ? "active" : ""}`}
              onClick={toggleWithMusic}
              title="Only games with a theme track"
            >♪ With theme</button>
          </Show>
          <Show when={viewMode() === "grid"}>
            <Select
              options={sortOptions}
              value={sortBy()}
              onChange={applyFilter(setSortBy)}
              placeholder="Sort by"
            />
          </Show>
          <Show when={totalGames() > 0}>
            <span class="results-count">{totalGames().toLocaleString()} games</span>
          </Show>
          <ViewToggle />
        </div>

        <Show when={activePlaylist()}>
          <div class="playlist-hero">
            <div class="playlist-hero-text">
              <div class="playlist-hero-title">
                {activePlaylist()!.name}
                {/* totalGames(), not game_count: the grid also applies
                    search/genre/collection, and the header must never
                    contradict what's actually rendered below. */}
                <span class="playlist-hero-count">
                  {totalGames().toLocaleString()} games
                </span>
              </div>
              <Show when={activePlaylist()!.description}>
                <div class="playlist-hero-desc">{activePlaylist()!.description}</div>
              </Show>
            </div>
            <button
              class="playlist-hero-clear"
              title="Show all games"
              onClick={() => applyPlaylistFilter("")}
            >✕</button>
          </div>
        </Show>

        {/* In the All view the grid is dominated by eXoDOS covers, so the
            poster-pack nudge keys on eXoDOS there - without the mapping the
            banner never fired again once All became the default (its empty-id
            guard reads "" as "not loaded"). */}
        <PackHintBanner collection={collectionFilter() || "eXoDOS"} />

        <Show when={error()}>
          <div class="error">{error()}</div>
        </Show>

        <Show when={hasFetched() && !loading() && !error() && games().length === 0}>
          <div class="lib-empty">
            <div class="lib-empty-icon">🔍</div>
            <div class="lib-empty-text">
              {searchQuery() ? `No results for "${searchQuery()}"` : "No games match these filters"}
            </div>
            <div class="lib-empty-sub">Try a different search or clear the genre filter</div>
          </div>
        </Show>

        <Show when={viewMode() === "grid"}>
          <div class="sections-list">
            <For each={sections()}>
              {(section) => (
                <>
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
                  <div class="game-grid game-section">
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
                </>
              )}
            </For>
          </div>
        </Show>
        <Show when={viewMode() === "list"}>
          <div class="game-list">
            <div class="game-list-header" style={{ top: separatorTop() }}>
              <span class="row-fav" />
              <span class="row-play" />
              <For each={listColumns}>
                {(col) => (
                  <button
                    class={`list-col ${col.cls}${col.asc ? " sortable" : ""}`}
                    disabled={!col.asc}
                    onClick={() => sortByColumn(col)}
                  >
                    {col.label}{columnIndicator(col)}
                  </button>
                )}
              </For>
            </div>
            <For each={games()}>
              {(game) => (
                <GameRow
                  game={game}
                  onFavoriteChanged={handleFavoriteChanged}
                  onDetail={setDetailGame}
                />
              )}
            </For>
          </div>
        </Show>
        </div>
      </Show>

      {/* ── My Library tab ── */}
      <Show when={activeTab() === "library"}>
        <div class={tabPaneClass()} onAnimationEnd={onTabSlideEnd}>
        {/* Not sticky: the shelf titles stick at the tab bar's edge (top:
            40px) and a sticky toolbar would sit on top of them. */}
        <div class="library-toolbar library-toolbar-plain">
          <ViewToggle />
        </div>
        <Show
          when={libraryHasMatches()}
          fallback={
            <Show
              when={librarySearch()}
              fallback={
                <div class="lib-empty">
                  <div class="lib-empty-icon">🎮</div>
                  <div class="lib-empty-text">No games yet</div>
                  <div class="lib-empty-sub">Switch to Browse to find and download games</div>
                  <button class="lib-empty-btn" onClick={() => switchTab("browse")}>Browse games</button>
                </div>
              }
            >
              {/* Searching with no hits is a different situation from an empty
                  library - offer the whole catalogue instead of "download
                  something first". The query carries over to Browse. */}
              <div class="lib-empty">
                <div class="lib-empty-icon">🔍</div>
                <div class="lib-empty-text">Nothing in your library matches "{searchQuery()}"</div>
                <div class="lib-empty-sub">It may still be in the full collection</div>
                <button class="lib-empty-btn" onClick={() => switchTab("browse")}>Search all games</button>
              </div>
            </Show>
          }
        >
          <Show when={shownRecent().length > 0}>
            <div class="library-section" data-shelf-key="recent">
              <h2 class="section-title">Recently Played <span class="section-count">{shownRecent().length}</span></h2>
              <ShelfGames games={shownRecent()} />
            </div>
          </Show>

          <Show when={shownFavorites().length > 0}>
            <div class="library-section" data-shelf-key="favorites">
              <h2 class="section-title">Favorites <span class="section-count">{shownFavorites().length}</span></h2>
              <ShelfGames games={shownFavorites()} />
            </div>
          </Show>

          <Show when={shownInstalled().length > 0}>
            <div class="library-section" data-shelf-key="installed">
              <h2 class="section-title">Installed <span class="section-count">{shownInstalled().length}</span></h2>
              <ShelfGames games={shownInstalled()} />
            </div>
          </Show>

          <For each={shownPlaylists()}>
            {(playlist) => (
              <div class="library-section" data-shelf-key={`pl-${playlist.id}`}>
                <h2 class="section-title">
                  {playlist.name}
                  <span class="section-count">
                    {librarySearch() ? shownPlaylistGames(playlist.id).length : playlist.game_count}
                  </span>
                  <button
                    class="shelf-menu-btn"
                    title="Playlist options"
                    onClick={(e) => {
                      e.stopPropagation();
                      setShelfMenu({ playlist, x: e.clientX, y: e.clientY });
                    }}
                  >⋯</button>
                </h2>
                <Show
                  when={shownPlaylistGames(playlist.id).length > 0}
                  fallback={
                    <div class="playlist-shelf-empty">
                      Right-click any game and choose "Add to playlist"
                    </div>
                  }
                >
                  <ShelfGames games={shownPlaylistGames(playlist.id)} />
                </Show>
              </div>
            )}
          </For>

          <button
            class="playlist-new-btn"
            onClick={() => setPlaylistDialog({ mode: "create" })}
          >＋ New playlist</button>
        </Show>
        </div>
      </Show>

      <Show when={shelfMenu()}>
        <Portal>
          <div class="context-backdrop" onMouseDown={() => setShelfMenu(null)} onContextMenu={(e) => { e.preventDefault(); setShelfMenu(null); }} />
          <div class="context-menu" style={{ left: `${shelfMenu()!.x}px`, top: `${shelfMenu()!.y}px` }}>
            <button class="context-menu-item" onMouseDown={(e) => e.stopPropagation()} onClick={() => {
              const playlist = shelfMenu()!.playlist;
              setShelfMenu(null);
              setPlaylistDialog({ mode: "rename", playlist });
            }}>
              Rename
            </button>
            <button class="context-menu-item danger" onMouseDown={(e) => e.stopPropagation()} onClick={() => {
              const playlist = shelfMenu()!.playlist;
              setShelfMenu(null);
              setConfirmDelete(playlist);
            }}>
              Delete playlist
            </button>
          </div>
        </Portal>
      </Show>

      <ConfirmDialog
        open={confirmDelete() != null}
        title="Delete playlist"
        message={`Delete "${confirmDelete()?.name}"? The games stay in your library.`}
        confirmLabel="Delete"
        danger
        onConfirm={() => { const p = confirmDelete(); if (p) { handleShelfDelete(p); } }}
        onClose={() => setConfirmDelete(null)}
      />

      <PlaylistNameDialog />

      {/* Infinite scroll sentinel - always mounted */}
      <div ref={sentinelRef} class="scroll-sentinel">
        <Show when={loading()}>
          <div class="loading">Loading...</div>
        </Show>
        <Show when={activeTab() === "browse" && !hasMore() && games().length > 0}>
          <div class="loading">{games().length} / {totalGames()} games</div>
        </Show>
      </div>

      {/* The jump bar targets grid section separators; the list view has a
          sticky column header instead and no sections to jump to. */}
      <Show when={activeTab() === "browse" && viewMode() === "grid" && jumpBarLabels().length > 1}>
        <Portal>
          <div class="jump-bar">
            <For each={jumpBarLabels()}>
              {(label) => (
                <button class="jump-bar-item" title={label} onClick={() => jumpToSection(label)}>
                  {jumpBarDisplayLabel(label)}
                </button>
              )}
            </For>
          </div>
        </Portal>
      </Show>

      <Show when={activeTab() === "library" && libraryShelves().length > 1}>
        <Portal>
          <div class="jump-bar">
            <For each={libraryShelves()}>
              {(shelf) => (
                <button class="jump-bar-item" title={shelf.label} onClick={() => jumpToShelf(shelf.key)}>
                  {jumpBarDisplayLabel(shelf.label)}
                </button>
              )}
            </For>
          </div>
        </Portal>
      </Show>

      {/* Hidden while the detail panel is open: its backdrop sits above the
          button, so a visible-but-dimmed pill would only eat the click that
          closes the panel. */}
      <button
        class={`back-to-top ${showBackToTop() && !detailGame() ? "visible" : ""}`}
        onClick={() => libraryRef?.scrollTo({ top: 0, behavior: "smooth" })}
        aria-hidden={!showBackToTop()}
        tabIndex={showBackToTop() && !detailGame() ? 0 : -1}
      >
        ↑ Top
      </button>

      <GameDetailPanel game={detailGame()} onClose={() => setDetailGame(null)} onDownloadStart={scrollToGame} />
    </div>
  );
}
