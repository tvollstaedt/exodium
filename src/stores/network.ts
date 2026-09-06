import { createSignal } from "solid-js";
import { getConfig, initDownloadManager, setConfig } from "../api/tauri";
import { stopAllDownloadTracking } from "./downloads";
import { cancelAllPackJobs } from "./contentPacks";

export type NetworkMode = "live" | "offline";

/** Mirrors the `network_mode` config key (§11); unset means live. */
const [networkMode, setNetworkModeSignal] = createSignal<NetworkMode>("live");
export { networkMode };

export const isOffline = () => networkMode() === "offline";

export async function loadNetworkMode() {
  try {
    const stored = await getConfig("network_mode");
    setNetworkModeSignal(stored === "offline" ? "offline" : "live");
  } catch (e) {
    console.warn("[network] failed to load network_mode:", e);
  }
}

export interface ModeSwitchResult {
  /** Torrent downloads whose tracking was stopped. librqbit keeps the file
   *  selection, so these pick up again when the session returns. */
  downloads: number;
  /** Content-pack installs that were cancelled outright - HTTP transfers with
   *  no resume, so the user has to start them again. */
  packs: number;
}

/** Persist the mode, then rebuild the torrent state (the config write MUST
 *  land before initDownloadManager). Returns what was stopped; rolls the
 *  write back on failure. */
export async function applyNetworkMode(mode: NetworkMode): Promise<ModeSwitchResult> {
  const previous = networkMode();
  setNetworkModeSignal(mode);
  // Trackers and HTTP pack jobs stop BEFORE the managers go.
  const stopped: ModeSwitchResult = mode === "offline"
    ? { downloads: stopAllDownloadTracking(), packs: await cancelAllPackJobs() }
    : { downloads: 0, packs: 0 };
  try {
    await setConfig("network_mode", mode);
    await initDownloadManager();
    return stopped;
  } catch (e) {
    setNetworkModeSignal(previous);
    try {
      await setConfig("network_mode", previous);
    } catch (rollbackError) {
      console.error("[network] could not roll back network_mode:", rollbackError);
    }
    throw e;
  }
}
