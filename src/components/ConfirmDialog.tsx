import { createSignal, createEffect, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { Button } from "./Button";

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  /** Styles the confirm button red for destructive actions. */
  danger?: boolean;
  /** Overrides the "Cancel" label when declining is a choice, not an escape. */
  cancelLabel?: string;
  /** Adds an opt-out checkbox; its state is handed to both callbacks so the
   *  caller can remember a "don't ask again" for either answer. */
  rememberLabel?: string;
  onConfirm: (remember: boolean) => void;
  onClose: (remember: boolean) => void;
}

/** Must match the CSS animation duration on .closing. */
const EXIT_MS = 180;

/** Small confirm modal sharing the ark-dialog look and the delayed-unmount
 *  exit animation used by PlaylistNameDialog. */
export function ConfirmDialog(props: ConfirmDialogProps) {
  const [closing, setClosing] = createSignal(false);
  let closeTimer: number | undefined;
  onCleanup(() => { if (closeTimer) { clearTimeout(closeTimer); } });

  // Reopening within the exit window must cancel the pending timer - a
  // stale one would close (or worse, confirm) the fresh dialog.
  createEffect(() => {
    if (props.open) {
      if (closeTimer) { clearTimeout(closeTimer); closeTimer = undefined; }
      setClosing(false);
    }
  });

  const [remember, setRemember] = createSignal(false);
  createEffect(() => { if (props.open) { setRemember(false); } });

  const close = (confirmed: boolean) => {
    if (closing()) { return; }
    const keep = remember();
    setClosing(true);
    closeTimer = window.setTimeout(() => {
      closeTimer = undefined;
      setClosing(false);
      if (confirmed) { props.onConfirm(keep); }
      props.onClose(keep);
    }, EXIT_MS);
  };

  return (
    <Show when={props.open}>
      <Dialog.Root open onOpenChange={(e) => { if (!e.open) { close(false); } }}>
        <Portal>
          <Dialog.Backdrop class={`ark-dialog-backdrop${closing() ? " closing" : ""}`} />
          <Dialog.Positioner class="ark-dialog-positioner">
            <Dialog.Content class={`ark-dialog-content playlist-dialog${closing() ? " closing" : ""}`}>
              <Dialog.Title class="ark-dialog-title">{props.title}</Dialog.Title>
              <Dialog.Description class="ark-dialog-desc">{props.message}</Dialog.Description>
              <Show when={props.rememberLabel}>
                <label class="dialog-remember">
                  <input
                    type="checkbox"
                    checked={remember()}
                    onChange={(e) => setRemember(e.currentTarget.checked)}
                  />
                  <span>{props.rememberLabel}</span>
                </label>
              </Show>
              <div class="ark-dialog-actions">
                <Button variant="secondary" onClick={() => close(false)}>
                  {props.cancelLabel ?? "Cancel"}
                </Button>
                <button
                  class={props.danger ? "btn-danger" : "btn-primary"}
                  data-testid="confirm-ok"
                  onClick={() => close(true)}
                >
                  {props.confirmLabel}
                </button>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Show>
  );
}
