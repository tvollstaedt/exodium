import { createSignal, createEffect, onCleanup } from "solid-js";

/** Flat time before a progress bar sweeps. Long, because progress lands one
 *  whole piece at a time; equals STALL_HINT_SECS in stores/downloads.ts so
 *  the sweep and the "waiting for peers" text agree. */
export const STALL_MS = 15000;

/** Signal that flips true when `value` hasn't changed for `stallMs`, and back
 *  to false on the next change. Never fires once the value reaches 1 - a
 *  finished transfer waiting on extraction is not stalled. */
export function createStallDetector(value: () => number, stallMs = STALL_MS) {
  const [stalled, setStalled] = createSignal(false);
  let lastValue = value();
  let lastChangeAt = Date.now();

  createEffect(() => {
    const v = value();
    if (v !== lastValue) {
      lastValue = v;
      lastChangeAt = Date.now();
      setStalled(false);
    }
  });

  const id = setInterval(() => {
    if (value() < 1 && Date.now() - lastChangeAt > stallMs) {
      setStalled(true);
    }
  }, 500);
  onCleanup(() => clearInterval(id));

  return stalled;
}
