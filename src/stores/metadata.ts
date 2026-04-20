import { createEffect } from "solid-js";
import { getGameMetadata, type GameMetadata } from "../api/tauri";
import { installedPacks } from "./contentPacks";
import { lastGameLibraryChange } from "./games";

// Cache keyed "<collection>:<title>". Invalidated whenever the set of
// installed packs changes (install/uninstall) OR a game's library state
// changes (download/uninstall — manual becomes available/unavailable).
const EMPTY: GameMetadata = { manual_path: null, manual_kind: null, images: [] };
const cache = new Map<string, GameMetadata>();

createEffect(() => {
  installedPacks();
  lastGameLibraryChange();
  cache.clear();
});

export async function loadGameMetadata(
  collection: string | null | undefined,
  title: string | null | undefined,
  shortcode: string | null | undefined,
  manualPath: string | null | undefined,
): Promise<GameMetadata | null> {
  if (!collection || !title) { return null; }
  // Cache key includes title since the backend now matches by title.
  const key = `${collection}:${title}`;
  const hit = cache.get(key);
  if (hit) { return hit; }
  try {
    const fresh = await getGameMetadata(collection, title, shortcode ?? null, manualPath ?? null);
    cache.set(key, fresh);
    return fresh;
  } catch {
    cache.set(key, EMPTY);
    return null;
  }
}
