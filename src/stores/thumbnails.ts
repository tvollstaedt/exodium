import { createSignal } from "solid-js";
import { getThumbnailDir, getAvailableCollections } from "../api/tauri";

const [thumbnailDirs, setThumbnailDirs] = createSignal<Record<string, string>>({});

export { thumbnailDirs };

/** Return the thumbnail directory for a given torrent_source (collection ID). */
export function thumbnailDirForCollection(collectionId: string | null | undefined): string | null {
  const dirs = thumbnailDirs();
  if (!collectionId) return dirs["eXoDOS"] ?? null;
  return dirs[collectionId] ?? dirs["eXoDOS"] ?? null;
}

export async function loadThumbnailDir() {
  try {
    const available = await getAvailableCollections();
    const results = await Promise.allSettled(
      available.map(async (col) => {
        const dir = await getThumbnailDir(col.id);
        return { id: col.id, dir };
      })
    );
    const dirs: Record<string, string> = {};
    for (const r of results) {
      if (r.status === "fulfilled") {
        dirs[r.value.id] = r.value.dir;
      }
    }
    setThumbnailDirs(dirs);
  } catch {
    setThumbnailDirs({});
  }
}
