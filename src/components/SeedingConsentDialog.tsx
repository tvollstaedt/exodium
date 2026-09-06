import { createSignal, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { Button } from "./Button";

interface Props {
  open: boolean;
  /** Called with the user's answer; the caller persists and applies it. */
  onDecide: (enabled: boolean) => Promise<void>;
}

/** The setup's seeding question, asked late of installs that predate it
 *  (§11). Two buttons, not dismissible. */
export function SeedingConsentDialog(props: Props) {
  const [busy, setBusy] = createSignal<"on" | "off" | null>(null);
  const [error, setError] = createSignal("");

  const decide = async (enabled: boolean) => {
    setError("");
    setBusy(enabled ? "on" : "off");
    try {
      await props.onDecide(enabled);
    } catch (e) {
      setError(`Could not save that: ${e}`);
      setBusy(null);
    }
  };

  return (
    <Dialog.Root open={props.open} closeOnEscape={false} closeOnInteractOutside={false}>
      <Portal>
        <Dialog.Backdrop class="ark-dialog-backdrop" />
        <Dialog.Positioner class="ark-dialog-positioner">
          <Dialog.Content class="ark-dialog-content">
            <Dialog.Title class="ark-dialog-title">Sharing is now your choice</Dialog.Title>
            <Dialog.Description class="ark-dialog-desc">
              Until now, Exodium shared parts of the games you have with other
              players while it was running, without asking. From this version
              on it is up to you.
            </Dialog.Description>

            <p class="setting-hint">
              Sharing keeps the collection alive for everyone - but it also
              means you are distributing the game files, which is a legal risk
              in some countries. Exodium is not uploading anything until you
              answer, and you can change your mind any time in Settings →
              Network.
            </p>

            <Show when={error()}>
              <div class="error" style="margin-top:12px">{error()}</div>
            </Show>

            <div class="ark-dialog-actions">
              <Button
                variant="secondary"
                onClick={() => decide(false)}
                loading={busy() === "off"}
                disabled={busy() !== null}
              >
                Don't share
              </Button>
              <Button
                variant="primary"
                onClick={() => decide(true)}
                loading={busy() === "on"}
                disabled={busy() !== null}
              >
                Keep sharing
              </Button>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
