import { createSignal } from "solid-js";
import { getThumbnailDir } from "../api/tauri";

const [thumbnailDir, setThumbnailDir] = createSignal<string | null>(null);

export { thumbnailDir };

export async function loadThumbnailDir() {
  try {
    // Try eXoDOS first (most games)
    const dir = await getThumbnailDir("eXoDOS");
    setThumbnailDir(dir);
  } catch {
    setThumbnailDir(null);
  }
}
