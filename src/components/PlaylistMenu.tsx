import { createSignal, onMount, For, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { getGamePlaylists } from "../api/tauri";
import {
  userPlaylists, togglePlaylistMembership, setPlaylistDialog,
} from "../stores/playlists";
import { showToast } from "../stores/toasts";

interface PlaylistMenuProps {
  x: number;
  y: number;
  gameId: number;
  onClose: () => void;
}

/** Floating "Add to playlist" picker: user playlists with membership
 *  checkmarks + a "New playlist…" entry. Opened from the game card's
 *  context menu and the detail panel. Curated playlists are read-only and
 *  intentionally absent. */
export function PlaylistMenu(props: PlaylistMenuProps) {
  const [memberOf, setMemberOf] = createSignal<Set<number>>(new Set());
  // Once the user toggles anything, the (possibly still in-flight) initial
  // membership fetch must not overwrite the optimistic state - a slow
  // response would revert checkmarks for actions that already succeeded.
  let touched = false;
  // Anchor point clamped to the viewport - the detail panel's button sits
  // near the right window edge, where an unclamped menu gets clipped.
  const [pos, setPos] = createSignal({ x: props.x, y: props.y });
  let menuRef: HTMLDivElement | undefined;

  onMount(() => {
    // onMount runs after the DOM insert but before paint; measuring here
    // repositions without a visible overflow frame.
    if (menuRef) {
      const rect = menuRef.getBoundingClientRect();
      setPos({
        x: Math.max(8, Math.min(props.x, window.innerWidth - rect.width - 8)),
        y: Math.max(8, Math.min(props.y, window.innerHeight - rect.height - 8)),
      });
    }
    getGamePlaylists(props.gameId)
      .then((ids) => { if (!touched) { setMemberOf(new Set(ids)); } })
      .catch(() => {});
  });

  const toggle = async (playlistId: number) => {
    touched = true;
    const isMember = memberOf().has(playlistId);
    const playlistName = userPlaylists().find((p) => p.id === playlistId)?.name ?? "playlist";
    // Optimistic: flip locally, revert on failure.
    setMemberOf((prev) => {
      const next = new Set(prev);
      if (isMember) { next.delete(playlistId); } else { next.add(playlistId); }
      return next;
    });
    try {
      await togglePlaylistMembership(playlistId, props.gameId, !isMember);
      showToast(
        isMember ? `Removed from "${playlistName}"` : `Added to "${playlistName}"`,
        "success",
      );
    } catch (e) {
      showToast(`Couldn't update "${playlistName}"`, "error", { detail: String(e) });
      setMemberOf((prev) => {
        const next = new Set(prev);
        if (isMember) { next.add(playlistId); } else { next.delete(playlistId); }
        return next;
      });
    }
  };

  return (
    <Portal>
      <div
        class="context-backdrop playlist-menu-backdrop"
        onMouseDown={props.onClose}
        onContextMenu={(e) => { e.preventDefault(); props.onClose(); }}
      />
      <div ref={menuRef} class="context-menu playlist-menu" style={{ left: `${pos().x}px`, top: `${pos().y}px` }}>
        <Show when={userPlaylists().length > 0}>
          <For each={userPlaylists()}>
            {(p) => (
              <button
                class="context-menu-item playlist-menu-item"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => toggle(p.id)}
              >
                <span class={`playlist-menu-check ${memberOf().has(p.id) ? "checked" : ""}`}>✓</span>
                <span class="playlist-menu-name">{p.name}</span>
              </button>
            )}
          </For>
          <div class="playlist-menu-divider" />
        </Show>
        <button
          class="context-menu-item"
          data-testid="playlist-menu-new"
          onMouseDown={(e) => e.stopPropagation()}
          onClick={() => {
            props.onClose();
            setPlaylistDialog({ mode: "create", gameId: props.gameId });
          }}
        >
          ＋ New playlist…
        </button>
      </div>
    </Portal>
  );
}
