import { createSignal, createEffect, on, Show, For } from "solid-js";
import type { Game } from "../api/tauri";
import { loadVariants } from "../stores/variants";
import { formatBytes, parseLangEntries, langBadgeClass } from "../util";
import { downloads, cancelGameDownload } from "../stores/downloads";
import { isOffline } from "../stores/network";
import { toggleFavorite } from "../stores/games";
import {
  playableHint, playFromList, togglePlay, currentTrack, musicPlaying, musicCached, getMusicState,
} from "../stores/music";
import { GameActionsMenu } from "./GameActionsMenu";

interface GameRowProps {
  game: Game;
  onFavoriteChanged?: (id: number, favorited: boolean) => void;
  onDetail: (game: Game) => void;
}

/** One line of the Browse list view (#21) - GameCard's exact props, so both
 *  slot into the same render sites. Shares the variants and downloads stores
 *  with the cards; no fetches of its own beyond the variant preload. */
export function GameRow(props: GameRowProps) {
  const [status, setStatus] = createSignal("");
  const [favorited, setFavorited] = createSignal(props.game.favorited);
  const [variants, setVariants] = createSignal<Game[]>([]);
  const [contextMenu, setContextMenu] = createSignal<{x: number, y: number} | null>(null);

  createEffect(on(() => props.game.id, () => { setFavorited(props.game.favorited); }, { defer: true }));

  const langEntries = () => parseLangEntries(props.game);
  const isMultiLang = () => langEntries().length > 1;

  // Same preload as GameCard: without it a variant's download would be
  // invisible on the merged row. Deduped by the promise cache in the store.
  createEffect(() => {
    const shortcode = props.game.shortcode;
    if (!isMultiLang() || !shortcode) { return; }
    loadVariants(props.game)
      .then((v) => { if (props.game.shortcode === shortcode) { setVariants(v); } })
      .catch(() => {});
  });

  const dlEntry = () => {
    const dl = downloads();
    // ?.downloading also for the primary: a finished/failed entry lingers in
    // the store (extras phase, errors are never cleaned up) and would shadow
    // a variant's LIVE download - and a non-downloading entry is never
    // rendered here anyway.
    if (props.game.id != null && dl[props.game.id]?.downloading) {
      return { id: props.game.id, state: dl[props.game.id] };
    }
    for (const v of variants()) {
      if (v.id != null && dl[v.id]?.downloading) { return { id: v.id, state: dl[v.id] }; }
    }
    return undefined;
  };
  const dlState = () => dlEntry()?.state;
  const isDownloading = () => dlState()?.downloading ?? false;

  const handleContextMenu = (e: MouseEvent) => {
    if (props.game.id == null) { return; }
    e.preventDefault();
    setContextMenu({ x: e.clientX, y: e.clientY });
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

  const genreText = () => (props.game.genre ?? "").split(";").map(s => s.trim()).filter(Boolean).join(", ");

  const isCurrentTrack = () => props.game.id != null && currentTrack()?.gameId === props.game.id;
  const isCached = () => props.game.id != null && musicCached().has(props.game.id);
  const isBusy = () => {
    if (props.game.id == null) { return false; }
    const phase = getMusicState(props.game.id)?.phase;
    return phase === "probing" || phase === "fetching";
  };
  // Offline, an uncached track can never arrive - the button stays visible so
  // the column doesn't jump, but says why it does nothing.
  const playBlocked = () => isOffline() && !isCached();
  const playTitle = () => {
    if (playBlocked()) { return "Not cached – offline"; }
    if (isCurrentTrack() && musicPlaying()) { return "Pause"; }
    return "Play theme";
  };
  const onPlayClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (isCurrentTrack()) { togglePlay(); } else { playFromList(props.game); }
  };

  return (
    <div
      class={`game-row ${props.game.installed || props.game.in_library ? "installed" : ""}${isCurrentTrack() ? " is-playing" : ""}`}
      data-game-id={props.game.id != null ? String(props.game.id) : undefined}
      onClick={(e) => {
        // Solid's delegated clicks walk back through the Portal to this row,
        // so a click on a context-menu item that does NOT unmount the menu
        // (the confirm-arming Uninstall/Reset clicks) would open the detail
        // panel underneath it. Only real DOM descendants count as row clicks.
        if (!e.currentTarget.contains(e.target as Node)) { return; }
        props.onDetail(props.game);
      }}
      onContextMenu={handleContextMenu}
    >
      <Show when={props.game.id != null} fallback={<span class="row-fav" />}>
        <button
          class={`row-fav${favorited() ? " is-favorited" : ""}`}
          onClick={handleToggleFavorite}
          title={favorited() ? "Remove from favorites" : "Add to favorites"}
        >★</button>
      </Show>
      {/* The cell is always rendered so the shared grid template keeps its
          column even on rows with no theme. */}
      <span class="row-play">
        <Show when={playableHint(props.game) && props.game.id != null}>
          <button
            class={`row-play-btn${isCurrentTrack() ? " is-current" : ""}${isBusy() ? " is-busy" : ""}${isCached() ? " is-cached" : ""}`}
            disabled={playBlocked()}
            title={playTitle()}
            onClick={onPlayClick}
          >{isCurrentTrack() && musicPlaying() ? "⏸" : "▶"}</button>
        </Show>
      </span>
      <span class="row-title" title={props.game.title}>
        <span class="row-title-text">{props.game.title}</span>
        <For each={langEntries()}>
          {(entry) => (
            <span class={`badge badge-lang ${langBadgeClass(entry.state)}`}>{entry.lang}</span>
          )}
        </For>
      </span>
      <span class="row-year">{props.game.year ?? ""}</span>
      <span class="row-genre" title={genreText()}>{genreText()}</span>
      <span class="row-dev" title={props.game.developer ?? ""}>{props.game.developer ?? ""}</span>
      <span class="row-pub" title={props.game.publisher ?? ""}>{props.game.publisher ?? ""}</span>
      <span class="row-rating">
        <Show when={props.game.rating != null}>★ {props.game.rating!.toFixed(1)}</Show>
      </span>
      <span class="row-size">{props.game.download_size ? formatBytes(props.game.download_size) : ""}</span>
      <span class="row-status">
        <Show when={status()}>
          <span class="card-action-label action-downloading">{status()}</span>
        </Show>
        <Show when={!status()}>
          <Show when={isDownloading()}>
            {/* The phase text, same as GameCard - a bare percentage read as a
                stuck "100%" through the whole extraction phase. */}
            <span class="card-action-label action-downloading" title={dlState()?.status}>
              {dlState()?.status}
            </span>
            <button
              class="row-cancel"
              title="Cancel download"
              onClick={(e) => { e.stopPropagation(); cancelGameDownload(dlEntry()!.id); }}
            >✕</button>
          </Show>
          <Show when={!isDownloading() && props.game.installed}>
            <span class="card-action-label action-installed">▶ Play</span>
          </Show>
          <Show when={!isDownloading() && !props.game.installed && props.game.in_library}>
            <span class="card-action-label action-incomplete">⚠ Incomplete</span>
          </Show>
          <Show when={!isDownloading() && !props.game.installed && !props.game.in_library}>
            <span class={`card-action-label ${isOffline() ? "action-offline" : "action-download"}`}>
              {isOffline() ? "Not installed" : "↓ Download"}
            </span>
          </Show>
        </Show>
      </span>

      <Show when={contextMenu()}>
        <GameActionsMenu
          game={props.game}
          x={contextMenu()!.x}
          y={contextMenu()!.y}
          downloading={isDownloading()}
          setStatus={setStatus}
          onDetail={props.onDetail}
          onClose={() => setContextMenu(null)}
        />
      </Show>
    </div>
  );
}
