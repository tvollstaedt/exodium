import { createSignal, createEffect, Show } from "solid-js";
import { Portal } from "solid-js/web";
import type { Game } from "../api/tauri";
import { GameSettingsDialog } from "./GameSettingsDialog";
import { PlaylistMenu } from "./PlaylistMenu";
import { performReset, performUninstall } from "../util";

export interface GameActionsMenuProps {
  game: Game;
  /** Viewport position of the menu's anchor corner. */
  x: number;
  y: number;
  /** Anchor by the RIGHT edge - for a button at the end of a narrow bar, a
   *  left-anchored menu hangs off the panel. */
  rightAnchored?: boolean;
  /** Raise above the detail panel (z-index 150). The grid's own right-click
   *  menu does not need it and would then sit over unrelated overlays. */
  abovePanel?: boolean;
  /** Offer "Details…" as the first entry. Pointless inside the panel, which
   *  IS the details view. */
  onDetail?: (game: Game) => void;
  /** Uninstall and Reset report progress through this when the menu performs
   *  them itself. */
  setStatus?: (s: string) => void;
  /** Take over the confirmed action (the panel renders its own progress
   *  bar); absent, the menu runs the shared helper. */
  onReset?: (gameId: number) => void;
  onUninstall?: (gameId: number) => void;
  /** Called once the menu AND anything it opened have closed - the host can
   *  drop it from the tree then, not before. */
  onClose: () => void;
  /** After an uninstall completed - the panel refreshes its variant rows. */
  onUninstalled?: () => void | Promise<void>;
  /** Suppress Uninstall while a download is in flight: performUninstall would
   *  cancel it first, and offering both side by side is confusing. */
  downloading?: boolean;
}

/** What this menu is currently showing. "Add to playlist…" and "Game
 *  settings…" replace the list rather than closing it, so the host must keep
 *  the component mounted until everything is finished - hence `done`. */
type Phase = "menu" | "playlist" | "settings" | "done";

/** The one menu behind the grid's right-click and the panel's ⋯, so a game
 *  offers the same actions wherever it is looked at. */
export function GameActionsMenu(props: GameActionsMenuProps) {
  const [phase, setPhase] = createSignal<Phase>("menu");
  // Both destructive entries confirm in place: a menu is easy to mis-hit, and
  // uninstall keeps saves while reset throws them away.
  const [confirmReset, setConfirmReset] = createSignal(false);
  const [confirmUninstall, setConfirmUninstall] = createSignal(false);

  createEffect(() => {
    if (phase() === "done") { props.onClose(); }
  });

  const id = () => props.game.id;
  const canSettings = () => props.game.installed && id() != null;
  const canReset = () => props.game.installed && id() != null;
  const canUninstall = () =>
    !props.downloading && (props.game.installed || props.game.in_library) && id() != null;
  const canPlaylist = () => id() != null;

  const dismiss = () => {
    setConfirmReset(false);
    setConfirmUninstall(false);
    setPhase("done");
  };
  const noop = () => {};

  return (
    <>
      <Show when={phase() === "menu"}>
        <Portal>
          <div
            class={`context-backdrop${props.abovePanel ? " is-above-panel" : ""}`}
            onMouseDown={dismiss}
            onContextMenu={(e) => { e.preventDefault(); dismiss(); }}
          />
          <div
            class={`context-menu${props.rightAnchored ? " is-right-anchored" : ""}`
              + `${props.abovePanel ? " is-above-panel" : ""}`}
            style={{ left: `${props.x}px`, top: `${props.y}px` }}
          >
            <Show when={props.onDetail}>
              <button
                class="context-menu-item"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => { props.onDetail!(props.game); setPhase("done"); }}
              >Details…</button>
            </Show>

            <Show when={canPlaylist()}>
              <button
                class="context-menu-item"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => setPhase("playlist")}
              >Add to playlist…</button>
            </Show>

            <Show when={canSettings()}>
              <button
                class="context-menu-item"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => setPhase("settings")}
              >Game settings…</button>
            </Show>

            {/* Uninstall keeps saves by design (they are renamed into !save/
                and restored on reinstall), so it cannot give a clean slate -
                this can. Only while installed: it restores from the ZIP. */}
            <Show when={canReset()}>
              <button
                class="context-menu-item danger"
                title="Discard saves and every in-game change, then unpack the game again"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => {
                  if (!confirmReset()) { setConfirmReset(true); return; }
                  const gameId = id()!;
                  const run = props.onReset
                    ?? ((gid: number) => void performReset(gid, props.setStatus ?? noop, props.game.title));
                  setPhase("done");
                  run(gameId);
                }}
              >{confirmReset() ? "Discard all game data?" : "↺ Reset game data"}</button>
            </Show>

            <Show when={canUninstall()}>
              <button
                class="context-menu-item danger"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={() => {
                  if (!confirmUninstall()) { setConfirmUninstall(true); return; }
                  const gameId = id()!;
                  const run = props.onUninstall
                    ?? ((gid: number) => void performUninstall(
                      gid, props.setStatus ?? noop, props.onUninstalled, props.game.title));
                  setPhase("done");
                  run(gameId);
                }}
              >{confirmUninstall() ? "Confirm uninstall?" : "Uninstall"}</button>
            </Show>
          </div>
        </Portal>
      </Show>

      <Show when={phase() === "playlist" && id() != null}>
        <PlaylistMenu x={props.x} y={props.y} gameId={id()!} onClose={dismiss} />
      </Show>

      <Show when={phase() === "settings"}>
        <GameSettingsDialog gameId={id()} gameTitle={props.game.title} open onClose={dismiss} />
      </Show>
    </>
  );
}
