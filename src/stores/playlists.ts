import { createSignal } from "solid-js";
import {
  getPlaylists, createPlaylist as apiCreate, renamePlaylist as apiRename,
  deletePlaylist as apiDelete, setPlaylistMembership, type Playlist,
} from "../api/tauri";

const [playlists, setPlaylists] = createSignal<Playlist[]>([]);
export { playlists };

export const userPlaylists = () => playlists().filter(p => p.kind === "user");
export const curatedPlaylists = () => playlists().filter(p => p.kind === "curated");

// Single app-wide name dialog (mounted once in Library): "create" optionally
// carries a game to add to the fresh playlist; "rename" carries the playlist.
export type PlaylistDialogRequest =
  | { mode: "create"; gameId?: number }
  | { mode: "rename"; playlist: Playlist };
const [playlistDialog, setPlaylistDialog] = createSignal<PlaylistDialogRequest | null>(null);
export { playlistDialog, setPlaylistDialog };

export async function loadPlaylists(): Promise<void> {
  try {
    const fresh = await getPlaylists();
    // Keep unchanged objects: `<For>` keys shelves by reference.
    setPlaylists((prev) => {
      const prevById = new Map(prev.map((p) => [p.id, p]));
      return fresh.map((f) => {
        const old = prevById.get(f.id);
        if (old
          && old.name === f.name
          && old.kind === f.kind
          && old.description === f.description
          && old.game_count === f.game_count) {
          return old;
        }
        return f;
      });
    });
  } catch (e) {
    console.warn("[playlists] load failed:", e);
  }
}

// Fire-and-forget after a mutation: the write is what the caller awaits.
function refreshInBackground() {
  loadPlaylists();
}

/// Create and return the new playlist's id.
export async function createPlaylist(name: string): Promise<number> {
  const id = await apiCreate(name);
  refreshInBackground();
  return id;
}

export async function renamePlaylist(id: number, name: string): Promise<void> {
  await apiRename(id, name);
  refreshInBackground();
}

export async function deletePlaylist(id: number): Promise<void> {
  await apiDelete(id);
  refreshInBackground();
}

export async function togglePlaylistMembership(
  playlistId: number,
  gameId: number,
  member: boolean,
): Promise<void> {
  await setPlaylistMembership(playlistId, gameId, member);
  // Counts changed; shelves and dropdown labels follow in the background.
  refreshInBackground();
}
