import { createSignal } from "solid-js";
import { getPreviewDir, getPosterDir, getAvailableCollections } from "../api/tauri";

// ── Directory caches ─────────────────────────────────────────────────────────

const [previewDirs, setPreviewDirs] = createSignal<Record<string, string>>({});
const [posterDirs, setPosterDirs] = createSignal<Record<string, string>>({});

export { previewDirs, posterDirs };

/** Return the Tier 0 preview dir for a collection (bundled, always available). */
export function previewDirForCollection(collectionId: string | null | undefined): string | null {
  const dirs = previewDirs();
  if (!collectionId) { return dirs["eXoDOS"] ?? null; }
  return dirs[collectionId] ?? dirs["eXoDOS"] ?? null;
}

/** Return the Tier 1 poster dir for a collection (runtime-downloaded).
 *  Falls back to eXoDOS since all poster thumbnails live in one directory. */
export function posterDirForCollection(collectionId: string | null | undefined): string | null {
  const dirs = posterDirs();
  if (!collectionId) { return dirs["eXoDOS"] ?? null; }
  return dirs[collectionId] ?? dirs["eXoDOS"] ?? null;
}

// ── Backward compat alias (used by existing callers during migration) ────────

/** @deprecated Use previewDirForCollection or bestThumbnailPath instead. */
export function thumbnailDirForCollection(collectionId: string | null | undefined): string | null {
  return posterDirForCollection(collectionId) ?? previewDirForCollection(collectionId);
}

// ── Best-available-tier resolution ───────────────────────────────────────────

/**
 * Return the best available thumbnail path for a game card.
 *
 * Resolution is based on whether each tier's directory is resolved (i.e. the
 * files physically exist on disk), not on the installedPacks signal. This
 * avoids mismatches where LP games (torrent_source = "eXoDOS_GLP") look for
 * "eXoDOS_GLP:posters" but the installed pack is keyed "eXoDOS:posters".
 *
 * Resolution order:
 *   1. Tier 1 — poster dir available (runtime-downloaded HD box art)
 *   2. Tier 0 — preview dir available (bundled low-quality JPEG)
 *   3. null   — no thumbnail at all (has_thumbnail = false)
 */
export function bestThumbnailPath(
  collection: string | null | undefined,
  thumbnailKey: string | null | undefined,
): string | null {
  if (!thumbnailKey) { return null; }

  // Tier 1: runtime-downloaded poster pack. posterDirForCollection falls back
  // to the eXoDOS pack dir for LP collections (LP variants share EN covers).
  const posterDir = posterDirForCollection(collection);
  if (posterDir) { return `${posterDir}/${thumbnailKey}.jpg`; }

  // Tier 0: bundled preview shipped with the app.
  const prevDir = previewDirForCollection(collection);
  if (prevDir) { return `${prevDir}/${thumbnailKey}.jpg`; }

  return null;
}

// ── Load / refresh tier directories ──────────────────────────────────────────

/** Called on app startup and after content-pack state changes. */
export async function loadThumbnailDir() {
  try {
    const available = await getAvailableCollections();

    // Resolve Tier 0 preview dirs.
    const previews: Record<string, string> = {};
    const posters: Record<string, string> = {};

    const results = await Promise.allSettled(
      available.flatMap((col) => [
        getPreviewDir(col.id).then((dir) => ({ type: "preview" as const, id: col.id, dir })),
        getPosterDir(col.id).then((dir) => ({ type: "poster" as const, id: col.id, dir })),
      ]),
    );

    for (const r of results) {
      if (r.status === "fulfilled") {
        if (r.value.type === "preview") {
          previews[r.value.id] = r.value.dir;
        } else {
          posters[r.value.id] = r.value.dir;
        }
      }
    }

    setPreviewDirs(previews);
    setPosterDirs(posters);
  } catch {
    setPreviewDirs({});
    setPosterDirs({});
  }
}
