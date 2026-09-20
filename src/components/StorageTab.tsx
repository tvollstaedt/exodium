import { createSignal, createEffect, on, Show, For } from "solid-js";
import { Button } from "./Button";
import { SettingRow, SettingSwitch, SectionTitle } from "./SettingRow";
import { PieChart, Gamepad2 } from "lucide-solid";
import {
  storageOverview, installedGamesStorage, clearMediaCaches, deleteSaveBackups,
  removeInstalledArchives, archiveUsage, getConfig, setConfig,
  type StorageOverview, type StorageCategory, type StorageCategoryId, type GameStorage, type ArchiveUsage,
} from "../api/tauri";
import { formatBytes, platformTag, performUninstall } from "../util";
import { showToast } from "../stores/toasts";
import { lastGameLibraryChange } from "../stores/games";
import { forgetVideos } from "../stores/videos";
import { forgetMusic } from "../stores/music";
import { invalidateMetadata } from "../stores/metadata";

/** Order, wording and colour per category. The bar paints them in this
 *  order, so the big ones (games, packs) come first. */
const CATEGORIES: Record<StorageCategoryId, { name: string; hint: string; color: string; unit?: string }> = {
  games: { name: "Games", hint: "Unpacked game folders", color: "#7c6cf2", unit: "game" },
  archives: { name: "Game archives", hint: "Downloaded ZIPs kept beside unpacked games", color: "#9d8ef5", unit: "archive" },
  extras: { name: "Extras", hint: "Videos, manuals and music that DOS games share", color: "#4c9ee8", unit: "archive" },
  packs: { name: "Content packs", hint: "Covers, screenshots, manuals, emulators", color: "#39b8a6" },
  pack_archives: { name: "Pack archives", hint: "Downloaded pack ZIPs from the torrents", color: "#5fd4c3", unit: "archive" },
  support: { name: "Emulators & support", hint: "eXo's own emulator builds, Windows 9x system images, ScummVM ROMs", color: "#e0a83a" },
  reading: { name: "Reading room", hint: "Magazines, books and catalogs - remove a document from its context menu in the reading room", color: "#e8735a", unit: "document" },
  saves: { name: "Save backups", hint: "What uninstalls set aside. Delete skips backups of games in your library and backups that hold a whole game folder.", color: "#d85c8f", unit: "backup" },
  caches: { name: "Media caches", hint: "Preview videos, theme music, gallery thumbnails", color: "#8b95a8" },
  configs: { name: "Launch configs", hint: "Per-game DOSBox and emulator configs", color: "#6b7385" },
  other: { name: "Other", hint: "Torrent placeholders and pieces shared with neighbouring games", color: "#4b5162" },
};

const BAR_ORDER: StorageCategoryId[] = [
  "games", "archives", "extras", "packs", "pack_archives", "support", "reading", "saves", "caches", "configs", "other",
];

const plural = (n: number, unit: string) => `${n.toLocaleString()} ${unit}${n === 1 ? "" : "s"}`;

function lastPlayed(iso: string | null): string {
  if (!iso) { return "Never played"; }
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "Never played" : `Played ${d.toLocaleDateString()}`;
}

/** Disk usage by what it is for, plus every installed game by size - the
 *  Steam storage manager's shape. */
// Module-level: a measurement survives switching sections and reopening the
// dialog. Refresh, every action here, and a library change re-measure.
const [overview, setOverview] = createSignal<StorageOverview | null>(null);
const [games, setGames] = createSignal<GameStorage[]>([]);
const [installedArchives, setInstalledArchives] = createSignal<ArchiveUsage | null>(null);
const [keepArchives, setKeepArchives] = createSignal(true);
let measuredAt = -1;

/** A new game folder or a factory reset: nothing measured still applies. */
export function resetStorageCache() {
  setOverview(null);
  setGames([]);
  setInstalledArchives(null);
  measuredAt = -1;
}

export function StorageTab(props: { active: boolean; onGoToPacks: () => void }) {
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [confirm, setConfirm] = createSignal<string | null>(null);
  const [sort, setSort] = createSignal<"size" | "name" | "played">("size");
  const [uninstalling, setUninstalling] = createSignal<ReadonlySet<number>>(new Set());
  const markUninstalling = (id: number, on: boolean) => {
    const next = new Set(uninstalling());
    if (on) { next.add(id); } else { next.delete(id); }
    setUninstalling(next);
  };

  const load = async () => {
    setLoading(true);
    setError("");
    const stamp = lastGameLibraryChange()?.ts ?? 0;
    try {
      const [o, g, a, keep] = await Promise.all([
        storageOverview(), installedGamesStorage(), archiveUsage(), getConfig("keep_archives"),
      ]);
      setOverview(o);
      setGames(g);
      setInstalledArchives(a);
      setKeepArchives(keep !== "0");
      measuredAt = stamp;
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };
  // The walk takes seconds on a large library: once per visit, and again
  // only after the library changed (an uninstall from the grid, an install).
  createEffect(on(() => props.active, (active) => {
    setConfirm(null);
    if (active && !loading() && (!overview() || measuredAt !== (lastGameLibraryChange()?.ts ?? 0))) { void load(); }
  }));

  const category = (id: StorageCategoryId): StorageCategory | undefined =>
    overview()?.categories.find((c) => c.id === id);
  const byteShare = (bytes: number) => {
    const total = overview()?.total_bytes ?? 0;
    return total > 0 ? `${(bytes / total) * 100}%` : "0%";
  };
  const otherOnDisk = () => overview()?.other_bytes ?? 0;

  const sortedGames = () => {
    const list = games().map((g) => ({ ...g, total: g.game_bytes + g.archive_bytes + g.save_bytes }));
    switch (sort()) {
      case "name": return list.sort((a, b) => a.title.localeCompare(b.title));
      case "played": return list.sort((a, b) => (b.last_played ?? "").localeCompare(a.last_played ?? ""));
      default: return list.sort((a, b) => b.total - a.total);
    }
  };

  /** Two-click destructive actions: the first click arms the label. */
  const run = async (key: string, action: () => Promise<{ items: number; bytes: number }>, done: (r: { items: number; bytes: number }) => string) => {
    if (confirm() !== key) { setConfirm(key); return; }
    setConfirm(null);
    setBusy(key);
    try {
      const r = await action();
      showToast(done(r), "success");
      if (key === "caches") {
        // Settled media states point at files that are gone now.
        forgetVideos();
        invalidateMetadata();
        void forgetMusic();
      }
      await load();
    } catch (e) {
      showToast("Couldn't free the space", "error", { detail: String(e) });
    } finally {
      setBusy(null);
    }
  };

  const toggleKeep = async (next: boolean) => {
    setKeepArchives(next);
    try {
      await setConfig("keep_archives", next ? "1" : "0");
    } catch (e) {
      console.error("[settings] failed to save keep_archives:", e);
      setKeepArchives(!next);
    }
  };

  /** Several may run at once; the list refreshes per game, the (slow) disk
   *  walk once the last one is done. */
  const uninstall = async (g: GameStorage) => {
    if (confirm() !== `game:${g.id}`) { setConfirm(`game:${g.id}`); return; }
    setConfirm(null);
    markUninstalling(g.id, true);
    try {
      await performUninstall(g.id, () => {}, undefined, g.title);
      try { setGames(await installedGamesStorage()); } catch { /* keep the old list */ }
    } finally {
      markUninstalling(g.id, false);
      if (uninstalling().size === 0) { void load(); }
    }
  };

  const actionLabel = (key: string, idle: string, armed: string) =>
    confirm() === key ? armed : idle;

  return (
    <div class="storage" data-testid="storage-tab">
      <Show when={error()}>
        <div class="error">{error()}</div>
      </Show>
      <Show when={loading() && !overview()}>
        <div class="storage-loading"><span class="btn-spinner" /> Measuring the game folder - up to a minute on a large library…</div>
      </Show>

      <Show when={overview()}>
        {(o) => (
          <>
            <section class="storage-drive">
              <div class="storage-drive-head">
                <span class="storage-drive-path" title={o().folder}>{o().folder}</span>
                <Button variant="small" loading={loading()} loadingLabel="Measuring…" onClick={() => void load()}>Refresh</Button>
              </div>
              <div class="storage-bar" role="img" aria-label="Disk usage">
                <For each={BAR_ORDER}>
                  {(id) => (
                    <Show when={(category(id)?.bytes ?? 0) > 0}>
                      <span
                        class="storage-bar-seg"
                        style={{ width: byteShare(category(id)!.bytes), background: CATEGORIES[id].color }}
                        title={`${CATEGORIES[id].name} · ${formatBytes(category(id)!.bytes)}`}
                      />
                    </Show>
                  )}
                </For>
                <span class="storage-bar-seg is-foreign" style={{ width: byteShare(otherOnDisk()) }} title={`Other files on this disk · ${formatBytes(otherOnDisk())}`} />
              </div>
              <div class="storage-drive-line">
                <span><b>{formatBytes(o().used_bytes)}</b> used by Exodium</span>
                <span>{formatBytes(otherOnDisk())} other files</span>
                <span><b>{formatBytes(o().free_bytes)}</b> free of {formatBytes(o().total_bytes)}</span>
              </div>
            </section>

            <section class="settings-section">
              <SectionTitle icon={PieChart}>What takes the space</SectionTitle>
              <For each={BAR_ORDER}>
                {(id) => {
                  const c = () => category(id);
                  const meta = CATEGORIES[id];
                  return (
                    <Show when={c() && (c()!.bytes > 0 || id === "games" || id === "archives")}>
                      <SettingRow
                        label={<span class="storage-cat-label" style={{ "--dot": meta.color }}>{meta.name}</span>}
                        value={
                          <span class="storage-cat-size">
                            {formatBytes(c()!.bytes)}
                            <Show when={meta.unit && c()!.items > 0}>
                              <span class="storage-cat-items"> · {plural(c()!.items, meta.unit!)}</span>
                            </Show>
                          </span>
                        }
                        htmlFor={id === "archives" ? "keep-archives" : undefined}
                        hint={id === "archives"
                          ? `${keepArchives()
                            ? "Kept after install so Reset needs no download and games can be seeded."
                            : "Not kept after install - Reset downloads the game again, and those games cannot be seeded."}${
                            (installedArchives()?.count ?? 0) > 0
                              ? ` Remove takes the ${plural(installedArchives()!.count, "archive")} of installed games (${formatBytes(installedArchives()!.bytes)}).`
                              : ""}`
                          : meta.hint}
                      >
                        <Show when={id === "archives"}>
                          <SettingSwitch id="keep-archives" checked={keepArchives()} onChange={toggleKeep} label="Keep archives after install" />
                          <Show when={(installedArchives()?.count ?? 0) > 0}>
                            <Button variant="small" loading={busy() === "archives"} loadingLabel="Removing…"
                              onClick={() => void run("archives",
                                () => removeInstalledArchives().then((r) => ({ items: r.count, bytes: r.bytes })),
                                (r) => `Removed ${plural(r.items, "archive")} · ${formatBytes(r.bytes)} freed`)}>
                              {actionLabel("archives", "Remove", `Free ${formatBytes(installedArchives()!.bytes)}?`)}
                            </Button>
                          </Show>
                        </Show>
                        <Show when={id === "saves" && c()!.bytes > 0}>
                          <Button variant="small" loading={busy() === "saves"} loadingLabel="Deleting…"
                            onClick={() => void run("saves", deleteSaveBackups,
                              (r) => `Deleted ${plural(r.items, "backup")} · ${formatBytes(r.bytes)} freed`)}>
                            {actionLabel("saves", "Delete", `Delete ${formatBytes(c()!.bytes)} of backups?`)}
                          </Button>
                        </Show>
                        <Show when={id === "caches" && c()!.bytes > 0}>
                          <Button variant="small" loading={busy() === "caches"} loadingLabel="Clearing…"
                            onClick={() => void run("caches", clearMediaCaches,
                              (r) => `Cleared ${formatBytes(r.bytes)} of cached media`)}>
                            {actionLabel("caches", "Clear", "Clear caches?")}
                          </Button>
                        </Show>
                        <Show when={id === "packs"}>
                          <Button variant="small" onClick={props.onGoToPacks}>Manage</Button>
                        </Show>
                      </SettingRow>
                    </Show>
                  );
                }}
              </For>
            </section>

            <section class="settings-section">
              <div class="storage-games-head">
                <SectionTitle icon={Gamepad2}>Installed games · {games().length}</SectionTitle>
                <label class="storage-sort">
                  Sort by
                  <select value={sort()} onChange={(e) => setSort(e.currentTarget.value as "size" | "name" | "played")}>
                    <option value="size">Size</option>
                    <option value="name">Name</option>
                    <option value="played">Last played</option>
                  </select>
                </label>
              </div>
              <Show when={games().length === 0}>
                <p class="settings-row-hint">No games installed.</p>
              </Show>
              <div class="storage-games">
                <For each={sortedGames()}>
                  {(g) => (
                    <div class="storage-game" data-testid="storage-game">
                      <div class="storage-game-main">
                        <span class="storage-game-title">{g.title}</span>
                        <span class="storage-game-meta">
                          <Show when={platformTag(g.collection)}>
                            <span class="storage-game-tag">{platformTag(g.collection)}</span>
                          </Show>
                          <span class="storage-game-tag">{g.language}</span>
                          <span>{lastPlayed(g.last_played)}</span>
                        </span>
                      </div>
                      <div class="storage-game-size">
                        <b>{formatBytes(g.total)}</b>
                        <Show when={g.archive_bytes > 0 || g.save_bytes > 0}>
                          <span class="storage-game-breakdown">
                            game {formatBytes(g.game_bytes)}
                            <Show when={g.archive_bytes > 0}> · archive {formatBytes(g.archive_bytes)}</Show>
                            <Show when={g.save_bytes > 0}> · saves {formatBytes(g.save_bytes)}</Show>
                          </span>
                        </Show>
                      </div>
                      <Button variant="small" loading={uninstalling().has(g.id)} loadingLabel="Uninstalling…"
                        onClick={() => void uninstall(g)}>
                        {confirm() === `game:${g.id}` ? "Confirm uninstall?" : "Uninstall"}
                      </Button>
                    </div>
                  )}
                </For>
              </div>
            </section>
          </>
        )}
      </Show>
    </div>
  );
}
