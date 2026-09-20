import { createSignal } from "solid-js";
import { getPreviewDir, getPosterDir, getAvailableCollections } from "../api/tauri";

/** The Media Pack's cover set, keyed like a collection's (§19). */
export const MEDIA_SOURCE = "eXoMedia";

// ── Directory caches ─────────────────────────────────────────────────────────

const [previewDirs, setPreviewDirs] = createSignal<Record<string, string>>({});
const [posterDirs, setPosterDirs] = createSignal<Record<string, string>>({});
/** False until the first resolve finishes. Callers that ask "is this
 *  collection on the low-res tier?" would otherwise read the empty startup
 *  state as "yes" and act on it. */
const [dirsLoaded, setDirsLoaded] = createSignal(false);

export { previewDirs, posterDirs, dirsLoaded as thumbnailDirsLoaded };

/** Which collection's art a pack borrows is decided in Rust (`asset_fallback`)
 *  and already baked into the resolved dir, so there is nothing to fall back to
 *  here - a second, string-prefix copy of that rule would only drift from it. */
function dirForCollection(
  dirs: Record<string, string>,
  collectionId: string | null | undefined,
): string | null {
  return dirs[collectionId ?? "eXoDOS"] ?? null;
}

/** Return the Tier 0 preview dir for a collection (bundled, always available). */
export function previewDirForCollection(collectionId: string | null | undefined): string | null {
  return dirForCollection(previewDirs(), collectionId);
}

/** Return the Tier 1 poster dir for a collection (runtime-downloaded). */
export function posterDirForCollection(collectionId: string | null | undefined): string | null {
  return dirForCollection(posterDirs(), collectionId);
}

// ── Best-available-tier resolution ───────────────────────────────────────────

/** The first of `thumbnailCandidates`, or null. */
export function bestThumbnailPath(
  collection: string | null | undefined,
  thumbnailKey: string | null | undefined,
): string | null {
  const [first] = thumbnailCandidates(collection, thumbnailKey);
  return first ?? null;
}

/** Cover paths, Tier 1 (poster pack) then Tier 0 (bundled); the card walks
 *  them on `<img onError>` (§13). Resolved from the dirs on disk, not the
 *  pack ledger. */
export function thumbnailCandidates(
  collection: string | null | undefined,
  thumbnailKey: string | null | undefined,
): string[] {
  if (!thumbnailKey) { return []; }
  const out: string[] = [];
  const posterDir = posterDirForCollection(collection);
  if (posterDir) { out.push(`${posterDir}/${thumbnailKey}.jpg`); }
  const prevDir = previewDirForCollection(collection);
  if (prevDir) { out.push(`${prevDir}/${thumbnailKey}.jpg`); }
  return out;
}

// ── Load / refresh tier directories ──────────────────────────────────────────

/** Called on app startup and after content-pack state changes. */
export async function loadThumbnailDir() {
  try {
    const available = await getAvailableCollections();

    // Resolve Tier 0 preview dirs.
    const previews: Record<string, string> = {};
    const posters: Record<string, string> = {};

    // The Lesesaal's covers ship the same way a collection's do, but its
    // source is not a collection (§19), so it is not in `available`.
    const sources = [...available.map((col) => col.id), MEDIA_SOURCE];
    const results = await Promise.allSettled(
      sources.flatMap((id) => [
        getPreviewDir(id).then((dir) => ({ type: "preview" as const, id, dir })),
        getPosterDir(id).then((dir) => ({ type: "poster" as const, id, dir })),
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
  } finally {
    setDirsLoaded(true);
  }
}
