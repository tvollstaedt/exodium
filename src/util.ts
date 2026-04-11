import { uninstallGame } from "./api/tauri";
import { fetchGames } from "./stores/games";

export async function performUninstall(
  gameId: number,
  setStatus: (s: string) => void,
  onSuccess?: () => void | Promise<void>,
): Promise<void> {
  setStatus("Uninstalling...");
  try {
    await uninstallGame(gameId);
    fetchGames();
    await onSuccess?.();
    setStatus("Uninstalled");
    setTimeout(() => setStatus(""), 2000);
  } catch (e) {
    console.error("Uninstall failed:", e);
    setStatus(`Error: ${String(e)}`);
    setTimeout(() => setStatus(""), 3000);
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

export function langBadgeClass(state: number): string {
  if (state === 2) { return "lang-installed"; }
  if (state === 1) { return "lang-downloading"; }
  return "";
}
