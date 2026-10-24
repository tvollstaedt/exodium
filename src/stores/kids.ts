import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { kidsLock, kidsStatus, kidsUnlock, type KidsStatus } from "../api/tauri";
import { stop as stopMusic } from "./music";

/** Kids mode's state as the backend holds it (§26); null until read. */
const [status, setStatus] = createSignal<KidsStatus | null>(null);
export { status as kidsStatus, setStatus as setKidsStatus };

export const kidsLocked = () => status()?.locked === true;
export const kidsEnabled = () => status()?.enabled === true;

export async function loadKidsStatus() {
  try {
    setStatus(await kidsStatus());
  } catch (e) {
    console.warn("[kids] status unavailable:", e);
  }
}

/** A locked screen has no player to stop it with, so the music goes too. */
export async function lockKids() {
  stopMusic();
  setStatus(await kidsLock());
}

/** Answers how long to wait when the PIN was wrong (0: try again now). */
export async function unlockKids(pin: string): Promise<{ ok: boolean; retryIn: number }> {
  const r = await kidsUnlock(pin);
  await loadKidsStatus();
  return { ok: r.ok, retryIn: r.retry_in };
}

/** `--reset-kids-mode` sent to the running app turns kids mode off there too. */
export async function initKidsReset() {
  await listen("kids-mode-reset", () => { void loadKidsStatus(); });
}
