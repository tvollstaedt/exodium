import { createSignal, createEffect, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import {
  playlistDialog, setPlaylistDialog, createPlaylist, renamePlaylist,
  togglePlaylistMembership,
} from "../stores/playlists";
import { showToast } from "../stores/toasts";
import { Button } from "./Button";

/** How long the exit animation runs before the dialog unmounts.
 *  Must match the CSS animation duration on .closing. */
const EXIT_MS = 180;

/** App-wide create/rename playlist dialog, driven by the playlistDialog
 *  store signal. Mounted once in Library. */
export function PlaylistNameDialog() {
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  // Kept mounted with a .closing class while the exit animation plays -
  // unmounting immediately would snap the modal away.
  const [closing, setClosing] = createSignal(false);
  let inputRef: HTMLInputElement | undefined;
  let closeTimer: number | undefined;
  onCleanup(() => { if (closeTimer) { clearTimeout(closeTimer); } });

  const request = () => playlistDialog();

  createEffect(() => {
    const req = request();
    if (!req) { return; }
    // A reopen within the exit window must cancel the pending unmount, or
    // the stale timer closes the fresh dialog under the user's cursor.
    if (closeTimer) { clearTimeout(closeTimer); closeTimer = undefined; }
    setClosing(false);
    setName(req.mode === "rename" ? req.playlist.name : "");
    setError("");
    // Focus after the dialog content mounts.
    requestAnimationFrame(() => inputRef?.select());
  });

  const close = () => {
    if (closing()) { return; }
    setClosing(true);
    closeTimer = window.setTimeout(() => {
      closeTimer = undefined;
      setClosing(false);
      setPlaylistDialog(null);
    }, EXIT_MS);
  };

  const handleSave = async () => {
    const req = request();
    const trimmed = name().trim();
    if (!req || !trimmed || saving()) { return; }
    setSaving(true);
    setError("");
    try {
      if (req.mode === "create") {
        const id = await createPlaylist(trimmed);
        if (req.gameId != null) {
          await togglePlaylistMembership(id, req.gameId, true);
          showToast(`Created "${trimmed}" and added the game`, "success");
        } else {
          showToast(`Playlist "${trimmed}" created`, "success");
        }
      } else {
        await renamePlaylist(req.playlist.id, trimmed);
        showToast(`Renamed to "${trimmed}"`, "success");
      }
      close();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Show when={request()}>
      <Dialog.Root open onOpenChange={(e) => { if (!e.open) { close(); } }}>
        <Portal>
          <Dialog.Backdrop class={`ark-dialog-backdrop${closing() ? " closing" : ""}`} />
          <Dialog.Positioner class="ark-dialog-positioner">
            <Dialog.Content class={`ark-dialog-content playlist-dialog${closing() ? " closing" : ""}`}>
              <Dialog.Title class="ark-dialog-title">
                {request()!.mode === "create" ? "New playlist" : "Rename playlist"}
              </Dialog.Title>
              <input
                ref={inputRef}
                class="playlist-name-input"
                data-testid="playlist-name-input"
                type="text"
                value={name()}
                placeholder="Playlist name"
                maxLength={80}
                onInput={(e) => setName(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") { handleSave(); }
                }}
              />
              <Show when={error()}>
                <div class="playlist-name-error">{error()}</div>
              </Show>
              <div class="ark-dialog-actions">
                <Button variant="secondary" onClick={close}>Cancel</Button>
                <Button variant="primary"
                  data-testid="playlist-name-save"
                  onClick={handleSave}
                  disabled={!name().trim() || saving()}
                >
                  {saving() ? "Saving…" : request()!.mode === "create" ? "Create" : "Rename"}
                </Button>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Show>
  );
}
