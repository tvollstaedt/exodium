import { createEffect } from "solid-js";
import { getGameMetadata, type GameMetadata } from "../api/tauri";
import { installedPacks } from "./contentPacks";
import { lastGameLibraryChange } from "./games";

// Cache keyed "<collection>:<title>". Invalidated whenever the set of
// installed packs changes (install/uninstall) OR a game's library state
// changes (download/uninstall - manual becomes available/unavailable).
const EMPTY: GameMetadata = { manual_path: null, manual_kind: null, images: [], thumbnails: [] };
const cache = new Map<string, GameMetadata>();

createEffect(() => {
  installedPacks();
  lastGameLibraryChange();
  cache.clear();
});

/** Thumbnails live in a cache on disk; once that is cleared the paths here
 *  are dead. */
export function invalidateMetadata() {
  cache.clear();
}

export async function loadGameMetadata(
  collection: string | null | undefined,
  title: string | null | undefined,
  shortcode: string | null | undefined,
  manualPath: string | null | undefined,
  force = false,
): Promise<GameMetadata | null> {
  if (!collection || !title) { return null; }
  // Cache key includes title since the backend now matches by title.
  const key = `${collection}:${title}`;
  if (!force) {
    const hit = cache.get(key);
    if (hit) { return hit; }
  }
  try {
    const fresh = await getGameMetadata(collection, title, shortcode ?? null, manualPath ?? null);
    // A game whose catalog row promises a manual that didn't resolve is a
    // TRANSIENT state - its GameData ZIP (the actual manual source) is
    // usually still downloading. Caching it would pin "No manual" for the
    // whole session even after the download lands.
    if (!(manualPath && !fresh.manual_path)) {
      cache.set(key, fresh);
    }
    return fresh;
  } catch {
    // Same guard as above: a failed invoke for a game whose catalog row
    // promises a manual is transient (startup race, scan hiccup) - pinning
    // EMPTY here kept the Manual button dead for the whole session.
    if (!manualPath) {
      cache.set(key, EMPTY);
    }
    return null;
  }
}
