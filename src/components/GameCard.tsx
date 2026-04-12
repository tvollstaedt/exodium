import { createSignal, createEffect, on, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Progress } from "@ark-ui/solid/progress";
import type { Game } from "../api/tauri";
import { getGameVariants } from "../api/tauri";
import { formatBytes, parseLangEntries, langBadgeClass, performUninstall } from "../util";
import { bestThumbnailPath } from "../stores/thumbnails";
import { downloads, cancelGameDownload } from "../stores/downloads";
import { toggleFavorite } from "../stores/games";

interface GameCardProps {
  game: Game;
  onFavoriteChanged?: (id: number, favorited: boolean) => void;
  showFavoriteBtn?: boolean;
  onDetail: (game: Game) => void;
}

export function GameCard(props: GameCardProps) {
  const [status, setStatus] = createSignal("");
  const [imgError, setImgError] = createSignal(false);
  const [favorited, setFavorited] = createSignal(props.game.favorited);
  const [variants, setVariants] = createSignal<Game[]>([]);
  const [contextMenu, setContextMenu] = createSignal<{x: number, y: number} | null>(null);
  const [confirmUninstall, setConfirmUninstall] = createSignal(false);

  // Re-sync favorited from props only when the card is reused for a different game (For loop
  // key change). Do NOT run on favorited-flag-only changes — that would race with the
  // optimistic update in handleToggleFavorite and cause a visible flicker.
  createEffect(on(() => props.game.id, () => { setFavorited(props.game.favorited); }, { defer: true }));

  // Pre-load variant IDs for multi-lang games so download state is visible on main card.
  // createEffect re-runs when props.game.shortcode changes, handling component reuse in For loops.
  createEffect(() => {
    const shortcode = props.game.shortcode;
    if (!isMultiLang() || !shortcode) { return; }
    getGameVariants(shortcode)
      .then((v) => { if (props.game.shortcode === shortcode) { setVariants(v); } })
      .catch(() => {});
  });

  const thumbSrc = () => {
    const path = bestThumbnailPath(props.game.torrent_source, props.game.shortcode, props.game.has_thumbnail);
    if (!path) { return null; }
    return convertFileSrc(path);
  };

  const langEntries = () => parseLangEntries(props.game);
  const isMultiLang = () => langEntries().length > 1;

  // Read download state — check primary game and any loaded variants
  const dlState = () => {
    const dl = downloads();
    if (props.game.id != null && dl[props.game.id]) { return dl[props.game.id]; }
    for (const v of variants()) {
      if (v.id != null && dl[v.id]?.downloading) { return dl[v.id]; }
    }
    return undefined;
  };

  const handleContextMenu = (e: MouseEvent) => {
    if ((!props.game.installed && !props.game.in_library) || props.game.id == null) { return; }
    e.preventDefault();
    setConfirmUninstall(false);
    setContextMenu({ x: e.clientX, y: e.clientY });
  };

  const handleContextUninstall = async () => {
    setContextMenu(null);
    if (props.game.id == null) { return; }
    await performUninstall(props.game.id, setStatus);
  };

  const handleClick = (e: MouseEvent) => {
    e.stopPropagation();
    props.onDetail(props.game);
  };

  const handleToggleFavorite = async (e: MouseEvent) => {
    e.stopPropagation();
    if (props.game.id == null) { return; }
    const prev = favorited();
    setFavorited(!prev);
    try {
      const next = await toggleFavorite(props.game.id);
      setFavorited(next);
      props.onFavoriteChanged?.(props.game.id, next);
    } catch {
      setFavorited(prev);
    }
  };

  const currentProgress = () => dlState()?.progress ?? 0;
  const isDownloading = () => dlState()?.downloading ?? false;

  return (
    <div class={`game-card ${props.game.installed || props.game.in_library ? "installed" : ""}`} onContextMenu={handleContextMenu} data-game-id={props.game.id != null ? String(props.game.id) : undefined}>
      <div onClick={handleClick}>
        <Show when={thumbSrc() && !imgError()}>
          <img
            class="game-card-thumb"
            src={thumbSrc()!}
            alt=""
            loading="lazy"
            onError={() => setImgError(true)}
          />
        </Show>
        <div class="game-card-body">
          <div class="game-card-title">{props.game.title}</div>
          <div class="game-card-meta">
            {props.game.year && <span>{props.game.year}</span>}
            {props.game.genre && <span class="genre">{props.game.genre}</span>}
          </div>
          <div class="game-card-footer">
            <For each={langEntries()}>
              {(entry) => (
                <span class={`badge badge-lang ${langBadgeClass(entry.state)}`}>
                  {entry.lang}
                </span>
              )}
            </For>
          </div>
          <div class="game-card-action-bar">
            <Show when={status()}>
              <span class="card-action-label action-downloading">{status()}</span>
            </Show>
            <Show when={!status()}>
              <Show when={isDownloading()}>
                <Progress.Root value={currentProgress() * 100} class="ark-progress mini">
                  <Progress.Track class="ark-progress-track">
                    <Progress.Range class="ark-progress-range" />
                  </Progress.Track>
                </Progress.Root>
                <span class="card-action-label action-downloading">{dlState()?.status}</span>
                <Show when={props.game.id != null}>
                  <button class="card-cancel-btn" title="Cancel download"
                    onClick={(e) => { e.stopPropagation(); cancelGameDownload(props.game.id!); }}>✕</button>
                </Show>
              </Show>
              <Show when={!isDownloading() && props.game.installed}>
                <span class="card-action-label action-installed">▶ Play</span>
              </Show>
              <Show when={!isDownloading() && !props.game.installed && props.game.in_library}>
                <span class="card-action-label action-incomplete">⚠ Incomplete</span>
              </Show>
              <Show when={!isDownloading() && !props.game.installed && !props.game.in_library}>
                <span class="card-action-label action-download">
                  {props.game.download_size ? `↓ ${formatBytes(props.game.download_size)}` : "↓ Download"}
                </span>
              </Show>
            </Show>
          </div>
        </div>
      </div>

      <Show when={props.game.id != null && props.showFavoriteBtn !== false}>
        <button
          class={`favorite-btn${favorited() ? " is-favorited" : ""}`}
          onClick={handleToggleFavorite}
          title={favorited() ? "Remove from favorites" : "Add to favorites"}
        >★</button>
      </Show>

      <Show when={contextMenu()}>
        <Portal>
          <div class="context-backdrop" onMouseDown={() => setContextMenu(null)} onContextMenu={(e) => { e.preventDefault(); setContextMenu(null); }} />
          <div class="context-menu" style={{ left: `${contextMenu()!.x}px`, top: `${contextMenu()!.y}px` }}>
            <button class="context-menu-item danger" onMouseDown={(e) => e.stopPropagation()} onClick={() => {
              if (confirmUninstall()) {
                handleContextUninstall();
              } else {
                setConfirmUninstall(true);
              }
            }}>
              {confirmUninstall() ? "Confirm uninstall?" : "Uninstall"}
            </button>
          </div>
        </Portal>
      </Show>
    </div>
  );
}
