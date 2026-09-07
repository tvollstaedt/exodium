import { createSignal, createEffect, on, onCleanup, type Accessor } from "solid-js";
import type { Game, Win9xMultiplayerInfo, Win9xSupportStatus } from "../api/tauri";
import { win9xEngineAvailable, win9xMultiplayerInfo, getWin9xSupportStatus, getScummVmSupportStatus } from "../api/tauri";
import { installedPacks } from "../stores/contentPacks";
import { isOffline } from "../stores/network";
import { isWin9x, isScummVm } from "../launchNotes";

/** What the panel knows about a Win9x game's emulator, the shared support
 *  payload and online play. Probes on game change, re-probes the engine when
 *  a pack install lands or the payload turns ready, and polls the payload
 *  while it downloads. The payload poll also serves ScummVM games (utilSVM.zip
 *  carries eXo's builds on Windows); inert for every other collection. */
export function createWin9xStatus(game: Accessor<Game | null>, settled: Accessor<boolean>) {
  const [engineMissing, setEngineMissing] = createSignal(false);
  const [support, setSupport] = createSignal<Win9xSupportStatus | null>(null);
  const [mp, setMp] = createSignal<Win9xMultiplayerInfo | null>(null);

  const stillShown = (id: number | null | undefined) => game()?.id === id;
  const probeEngine = (g: Game) => {
    const id = g.id;
    win9xEngineAvailable(g.dosbox_variant ?? null)
      .then((ok) => { if (stillShown(id)) { setEngineMissing(!ok); } })
      .catch(() => {});
  };

  createEffect(on(() => game()?.id, () => {
    const g = game();
    setEngineMissing(false);
    setMp(null);
    if (!g || !isWin9x(g)) { return; }
    probeEngine(g);
    const id = g.id;
    if (id != null) {
      win9xMultiplayerInfo(id)
        .then((info) => { if (stillShown(id)) { setMp(info); } })
        .catch(() => {});
    }
  }));

  // A pack install landing must clear the "downloading emulator" note without
  // the panel being reopened.
  createEffect(() => {
    installedPacks();
    if (!settled()) { return; }
    const g = game();
    if (g && isWin9x(g)) { probeEngine(g); }
  });

  // The payload matters most when the game installed first and Play would
  // otherwise fail bare, so this is not gated on the engine or install state -
  // and reading the downloads store here would re-run it every second during
  // any download. "failed" is terminal until a restart re-arms the watcher.
  createEffect(() => {
    const g = game();
    const svm = isScummVm(g);
    if (!g || !(isWin9x(g) || svm) || isOffline()) { setSupport(null); return; }
    if (!settled()) { return; }
    const variant = g.dosbox_variant ?? null;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const probe = () => {
      (svm ? getScummVmSupportStatus() : getWin9xSupportStatus(variant))
        .then((s) => {
          if (cancelled) { return; }
          setSupport(s);
          if (s.phase === "failed") { return; }
          if (s.phase === "ready") {
            if (!svm && engineMissing()) { probeEngine(g); }
            return;
          }
          // A live bar for an active download; a steady "missing" only needs
          // to notice a download started elsewhere eventually.
          timer = setTimeout(probe, s.phase === "downloading" ? 3000 : 10000);
        })
        .catch(() => {});
    };
    probe();
    onCleanup(() => {
      cancelled = true;
      if (timer != null) { clearTimeout(timer); }
    });
  });

  return { engineMissing, support, mp, setMp };
}
