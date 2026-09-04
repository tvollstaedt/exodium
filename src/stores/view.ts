import { createSignal } from "solid-js";
import { getConfig, setConfig } from "../api/tauri";

export type ViewMode = "grid" | "list";

/** Browse presentation: cover grid or tabular list (#21). Mirrors the
 *  `view_mode` config key; unset means "grid" so existing installs are
 *  unaffected. */
const [viewMode, setViewModeSignal] = createSignal<ViewMode>("grid");
export { viewMode };

export async function loadViewMode() {
  try {
    const stored = await getConfig("view_mode");
    setViewModeSignal(stored === "list" ? "list" : "grid");
  } catch (e) {
    console.warn("[view] failed to load view_mode:", e);
  }
}

/** The music chip flips the mode for the session without changing the user's
 *  saved preference. */
export function setViewModeTransient(mode: ViewMode) {
  setViewModeSignal(mode);
}

/** Optimistic: the toggle must feel instant, and a failed write only costs
 *  persistence across restarts, not the current session. */
export function applyViewMode(mode: ViewMode) {
  setViewModeSignal(mode);
  setConfig("view_mode", mode).catch((e) => {
    console.warn("[view] failed to persist view_mode:", e);
  });
}
