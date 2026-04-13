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

/** Derive a candidate thumbnail filename stem from a title, mirroring the
 *  Rust `generate_shortcode()` in src-tauri/src/bin/generate_db.rs:
 *    - decompose diacritics and strip combining marks
 *    - keep only ASCII alphanumerics
 *    - truncate to the first 8 characters
 *
 *  This matches the LP-exclusive bundled thumbnails like `DasAmt.jpg`,
 *  `BerlinWa.jpg` (from "Berlin Wall"), etc. Used as a fallback when the
 *  DB shortcode either doesn't match a bundled file or is missing entirely
 *  (e.g. has_thumbnail=0 for LP-exclusive games whose DB build didn't see
 *  the file on disk but the bundled pack actually contains it). */
export function normalizeTitleKey(title: string): string {
  const stripped = title
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^A-Za-z0-9]/g, "");
  return stripped.slice(0, 8);
}

/** Fallback thumbnail path keyed by normalized title. Intentionally ignores
 *  `hasThumbnail` — the DB's has_thumbnail flag only reflects files the build
 *  pipeline matched, but the bundle may contain additional files generated
 *  under `generate_shortcode()` rules that the LP backfill missed. Worth a
 *  speculative lookup; the browser's <img onError> handles the miss. */
export function titleFallbackThumbnailPath(
  collection: string | null | undefined,
  title: string | null | undefined,
): string | null {
  if (!title) { return null; }
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
