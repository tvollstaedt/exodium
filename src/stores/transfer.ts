import { createSignal } from "solid-js";
import { getTransferStats, type TransferStats } from "../api/tauri";
import { isOffline } from "./network";

const IDLE_MS = 4000;
/** While bytes are moving the badge is a live readout, so it updates faster. */
const ACTIVE_MS = 1500;
/** Grace after an idle sample before a transfer counts as stopped: pieces
 *  arrive in bursts and a single sample regularly dips to zero. */
const TRANSFER_GRACE_MS = 8000;
/** Below this a rate reads as idle - see `formatRate`. */
const MOVING_BPS = 1024;

const [stats, setStats] = createSignal<TransferStats | null>(null);
export { stats as transferStats };

const [transferring, setTransferring] = createSignal(false);
/** Is a transfer running right now, dips included? Drives whether the badge
 *  shows a rate readout at all - the rate itself comes from `transferStats`. */
export { transferring as isTransferring };

let lastMovingAt = 0;

let timer: ReturnType<typeof setTimeout> | null = null;
let running = false;

function schedule(delay: number) {
  if (!running) { return; }
  timer = setTimeout(poll, delay);
}

async function poll() {
  if (!running) { return; }
  // Offline drops every manager, so the command would only ever answer zeroes.
  // Keep polling (cheaply) rather than stopping: the user can switch back.
  if (isOffline()) {
    setStats(null);
    setTransferring(false);
    schedule(IDLE_MS);
    return;
  }
  try {
    const next = await getTransferStats();
    // A poll in flight when stopTransferPolling() ran would otherwise write
    // its result after the stop cleared the signal.
    if (!running) { return; }
    setStats(next);
    const moving = next.download_bps >= MOVING_BPS || next.upload_bps >= MOVING_BPS;
    if (moving) { lastMovingAt = Date.now(); }
    const active = moving || Date.now() - lastMovingAt < TRANSFER_GRACE_MS;
    setTransferring(active);
    schedule(active ? ACTIVE_MS : IDLE_MS);
  } catch (e) {
    // Normal right after a mode switch; the last reading stays.
    console.debug("[transfer] stats unavailable:", e);
    schedule(IDLE_MS);
  }
}

/** Start the shared poll loop. Idempotent - the badge and settings panel both
 *  read the same signal rather than each polling on their own. */
export function startTransferPolling() {
  if (running) { return; }
  running = true;
  poll();
}

export function stopTransferPolling() {
  running = false;
  if (timer) { clearTimeout(timer); timer = null; }
  setStats(null);
  setTransferring(false);
  lastMovingAt = 0;
}

/** Bytes/s as a short label. Below 1 KB/s reads as idle: BitTorrent keep-alive
 *  traffic never quite reaches zero and a flickering "312 B/s" is noise. */
export function formatRate(bps: number): string {
  if (bps < MOVING_BPS) { return "0 KB/s"; }
  if (bps < 1024 * 1024) { return `${Math.round(bps / 1024)} KB/s`; }
  return `${(bps / (1024 * 1024)).toFixed(1)} MB/s`;
}
