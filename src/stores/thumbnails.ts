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
  shortcode: string | null | undefined,
  hasThumbnail: boolean,
): string | null {
  if (!shortcode || !hasThumbnail) {
    if (hasThumbnail && !shortcode) {
      console.warn("[thumbnails] miss: has_thumbnail=true but shortcode is empty", { collection });
    }
    return null;
  }

  // Tier 2 (media) would go here when implemented —
  // const mediaDir = mediaDirForCollection(collection);
  // if (mediaDir) { return `${mediaDir}/${shortcode}/poster.jpg`; }

  // Tier 1: poster pack on disk (posterDirForCollection falls back to eXoDOS
  // for LP collections, and returns null if no pack is installed).
  const posterDir = posterDirForCollection(collection);
  if (posterDir) { return `${posterDir}/${shortcode}.jpg`; }

  // Tier 0: bundled preview (always available if has_thumbnail).
  const prevDir = previewDirForCollection(collection);
  if (prevDir) { return `${prevDir}/${shortcode}.jpg`; }

  console.warn("[thumbnails] miss: no tier dir resolved", { collection, shortcode });
  return null;
}

/** Normalize a title into a possible thumbnail filename stem.
 *
 *  Strips all non-alphanumerics (incl. spaces, colons, apostrophes, diacritics
 *  after decomposition). Preserves case because some bundled thumbnails are
 *  title-case-keyed (e.g. `DasAmt.jpg`) rather than shortcode-keyed. Used as
 *  a fallback when the DB shortcode produces a 404 on disk — `GameCard` tries
 *  this after the primary `bestThumbnailPath` img fails to load. */
export function normalizeTitleKey(title: string): string {
  return title
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")  // strip combining marks
    .replace(/[^A-Za-z0-9]/g, "");
}

/** Fallback thumbnail path keyed by normalized title rather than shortcode.
 *  Returns null if no tier dir is available or the title normalises to empty. */
export function titleFallbackThumbnailPath(
  collection: string | null | undefined,
  title: string | null | undefined,
  hasThumbnail: boolean,
): string | null {
  if (!title || !hasThumbnail) { return null; }
  const key = normalizeTitleKey(title);
  if (!key) { return null; }
  const posterDir = posterDirForCollection(collection);
  if (posterDir) { return `${posterDir}/${key}.jpg`; }
  const prevDir = previewDirForCollection(collection);
  if (prevDir) { return `${prevDir}/${key}.jpg`; }
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
