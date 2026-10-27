import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { Lock } from "lucide-solid";
import { kidsGames, launchGame, type Game } from "../api/tauri";
import { lastGameLibraryChange } from "../stores/games";
import { showToast } from "../stores/toasts";
import { createCover } from "./cover";
import { KidsPinDialog } from "./KidsPinDialog";

function KidsCard(props: { game: Game; starting: boolean; onPlay: () => void }) {
  let ref: HTMLButtonElement | undefined;
  const cover = createCover(() => props.game, () => ref);
  return (
    <button ref={ref} class={`kids-card${props.starting ? " is-starting" : ""}`} data-testid="kids-card" onClick={props.onPlay}>
      <span class="kids-card-art">
        <Show when={cover.src()}>
          <img src={cover.src()!} alt="" onError={cover.onError} />
        </Show>
      </span>
      <span class="kids-card-title">{props.game.title}</span>
    </button>
  );
}

/** What a locked Exodium shows (§26): the kids playlist's installed games,
 *  a click starts one, and Unlock is the only way anywhere else. */
export function KidsScreen(props: { onUnlocked: () => void }) {
  const [games, setGames] = createSignal<Game[]>([]);
  const [loaded, setLoaded] = createSignal(false);
  const [askPin, setAskPin] = createSignal(false);
  const [starting, setStarting] = createSignal<number | null>(null);

  const load = async () => {
    try {
      setGames(await kidsGames());
    } catch (e) {
      console.warn("[kids] games unavailable:", e);
    } finally {
      setLoaded(true);
    }
  };
  onMount(() => {
    void load();
    // The order is by last play: the game just quit moves to the front.
    const unlisten = listen("game-exited", () => void load());
    onCleanup(() => { void unlisten.then((stop) => stop()); });
  });
  // A download a parent started finishes while the screen is locked.
  createEffect(on(lastGameLibraryChange, () => void load(), { defer: true }));

  const play = async (game: Game) => {
    if (game.id == null || starting() != null) { return; }
    setStarting(game.id);
    try {
      await launchGame(game.id);
    } catch (e) {
      showToast(`Couldn't start ${game.title}`, "error", { detail: String(e) });
    } finally {
      setStarting(null);
    }
  };

  return (
    <div class="kids-screen" data-testid="kids-screen">
      <div class="kids-header">
        <span class="kids-heading">Pick a game</span>
        <button class="kids-unlock" data-testid="kids-unlock" onClick={() => setAskPin(true)}>
          <Lock size={14} aria-hidden="true" /> Unlock
        </button>
      </div>
      <Show when={loaded() && games().length === 0}>
        <div class="kids-empty" data-testid="kids-empty">No games yet - ask an adult to add some.</div>
      </Show>
      <div class="kids-grid">
        <For each={games()}>
          {(game) => <KidsCard game={game} starting={starting() === game.id} onPlay={() => void play(game)} />}
        </For>
      </div>
      <KidsPinDialog open={askPin()} onClose={() => setAskPin(false)} onUnlocked={() => { setAskPin(false); props.onUnlocked(); }} />
    </div>
  );
}
