import { createSignal } from "solid-js";
import { getConfig, setSeedingEnabled } from "../api/tauri";
import { isOffline } from "./network";

// Shared because two places need it: the settings switch owns the change, and
// the network badge has to say "sharing is off" rather than leave a zero rate
// looking like a fault.
const [seedingOn, setSeedingOn] = createSignal(false);
export { seedingOn };

/** Read the stored preference. Opt-in: only an explicit "1" counts, mirroring
 *  the Rust side. */
export async function loadSeeding(): Promise<void> {
  try {
    setSeedingOn((await getConfig("seeding_enabled")) === "1");
  } catch (e) {
    console.warn("[settings] failed to load the seeding preference:", e);
  }
}

/** Persist and apply, rolling the signal back if the backend refuses. */
export async function applySeeding(enabled: boolean): Promise<void> {
  const previous = seedingOn();
  setSeedingOn(enabled);
  try {
    await setSeedingEnabled(enabled);
  } catch (e) {
    setSeedingOn(previous);
    throw e;
  }
}

/** Does this install still owe the seeding answer (§11)? Unset key and
 *  online; the backend reads unset as off meanwhile. */
export async function needsSeedingConsent(): Promise<boolean> {
  if (isOffline()) { return false; }
  try {
    return (await getConfig("seeding_enabled")) == null;
  } catch (e) {
    // Asking on a failed read would mean asking on every start; the safe state
    // (not seeding) already holds, so stay quiet.
    console.warn("[settings] could not read the seeding preference:", e);
    return false;
  }
}
