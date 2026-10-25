import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { kidsStatus, unlockKids } from "../stores/kids";
import { Button } from "./Button";

/** The kids screen's one door: the PIN, with the backend's wait after
 *  repeated misses shown as a countdown (§26). */
export function KidsPinDialog(props: { open: boolean; onClose: () => void; onUnlocked: () => void }) {
  const [pin, setPin] = createSignal("");
  const [error, setError] = createSignal("");
  const [waitUntil, setWaitUntil] = createSignal(0);
  const [now, setNow] = createSignal(Date.now());
  const tick = window.setInterval(() => setNow(Date.now()), 500);
  onCleanup(() => clearInterval(tick));
  let inputRef: HTMLInputElement | undefined;

  const waitSecs = () => Math.max(0, Math.ceil((waitUntil() - now()) / 1000));
  // `now` ticks every 500 ms; a deadline set against a fresher clock would
  // read as a second of wait that is not there.
  const waitFor = (secs: number) => {
    const t = Date.now();
    setNow(t);
    setWaitUntil(secs > 0 ? t + secs * 1000 : 0);
  };

  createEffect(on(() => props.open, (open) => {
    if (!open) { return; }
    setPin("");
    setError("");
    waitFor(kidsStatus()?.retry_in ?? 0);
    requestAnimationFrame(() => inputRef?.focus());
  }));

  const submit = async () => {
    if (waitSecs() > 0 || pin().length < 4) { return; }
    const r = await unlockKids(pin());
    if (r.ok) {
      props.onUnlocked();
      return;
    }
    setPin("");
    setError("Wrong PIN");
    waitFor(r.retryIn);
    inputRef?.focus();
  };

  return (
    <Show when={props.open}>
      <Dialog.Root open onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}>
        <Portal>
          <Dialog.Backdrop class="ark-dialog-backdrop" />
          <Dialog.Positioner class="ark-dialog-positioner">
            <Dialog.Content class="ark-dialog-content playlist-dialog" data-testid="kids-pin-dialog">
              <Dialog.Title class="ark-dialog-title">Unlock Exodium</Dialog.Title>
              <input
                ref={inputRef}
                class="playlist-name-input kids-pin-input"
                data-testid="kids-pin-input"
                type="password"
                inputMode="numeric"
                autocomplete="off"
                maxLength={8}
                placeholder="PIN"
                value={pin()}
                disabled={waitSecs() > 0}
                onInput={(e) => setPin(e.currentTarget.value.replace(/\D/g, ""))}
                onKeyDown={(e) => { if (e.key === "Enter") { void submit(); } }}
              />
              <Show when={waitSecs() > 0} fallback={<Show when={error()}><div class="playlist-name-error" data-testid="kids-pin-error">{error()}</div></Show>}>
                <div class="playlist-name-error" data-testid="kids-pin-error">Too many wrong PINs - try again in {waitSecs()} s</div>
              </Show>
              <div class="ark-dialog-actions">
                <Button variant="secondary" onClick={props.onClose}>Cancel</Button>
                <Button variant="primary" data-testid="kids-pin-submit" disabled={pin().length < 4 || waitSecs() > 0} onClick={() => void submit()}>
                  Unlock
                </Button>
              </div>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Show>
  );
}
