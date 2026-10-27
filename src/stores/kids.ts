import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import {
  kidsChangePin, kidsDisable, kidsLock, kidsStatus, kidsUnlock, type KidsStatus, type PinResult,
} from "../api/tauri";
import { stop as stopMusic } from "./music";

/** Kids mode's state as the backend holds it (§26); null until read. */
const [status, setStatus] = createSignal<KidsStatus | null>(null);
export { status as kidsStatus, setStatus as setKidsStatus };

export const kidsLocked = () => status()?.locked === true;
export const kidsEnabled = () => status()?.enabled === true;
/** The playlist the kids screen shows; null while kids mode is off. */
export const activeKidsPlaylist = () => (kidsEnabled() ? status()?.playlist_id ?? null : null);

let requestedPlaylist: number | null = null;

/** "Use for kids mode…" on a playlist: Settings → Kids mode picks it up once. */
export function requestKidsPlaylist(id: number) {
  requestedPlaylist = id;
}

export function takeRequestedKidsPlaylist(): number | null {
  const id = requestedPlaylist;
  requestedPlaylist = null;
  return id;
}

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

export interface PinCheck {
  ok: boolean;
  /** Seconds to wait when the PIN was wrong (0: try again now). */
  retryIn: number;
}

async function checked(result: Promise<PinResult>): Promise<PinCheck> {
  const r = await result;
  await loadKidsStatus();
  return { ok: r.ok, retryIn: r.retry_in };
}

export const unlockKids = (pin: string) => checked(kidsUnlock(pin));
export const changeKidsPin = (oldPin: string, newPin: string) => checked(kidsChangePin(oldPin, newPin));
export const disableKids = (pin: string) => checked(kidsDisable(pin));

/** `--reset-kids-mode` sent to the running app turns kids mode off there too. */
export async function initKidsReset() {
  await listen("kids-mode-reset", () => { void loadKidsStatus(); });
}
