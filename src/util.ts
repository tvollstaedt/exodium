import { uninstallGame, resetGameData } from "./api/tauri";
import { refreshLoadedGames, notifyGameLibraryChanged } from "./stores/games";
import { getDownloadState, cancelGameDownload, stopGameDownloadTracking } from "./stores/downloads";
import { showToast } from "./stores/toasts";

export async function performUninstall(
  gameId: number,
  setStatus: (s: string) => void,
  onSuccess?: () => void | Promise<void>,
  title?: string,
): Promise<void> {
  // If a download is in flight, cancel it before removing the directory -
  // otherwise the torrent writer races the uninstall and can leave partial
  // files or error out mid-extract.
  if (getDownloadState(gameId)?.downloading) {
    setStatus("Cancelling download…");
    await cancelGameDownload(gameId);
  }
  // Also kill any non-downloading tracker (extras-phase poller, error card) -
  // it would otherwise resurrect phantom state for the uninstalled game.
  stopGameDownloadTracking(gameId);
  setStatus("Uninstalling...");
  try {
    await uninstallGame(gameId);
    refreshLoadedGames();
    notifyGameLibraryChanged(gameId);
    await onSuccess?.();
    setStatus("");
    showToast(title ? `Uninstalled ${title}` : "Uninstalled", "success");
  } catch (e) {
    console.error("Uninstall failed:", e);
    setStatus("");
    showToast(title ? `Couldn't uninstall ${title}` : "Uninstall failed", "error", { detail: String(e) });
  }
}

/** Reset a game (§5), shared by the context menu and the panel. The backend
 *  returns the success message; `title` is for the failure toast. */
export async function performReset(
  gameId: number,
  setStatus: (s: string) => void,
  title?: string,
): Promise<void> {
  setStatus("Resetting…");
  try {
    const msg = await resetGameData(gameId);
    showToast(msg, "success");
  } catch (e) {
    console.error("Reset failed:", e);
    showToast(title ? `Couldn't reset ${title}` : "Reset failed", "error", { detail: String(e) });
  } finally {
    setStatus("");
  }
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} MB`;
  if (bytes >= 1e3) return `${(bytes / 1e3).toFixed(0)} KB`;
  return `${bytes} B`;
}

export interface LangEntry { lang: string | null; state: number }

export function parseLangEntries(game: {
  available_languages?: string | null;
  language?: string | null;
  installed?: boolean;
  in_library?: boolean;
}): LangEntry[] {
  const raw = game.available_languages;
  if (!raw) {
    const state = game.installed ? 2 : game.in_library ? 1 : 0;
    return [{ lang: game.language ?? null, state }];
  }
  return raw.split(",").map((entry) => {
    const parts = entry.split(":");
    const lang = parts[0] ?? null;
    const state = parts[1] != null ? parseInt(parts[1], 10) : 0;
    return { lang, state: isNaN(state) ? 0 : state };
  });
}

/** The collection family a row belongs to, as the grid shows it next to
 *  the language badges when no collection is selected. */
export function platformTag(torrentSource: string | null | undefined): string | null {
  if (!torrentSource) { return null; }
  if (torrentSource.startsWith("eXoDOS")) { return "DOS"; }
  if (torrentSource === "eXoWin3x") { return "Win3x"; }
  if (torrentSource === "eXoWin9x") { return "Win9x"; }
  if (torrentSource === "eXoScummVM") { return "ScummVM"; }
  return null;
}

export function langBadgeClass(state: number): string {
  if (state === 2) { return "lang-installed"; }
  if (state === 1) { return "lang-downloading"; }
  return "";
}

/** Client-side title match for the in-memory shelves, variant titles
 *  included, like the Browse SQL filter. */
export function matchesLibraryQuery(
  game: { title?: string | null; sort_title?: string | null; variant_titles?: string | null },
  query: string,
): boolean {
  const q = query.trim().toLowerCase();
  if (!q) { return true; }
  return (game.title ?? "").toLowerCase().includes(q)
    || (game.sort_title ?? "").toLowerCase().includes(q)
    || (game.variant_titles ?? "").toLowerCase().includes(q);
}
