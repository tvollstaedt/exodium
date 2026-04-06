import { createSignal, onMount, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Progress } from "@ark-ui/solid/progress";
import type { Game } from "../api/tauri";
import { launchGame, getGameVariants, uninstallGame } from "../api/tauri";
import { formatBytes } from "../util";
import { thumbnailDirForCollection } from "../stores/thumbnails";
import { downloads, startGameDownload, getDownloadState } from "../stores/downloads";
import { fetchGames, toggleFavorite } from "../stores/games";

interface GameCardProps {
  game: Game;
  onFavoriteChanged?: (id: number, favorited: boolean) => void;
  showFavoriteBtn?: boolean;
}

export function GameCard(props: GameCardProps) {
  const [status, setStatus] = createSignal("");
  const [imgError, setImgError] = createSignal(false);
  const [favorited, setFavorited] = createSignal(props.game.favorited);
  const [showLangPicker, setShowLangPicker] = createSignal(false);
  const [variants, setVariants] = createSignal<Game[]>([]);
  const [contextMenu, setContextMenu] = createSignal<{x: number, y: number} | null>(null);
  const [confirmUninstall, setConfirmUninstall] = createSignal(false);

  // Pre-load variant IDs for multi-lang games so download state is visible on main card
  onMount(async () => {
    if (isMultiLang() && props.game.shortcode && variants().length === 0) {
      try {
        const v = await getGameVariants(props.game.shortcode);
        setVariants(v);
      } catch {}
    }
  });

  const thumbSrc = () => {
    const dir = thumbnailDirForCollection(props.game.torrent_source);
    if (!dir || !props.game.shortcode || !props.game.has_thumbnail) return null;
    return convertFileSrc(`${dir}/${props.game.shortcode}.jpg`);
  };

  // Parse "DE:0,EN:2" format: 0=available, 1=downloading, 2=installed
  const langEntries = () => {
    const raw = props.game.available_languages;
    if (!raw) {
      const state = props.game.installed ? 2 : props.game.in_library ? 1 : 0;
      return [{ lang: props.game.language, state }];
    }
    return raw.split(",").map((entry) => {
      const [lang, flag] = entry.split(":");
      return { lang, state: parseInt(flag) || 0 };
    });
  };
  const availLangs = () => langEntries().map((e) => e.lang);
  const isMultiLang = () => availLangs().length > 1;

  const langBadgeClass = (state: number) => {
    if (state === 2) return "lang-installed";   // green
    if (state === 1) return "lang-downloading"; // amber
    return "";                                   // blue (default)
  };

  // Read download state — check primary game and any loaded variants
  const dlState = () => {
    const dl = downloads();
    if (props.game.id != null && dl[props.game.id]) return dl[props.game.id];
    // Check loaded variants
    for (const v of variants()) {
      if (v.id != null && dl[v.id]?.downloading) return dl[v.id];
    }
    return undefined;
  };

  const handleDownload = (gameId: number) => {
    setShowLangPicker(false);
    startGameDownload(gameId);
  };

  const handleUninstall = async (gameId: number) => {
    setShowLangPicker(false);
    try {
      setStatus("Uninstalling...");
      await uninstallGame(gameId);
      // Refresh everything
      fetchGames();
      if (props.game.shortcode) {
        const v = await getGameVariants(props.game.shortcode).catch(() => []);
        setVariants(v);
      }
      setStatus("Uninstalled");
      setTimeout(() => setStatus(""), 2000);
    } catch (e) {
      setStatus(`Error: ${String(e)}`);
      setTimeout(() => setStatus(""), 3000);
    }
  };

  const handleLaunch = async (gameId: number) => {
    setShowLangPicker(false);
    setStatus("Launching...");
    try {
      const result = await launchGame(gameId);
      setStatus(result);
    } catch (e) {
      setStatus(`Error: ${e}`);
    }
    setTimeout(() => setStatus(""), 3000);
  };

  const handleContextMenu = (e: MouseEvent) => {
    if ((!props.game.installed && !props.game.in_library) || props.game.id == null) return;
    e.preventDefault();
    setConfirmUninstall(false);
    setContextMenu({ x: e.clientX, y: e.clientY });
  };

  const handleClick = async (e: MouseEvent) => {
    e.stopPropagation();
    if (props.game.id == null) return;

    if (isMultiLang() && props.game.shortcode) {
      try {
        const v = await getGameVariants(props.game.shortcode);
        setVariants(v);
        setShowLangPicker(true);
      } catch (err) {
        setStatus(`Error: ${err}`);
      }
      return;
    }

    // Single language
    if (props.game.installed) {
      await handleLaunch(props.game.id);
    } else if (props.game.game_torrent_index != null && !dlState()?.downloading) {
      handleDownload(props.game.id);
    } else if (!props.game.game_torrent_index) {
      setStatus("No download available");
      setTimeout(() => setStatus(""), 2000);
    }
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

  const currentStatus = () => dlState()?.status || status();
  const currentProgress = () => dlState()?.progress ?? 0;
  const isDownloading = () => dlState()?.downloading ?? false;

  return (
    <div class={`game-card ${props.game.installed || props.game.in_library ? "installed" : ""}`} onContextMenu={handleContextMenu}>
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
            <Show when={!props.game.in_library && props.game.download_size && !isMultiLang() && !isDownloading()}>
              <span class="badge badge-size">
                {formatBytes(props.game.download_size!)}
              </span>
            </Show>
            <Show when={isDownloading()}>
              <Progress.Root value={currentProgress() * 100} class="ark-progress mini">
                <Progress.Track class="ark-progress-track">
                  <Progress.Range class="ark-progress-range" />
                </Progress.Track>
              </Progress.Root>
            </Show>
          </div>
          {currentStatus() && <div class="game-card-status">{currentStatus()}</div>}
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
                setContextMenu(null);
                handleUninstall(props.game.id!);
              } else {
                setConfirmUninstall(true);
              }
            }}>
              {confirmUninstall() ? "Confirm uninstall?" : "Uninstall"}
            </button>
          </div>
        </Portal>
      </Show>

      <Show when={showLangPicker()}>
        <div class="lang-picker-backdrop" onClick={() => setShowLangPicker(false)} />
        <div class="lang-picker">
          <div class="lang-picker-title">Select version</div>
          <For each={variants()}>
            {(variant) => {
              const vDl = () => variant.id != null ? getDownloadState(variant.id) : undefined;
              return (
                <div class={`lang-picker-item ${variant.installed ? "is-installed" : ""}`}>
                  <span class="badge badge-lang">{variant.language}</span>
                  <span class="lang-picker-label">{variant.title}</span>
                  <Show when={vDl()?.downloading}>
                    <span class="lang-picker-action action-download">{vDl()!.status}</span>
                  </Show>
                  <Show when={!vDl()?.downloading && variant.installed}>
                    <button class="lang-picker-btn action-play" onClick={(e) => { e.stopPropagation(); handleLaunch(variant.id!); }}>
                      ▶ Play
                    </button>
                    <button class="lang-picker-btn action-uninstall" onClick={(e) => { e.stopPropagation(); handleUninstall(variant.id!); }}>
                      ✕
                    </button>
                  </Show>
                  <Show when={!vDl()?.downloading && !variant.installed}>
                    <button
                      class="lang-picker-btn action-download"
                      onClick={(e) => {
                        e.stopPropagation();
                        if (variant.game_torrent_index != null) handleDownload(variant.id!);
                      }}
                    >
                      {variant.game_torrent_index != null ? `↓ ${formatBytes(variant.download_size ?? 0)}` : "—"}
                    </button>
                  </Show>
                </div>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}
