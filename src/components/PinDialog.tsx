import { createSignal, Index, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { kidsStatus, type PinCheck } from "../stores/kids";
import { Button } from "./Button";

export interface PinField {
  testId: string;
  placeholder: string;
}

/** A refused submit says why and how long the backend makes it wait. */
export type PinAnswer = { ok: true } | { ok: false; error: string; retryIn: number };

export function pinAnswer(check: PinCheck): PinAnswer {
  return check.ok ? { ok: true } : { ok: false, error: "Wrong PIN", retryIn: check.retryIn };
}

interface PinDialogProps {
  open: boolean;
  title: string;
  description?: string;
  fields: PinField[];
  /** Set when the last field repeats the one before it: the message for a mismatch. */
  mismatch?: string;
  submitLabel: string;
  /** A single-field check submits itself once it holds the PIN's digit count. */
  autoSubmit?: boolean;
  testId: string;
  submitTestId: string;
  onSubmit: (pins: string[]) => Promise<PinAnswer>;
  onClose: () => void;
}

/** Every kids-mode PIN prompt (§26): unlock, turn on, change, turn off. A
 *  pending wait after repeated misses shows as a countdown. */
export function PinDialog(props: PinDialogProps) {
  return (
    <Show when={props.open}>
      <PinDialogBody {...props} />
    </Show>
  );
}

function PinDialogBody(props: PinDialogProps) {
  const empty = () => props.fields.map(() => "");
  const [pins, setPins] = createSignal(empty());
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [waitUntil, setWaitUntil] = createSignal(0);
  const [now, setNow] = createSignal(Date.now());
  const tick = window.setInterval(() => setNow(Date.now()), 500);
  onCleanup(() => clearInterval(tick));
  const inputs: HTMLInputElement[] = [];

  const waitSecs = () => Math.max(0, Math.ceil((waitUntil() - now()) / 1000));
  // `now` ticks every 500 ms; a deadline set against a fresher clock would
  // read as a second of wait that is not there.
  const waitFor = (secs: number) => {
    const t = Date.now();
    setNow(t);
    setWaitUntil(secs > 0 ? t + secs * 1000 : 0);
  };
  waitFor(kidsStatus()?.retry_in ?? 0);
  onMount(() => requestAnimationFrame(() => inputs[0]?.focus()));

  const complete = () => pins().every((p) => p.length >= 4);
  const setPin = (i: number, value: string) => setPins((p) => p.map((v, j) => (j === i ? value : v)));

  const submit = async () => {
    if (busy() || waitSecs() > 0 || !complete()) { return; }
    const values = pins();
    const last = values.length - 1;
    if (props.mismatch && values[last] !== values[last - 1]) {
      setPin(last - 1, "");
      setPin(last, "");
      setError(props.mismatch);
      inputs[last - 1]?.focus();
      return;
    }
    setBusy(true);
    try {
      const answer = await props.onSubmit(values);
      if (answer.ok) { return; }
      setPins(empty());
      setError(answer.error);
      waitFor(answer.retryIn);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
    inputs[0]?.focus();
  };

  const onEnter = (i: number) => {
    if (i < props.fields.length - 1) {
      inputs[i + 1]?.focus();
    } else {
      void submit();
    }
  };

  return (
    <Dialog.Root open onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}>
      <Portal>
        <Dialog.Backdrop class="ark-dialog-backdrop" />
        <Dialog.Positioner class="ark-dialog-positioner">
          <Dialog.Content class="ark-dialog-content playlist-dialog" data-testid={props.testId}>
            <Dialog.Title class="ark-dialog-title">{props.title}</Dialog.Title>
            <Show when={props.description}>
              <Dialog.Description class="ark-dialog-desc">{props.description}</Dialog.Description>
            </Show>
            <Index each={props.fields}>
              {(field, i) => (
                <input
                  ref={(el) => { inputs[i] = el; }}
                  class="playlist-name-input kids-pin-input"
                  data-testid={field().testId}
                  type="password"
                  inputMode="numeric"
                  autocomplete="off"
                  maxLength={8}
                  placeholder={field().placeholder}
                  aria-label={field().placeholder}
                  value={pins()[i]}
                  disabled={waitSecs() > 0}
                  onInput={(e) => {
                    const pin = e.currentTarget.value.replace(/\D/g, "");
                    setPin(i, pin);
                    if (props.autoSubmit && pin.length === kidsStatus()?.pin_length) { void submit(); }
                  }}
                  onKeyDown={(e) => { if (e.key === "Enter") { onEnter(i); } }}
                />
              )}
            </Index>
            <Show when={waitSecs() > 0} fallback={<Show when={error()}><div class="playlist-name-error" data-testid="kids-pin-error">{error()}</div></Show>}>
              <div class="playlist-name-error" data-testid="kids-pin-error">Too many wrong PINs - try again in {waitSecs()} s</div>
            </Show>
            <div class="ark-dialog-actions">
              <Button variant="secondary" onClick={props.onClose}>Cancel</Button>
              <Button
                variant="primary"
                data-testid={props.submitTestId}
                disabled={!complete() || waitSecs() > 0}
                loading={busy()}
                onClick={() => void submit()}
              >
                {props.submitLabel}
              </Button>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
  );
}
