import { createSignal, For, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { setFavorites, type Playlist } from "../api/tauri";
import {
  announceFavorites, clearSelection, exitSelection, selectedRefs, selectionCount, selectMode,
} from "../stores/selection";
import { setMembershipMany, setPlaylistDialog, userPlaylists } from "../stores/playlists";
import { enqueueDownloads } from "../stores/batchDownloads";
import { isOffline } from "../stores/network";
import { showToast } from "../stores/toasts";
import { Button } from "./Button";

/** Downloads a selection starts belong to no playlist. */
const SELECTION_QUEUE = -1;

const games = (n: number) => `${n.toLocaleString()} ${n === 1 ? "game" : "games"}`;

interface Props {
  /** The user playlist the current list shows, if any: "Remove" acts on it. */
  playlist: Playlist | null;
  onSelectAll: () => void;
  /** After a change the visible lists must re-read (favorites, membership). */
  onChanged: () => void;
}

/** The bar a selection acts through (§25): add to or remove from a playlist,
 *  download, favorite. Bulk on purpose - nothing here removes a game. */
export function SelectionBar(props: Props) {
  const [menu, setMenu] = createSignal<{ x: number; y: number } | null>(null);
  const ids = () => selectedRefs().map((r) => r.id);
  const toFetch = () => selectedRefs().filter((r) => !r.installed && !r.in_library);
  const allFavorites = () => selectedRefs().length > 0 && selectedRefs().every((r) => r.favorited);

  const addTo = async (playlist: Playlist) => {
    setMenu(null);
    const n = ids().length;
    try {
      await setMembershipMany(playlist.id, ids(), true);
      showToast(`Added ${games(n)} to "${playlist.name}"`, "success");
      props.onChanged();
    } catch (e) {
      showToast(`Couldn't update "${playlist.name}"`, "error", { detail: String(e) });
    }
  };

  const remove = async () => {
    const playlist = props.playlist;
    if (!playlist) { return; }
    const n = ids().length;
    try {
      await setMembershipMany(playlist.id, ids(), false);
      showToast(`Removed ${games(n)} from "${playlist.name}"`, "success");
      clearSelection();
      props.onChanged();
    } catch (e) {
      showToast(`Couldn't update "${playlist.name}"`, "error", { detail: String(e) });
    }
  };

  const download = () => {
    const items = toFetch();
    enqueueDownloads(SELECTION_QUEUE, items.map((r) => ({
      gameId: r.id,
      title: r.title,
      cover: r.torrent_source ? { source: r.torrent_source, key: r.thumbnail_key } : undefined,
    })));
    showToast(`Downloading ${games(items.length)}`, "success");
  };

  const favorite = async () => {
    const value = !allFavorites();
    try {
      await setFavorites(ids(), value);
      announceFavorites(ids(), value);
      props.onChanged();
    } catch (e) {
      showToast("Couldn't update favorites", "error", { detail: String(e) });
    }
  };

  return (
    <Show when={selectMode() || selectionCount() > 0}>
      <div class="selection-bar" data-testid="selection-bar">
        <span class="selection-bar-count" data-testid="selection-count">
          {selectionCount() > 0 ? `${games(selectionCount())} selected` : "Click games to select them"}
        </span>
        <Show when={selectMode()}>
          <Button variant="small" data-testid="selection-all" onClick={props.onSelectAll}>Select all</Button>
        </Show>
        <Show when={selectionCount() > 0}>
          <Button
            variant="small"
            data-testid="selection-add"
            onClick={(e: MouseEvent) => {
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
              setMenu({ x: r.left, y: r.top });
            }}
          >Add to playlist ▾</Button>
          <Show when={props.playlist}>
            <Button variant="small" data-testid="selection-remove" onClick={() => void remove()}>
              Remove from "{props.playlist!.name}"
            </Button>
          </Show>
          <span title={isOffline() ? "Offline mode - nothing is downloaded." : toFetch().length === 0 ? "Every selected game is installed or downloading" : undefined}>
            <Button variant="small" disabled={isOffline() || toFetch().length === 0} onClick={download}>
              Download
            </Button>
          </span>
          <Button variant="small" onClick={() => void favorite()}>
            {allFavorites() ? "Unfavorite" : "Favorite"}
          </Button>
        </Show>
        <button class="selection-bar-close" title="Clear selection (Esc)" onClick={exitSelection}>✕</button>
      </div>

      <Show when={menu()}>
        <Portal>
          <div class="context-backdrop" onMouseDown={() => setMenu(null)} />
          <div class="context-menu selection-menu" style={{ left: `${menu()!.x}px`, bottom: `${window.innerHeight - menu()!.y + 6}px` }}>
            <For each={userPlaylists()}>
              {(p) => (
                <button class="context-menu-item" data-testid="selection-add-to" onMouseDown={(e) => e.stopPropagation()} onClick={() => void addTo(p)}>
                  {p.name}
                </button>
              )}
            </For>
            <Show when={userPlaylists().length > 0}>
              <div class="playlist-menu-divider" />
            </Show>
            <button
              class="context-menu-item"
              data-testid="selection-new-playlist"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => {
                setMenu(null);
                setPlaylistDialog({ mode: "create", gameIds: ids() });
              }}
            >＋ New playlist…</button>
          </div>
        </Portal>
      </Show>
    </Show>
  );
}
