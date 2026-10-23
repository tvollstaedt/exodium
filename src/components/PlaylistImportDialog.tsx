import { createEffect, createSignal, For, on, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import type { ImportCandidate, ImportPreview } from "../api/tauri";
import { importPlaylist } from "../stores/playlists";
import { showToast } from "../stores/toasts";
import { platformTag } from "../util";
import { Button } from "./Button";

/** Must match the CSS animation duration on .closing. */
const EXIT_MS = 180;

const candidateLabel = (c: ImportCandidate) =>
  `${platformTag(c.collection) ?? c.collection}${c.year ? ` · ${c.year}` : ""}`;

/** The import's preview (#43): what was found, what needs a pick, what was
 *  not found - then one new playlist named after the file. */
export function PlaylistImportDialog(props: { preview: ImportPreview | null; onClose: () => void }) {
  const [name, setName] = createSignal("");
  /** Per row index: the game it adds, or null to leave it out. */
  const [picks, setPicks] = createSignal<Record<number, number | null>>({});
  const [error, setError] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [closing, setClosing] = createSignal(false);
  let closeTimer: number | undefined;
  onCleanup(() => { if (closeTimer) { clearTimeout(closeTimer); } });

  createEffect(on(() => props.preview, (preview) => {
    if (!preview) { return; }
    if (closeTimer) { clearTimeout(closeTimer); closeTimer = undefined; }
    setClosing(false);
    setName(preview.name);
    setError("");
    setPicks(Object.fromEntries(preview.rows.map((r, i) => [i, r.candidates[0]?.game_id ?? null])));
  }));

  const rows = () => (props.preview?.rows ?? []).map((row, index) => ({ row, index }));
  const matched = () => rows().filter(({ row }) => row.candidates.length === 1);
  const ambiguous = () => rows().filter(({ row }) => row.candidates.length > 1);
  const missing = () => rows().filter(({ row }) => row.candidates.length === 0);
  const chosen = () => [...new Set(Object.values(picks()).filter((id): id is number => id != null))];

  const summary = () => [
    matched().length > 0 && `${matched().length} found`,
    ambiguous().length > 0 && `${ambiguous().length} to choose`,
    missing().length > 0 && `${missing().length} not found`,
  ].filter(Boolean).join(" · ");

  const close = () => {
    if (closing()) { return; }
    setClosing(true);
    closeTimer = window.setTimeout(() => {
      closeTimer = undefined;
      setClosing(false);
      props.onClose();
    }, EXIT_MS);
  };

  const create = async () => {
    const trimmed = name().trim();
    const ids = chosen();
    if (!trimmed || ids.length === 0 || saving()) { return; }
    setSaving(true);
    setError("");
    try {
      await importPlaylist(trimmed, ids);
      showToast(`Created "${trimmed}" with ${ids.length} ${ids.length === 1 ? "game" : "games"}`, "success");
      close();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Show when={props.preview}>
      <Dialog.Root open onOpenChange={(e) => { if (!e.open) { close(); } }}>
        <Portal>
          <Dialog.Backdrop class={`ark-dialog-backdrop${closing() ? " closing" : ""}`} />
          <Dialog.Positioner class="ark-dialog-positioner">
            <Dialog.Content
              class={`ark-dialog-content playlist-dialog playlist-io-dialog${closing() ? " closing" : ""}`}
              data-testid="playlist-import-dialog"
            >
              <Dialog.Title class="ark-dialog-title">Import list</Dialog.Title>
              <p class="playlist-io-sub">{summary() || "The list holds no titles."}</p>

              <div class="playlist-io-body">
                <Show when={ambiguous().length > 0}>
                  <h4 class="playlist-io-heading">In several collections - pick one</h4>
                  <For each={ambiguous()}>
                    {({ row, index }) => (
                      <div class="playlist-io-row" data-testid="import-ambiguous">
                        <span class="playlist-io-title">{row.title}</span>
                        <div class="variant-switcher" role="group" aria-label={`Versions of ${row.title}`}>
                          <For each={row.candidates}>
                            {(c) => (
                              <button
                                class={`variant-chip${picks()[index] === c.game_id ? " is-selected" : ""}`}
                                title={c.title}
                                onClick={() => setPicks((p) => ({ ...p, [index]: c.game_id }))}
                              >
                                {candidateLabel(c)}
                              </button>
                            )}
                          </For>
                          <button
                            class={`variant-chip${picks()[index] == null ? " is-selected" : ""}`}
                            onClick={() => setPicks((p) => ({ ...p, [index]: null }))}
                          >
                            Skip
                          </button>
                        </div>
                      </div>
                    )}
                  </For>
                </Show>

                <Show when={missing().length > 0}>
                  <details class="playlist-io-details" open={matched().length + ambiguous().length === 0}>
                    <summary>Not found ({missing().length})</summary>
                    <ul data-testid="import-missing">
                      <For each={missing()}>{({ row }) => <li>Line {row.line}: {row.title}</li>}</For>
                    </ul>
                  </details>
                </Show>

                <Show when={matched().length > 0}>
                  <details class="playlist-io-details">
                    <summary>Found ({matched().length})</summary>
                    <ul>
                      <For each={matched()}>
                        {({ row }) => <li>{row.candidates[0].title} <span class="playlist-io-muted">({candidateLabel(row.candidates[0])})</span></li>}
                      </For>
                    </ul>
                  </details>
                </Show>
              </div>

              <label class="playlist-io-label" for="import-name">Playlist name</label>
              <input
                id="import-name"
                class="playlist-name-input"
                type="text"
                value={name()}
                maxLength={80}
                onInput={(e) => setName(e.currentTarget.value)}
                onKeyDown={(e) => { if (e.key === "Enter") { void create(); } }}
              />
              <Show when={error()}>
                <div class="playlist-name-error">{error()}</div>
              </Show>
              <div class="ark-dialog-actions">
                <Button variant="secondary" onClick={close}>Cancel</Button>
                <Button
                  variant="primary"
                  data-testid="import-create"
                  disabled={!name().trim() || chosen().length === 0}
                  loading={saving()}
                  loadingLabel="Creating…"
                  onClick={() => void create()}
                >
                  {`Create playlist (${chosen().length})`}
                </Button>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Show>
  );
}
