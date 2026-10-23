import { createEffect, createSignal, For, on, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { playlistDownloadPlan, type DownloadPlan, type Playlist } from "../api/tauri";
import { enqueueDownloads } from "../stores/batchDownloads";
import { isOffline } from "../stores/network";
import { showToast } from "../stores/toasts";
import { formatBytes, languageName } from "../util";
import { Button } from "./Button";

/** Must match the CSS animation duration on .closing. */
const EXIT_MS = 180;

interface Props {
  playlist: Playlist | null;
  /** Preselected; a game without that version downloads in English. */
  language?: string;
  onClose: () => void;
}

/** "Download all" (#43): what the playlist still lacks, what it costs and in
 *  which language - then the batch queue takes over. */
export function DownloadAllDialog(props: Props) {
  const [language, setLanguage] = createSignal("EN");
  const [plan, setPlan] = createSignal<DownloadPlan | null>(null);
  /** Kept across plan reloads, so the picker does not vanish while one loads. */
  const [languages, setLanguages] = createSignal<string[]>([]);
  const [error, setError] = createSignal("");
  const [closing, setClosing] = createSignal(false);
  let closeTimer: number | undefined;
  onCleanup(() => { if (closeTimer) { clearTimeout(closeTimer); } });

  createEffect(on(() => props.playlist?.id, (id) => {
    if (id == null) { return; }
    if (closeTimer) { clearTimeout(closeTimer); closeTimer = undefined; }
    setClosing(false);
    setLanguages([]);
    setLanguage(props.language ?? "EN");
  }));

  let epoch = 0;
  createEffect(() => {
    const id = props.playlist?.id;
    const lang = language();
    if (id == null) { return; }
    const mine = ++epoch;
    setPlan(null);
    setError("");
    playlistDownloadPlan(id, lang)
      .then((p) => {
        if (mine !== epoch) { return; }
        setPlan(p);
        setLanguages(p.languages);
      })
      .catch((e) => { if (mine === epoch) { setError(String(e)); } });
  });

  const tooLittleSpace = () => {
    const p = plan();
    return p?.free_bytes != null && p.total_bytes > p.free_bytes;
  };
  const skipped = () => {
    const p = plan();
    if (!p) { return ""; }
    return [
      p.present > 0 && `${p.present} already installed`,
      p.in_progress > 0 && `${p.in_progress} already downloading`,
    ].filter(Boolean).join(", ");
  };

  const close = () => {
    if (closing()) { return; }
    setClosing(true);
    closeTimer = window.setTimeout(() => {
      closeTimer = undefined;
      setClosing(false);
      props.onClose();
    }, EXIT_MS);
  };

  const start = () => {
    const p = plan();
    const playlist = props.playlist;
    if (!p || !playlist || p.items.length === 0) { return; }
    enqueueDownloads(playlist.id, p.items.map((i) => ({
      gameId: i.game_id,
      title: i.title,
      cover: i.torrent_source ? { source: i.torrent_source, key: i.thumbnail_key } : undefined,
    })));
    const n = p.items.length;
    showToast(`Downloading ${n} ${n === 1 ? "game" : "games"} from "${playlist.name}"`, "success");
    close();
  };

  return (
    <Show when={props.playlist}>
      <Dialog.Root open onOpenChange={(e) => { if (!e.open) { close(); } }}>
        <Portal>
          <Dialog.Backdrop class={`ark-dialog-backdrop${closing() ? " closing" : ""}`} />
          <Dialog.Positioner class="ark-dialog-positioner">
            <Dialog.Content
              class={`ark-dialog-content playlist-dialog playlist-io-dialog${closing() ? " closing" : ""}`}
              data-testid="download-all-dialog"
            >
              <Dialog.Title class="ark-dialog-title">Download all</Dialog.Title>
              <p class="playlist-io-sub">{props.playlist!.name}</p>

              <Show when={languages().length > 1}>
                <label class="playlist-io-label" for="download-all-language">Language</label>
                <select
                  id="download-all-language"
                  class="game-settings-select"
                  value={language()}
                  onChange={(e) => setLanguage(e.currentTarget.value)}
                >
                  <For each={languages()}>{(l) => <option value={l}>{languageName(l)}</option>}</For>
                </select>
                <Show when={language() !== "EN"}>
                  <p class="setting-hint">Games without a {languageName(language())} version download in English.</p>
                </Show>
              </Show>

              <div class="playlist-io-facts" data-testid="download-all-facts">
                <Show when={plan()} fallback={<div>{error() || "Checking the playlist…"}</div>}>
                  {(p) => (
                    <>
                      <div>
                        {p().items.length === 0
                          ? "Nothing left to download."
                          : `${p().items.length} ${p().items.length === 1 ? "game" : "games"} to download · ${formatBytes(p().total_bytes)}`}
                      </div>
                      <Show when={skipped()}>
                        <div class="playlist-io-muted">Skipped: {skipped()}</div>
                      </Show>
                      <Show when={p().free_bytes != null}>
                        <div class={tooLittleSpace() ? "playlist-io-warn" : "playlist-io-muted"}>
                          {tooLittleSpace()
                            ? `Only ${formatBytes(p().free_bytes!)} free - not enough for all of them.`
                            : `${formatBytes(p().free_bytes!)} free`}
                        </div>
                      </Show>
                    </>
                  )}
                </Show>
              </div>

              <Show when={isOffline()}>
                <p class="playlist-io-warn">Offline mode - nothing is downloaded. Enable downloads in Settings → Network.</p>
              </Show>

              <div class="ark-dialog-actions">
                <Button variant="secondary" data-testid="download-all-cancel" onClick={close}>Cancel</Button>
                <Button
                  variant="primary"
                  data-testid="download-all-start"
                  disabled={!plan() || plan()!.items.length === 0 || isOffline()}
                  onClick={start}
                >
                  Download
                </Button>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Show>
  );
}
