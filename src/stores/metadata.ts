import { createEffect } from "solid-js";
import { getGameMetadata, type GameMetadata } from "../api/tauri";
import { installedPacks } from "./contentPacks";

// Cache keyed "<collection>:<title>". Invalidated whenever the set of
// installed packs changes (install/uninstall), so stale empty results from
// before the metadata pack landed don't stick around.
const EMPTY: GameMetadata = { manual_path: null, manual_kind: null, images: [] };
const cache = new Map<string, GameMetadata>();

createEffect(() => {
  installedPacks();
  cache.clear();
});

export async function loadGameMetadata(
  collection: string | null | undefined,
  title: string | null | undefined,
  shortcode: string | null | undefined,
): Promise<GameMetadata | null> {
  if (!collection || !title) { return null; }
  // Cache key includes title since the backend now matches by title.
  const key = `${collection}:${title}`;
  const hit = cache.get(key);
  if (hit) { return hit; }
  try {
    const fresh = await getGameMetadata(collection, title, shortcode ?? null);
    cache.set(key, fresh);
    return fresh;
  } catch {
    cache.set(key, EMPTY);
    return null;
  }
}
