import { createSignal, createEffect, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { listContentPacks, setConfig, type ContentPackStatus } from "../api/tauri";
import { startContentPackInstall } from "../stores/contentPacks";
import { formatBytes } from "../util";

interface Props {
  open: boolean;
  onClose: () => void;
}

export function WelcomeModal(props: Props) {
  const [packs, setPacks] = createSignal<ContentPackStatus[]>([]);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal("");

  createEffect(() => {
    if (!props.open) { return; }
    setLoading(true);
    setError("");
    setSelected(null);
    listContentPacks("eXoDOS")
      .then((result) => {
        setPacks(result);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  });

  const handleContinue = async () => {
    // Record the user's choice first — even if the download fails to start,
    // the user made a decision and shouldn't be prompted again.
    await setConfig("welcome_seen", "1");
    const packId = selected();
    if (packId) {
      try {
        await startContentPackInstall("eXoDOS", packId);
      } catch (e) {
        console.error("Failed to start content pack install:", e);
      }
    }
    props.onClose();
  };

  // Only mark as seen if the user explicitly made a choice — don't write
  // welcome_seen if the modal couldn't load packs (network issue).
  const canProceed = () => !loading() && !error();

  return (
    <Dialog.Root open={props.open} onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}>
      <Portal>
        <Dialog.Backdrop class="ark-dialog-backdrop" />
        <Dialog.Positioner class="ark-dialog-positioner">
          <Dialog.Content class="ark-dialog-content">
            <Dialog.Title class="ark-dialog-title">Enhance your library</Dialog.Title>
            <Dialog.Description class="ark-dialog-desc">
              Download optional content to see box art for your games.
              You can manage these anytime in Settings.
            </Dialog.Description>

            <Show when={loading()}>
              <p class="setting-hint">Loading available content packs...</p>
            </Show>

            <Show when={error()}>
              <p class="setting-hint">
                Content packs unavailable right now. Check Settings later when you're online.
              </p>
            </Show>

            <Show when={canProceed()}>
              <div class="welcome-pack-options">
                <label class="welcome-pack-option">
                  <input
                    type="radio"
                    name="content-pack"
                    checked={selected() === null}
                    onChange={() => setSelected(null)}
                  />
                  <span class="welcome-pack-label">Skip for now</span>
                </label>
                <For each={packs()}>
                  {(pack) => {
                    const isFuture = () => !pack.available;
                    return (
                      <label class="welcome-pack-option" classList={{ disabled: isFuture() }}>
                        <input
                          type="radio"
                          name="content-pack"
                          disabled={isFuture()}
                          checked={selected() === pack.id}
                          onChange={() => setSelected(pack.id)}
                        />
                        <span class="welcome-pack-label">
                          {pack.display_name}
                          <span class="welcome-pack-size"> — ~{formatBytes(pack.size_bytes)}</span>
                        </span>
                        <Show when={isFuture()}>
                          <span class="welcome-pack-future">Coming soon</span>
                        </Show>
                      </label>
                    );
                  }}
                </For>
              </div>
            </Show>

            <div class="ark-dialog-actions">
              <Show when={error()}>
                <button class="btn-secondary" onClick={props.onClose}>Close</button>
              </Show>
              <Show when={canProceed()}>
                <button class="btn-primary" onClick={handleContinue}>
                  {selected() ? "Download & Continue" : "Continue"}
                </button>
              </Show>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
