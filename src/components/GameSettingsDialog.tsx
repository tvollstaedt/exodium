import { createSignal, createEffect, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { gameEngineInfo, getGameSettings, setGameSettings, scummvmVariants, setScummvmOptions, type ScummVmVariants } from "../api/tauri";
import { Button } from "./Button";

interface GameSettingsDialogProps {
  gameId: number | null;
  gameTitle: string;
  open: boolean;
  onClose: () => void;
}

export function GameSettingsDialog(props: GameSettingsDialogProps) {
  const [engine, setEngine] = createSignal<string>("");
  const [glshader, setGlshader] = createSignal<string>("");
  const [fullscreen, setFullscreen] = createSignal<string>("");
  const [cycles, setCycles] = createSignal<string>("");
  const [customConf, setCustomConf] = createSignal<string>("");
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string>("");
  /** ECE could run this game here (decides whether the choice is offered;
   *  the override is ignored so Staging does not hide the way back). */
  const [eceIsDefault, setEceIsDefault] = createSignal(false);
  /** ...and what would run it with the choice currently in the dialog, which
   *  is what the shader note has to reflect - switching the engine has to take
   *  the warning away before saving, or the two controls contradict. */
  const usesEce = () => eceIsDefault() && engine() !== "staging";
  /** Non-null for an installed eXoScummVM game: the dialog then shows the
   *  variant tree's menus instead of the DOSBox controls. */
  const [svm, setSvm] = createSignal<ScummVmVariants | null>(null);
  const [svmSub, setSvmSub] = createSignal("");
  const [svmSound, setSvmSound] = createSignal("");
  const [svmSubtitles, setSvmSubtitles] = createSignal(false);
  const [svmAspect, setSvmAspect] = createSignal(true);
  const svmVariant = () => svm()?.variants.find((v) => v.name === svm()?.selected.variant) ?? null;
  const svmSubs = () => svmVariant()?.subs ?? [];
  const svmSounds = () => {
    const v = svmVariant();
    if (!v) { return []; }
    const sub = v.subs.find((s) => s.name === svmSub());
    return sub && sub.sounds.length > 0 ? sub.sounds : v.sounds;
  };
  const svmHasSubtitles = () => {
    const v = svmVariant();
    if (!v) { return false; }
    const sub = v.subs.find((s) => s.name === svmSub());
    return (sub?.has_subtitles ?? false) || v.has_subtitles;
  };

  createEffect(() => {
    if (!props.open || props.gameId == null) { return; }
    const id = props.gameId;
    // Reset synchronously before the async load resolves - signals persist
    // across opens, so without this the previous game's values are visible
    // (and saveable onto the wrong game) until the fetch lands.
    setEngine("");
    setGlshader("");
    setFullscreen("");
    setCycles("");
    setCustomConf("");
    setSaveError("");
    setEceIsDefault(false);
    setSvm(null);
    scummvmVariants(id).then((v) => {
      if (props.gameId !== id || !v) { return; }
      setSvm(v);
      setSvmSub(v.selected.sub ?? "");
      setSvmSound(v.selected.sound ?? "");
      setSvmSubtitles(v.selected.subtitles);
      setSvmAspect(v.selected.aspect);
    }).catch(() => {});
    gameEngineInfo(id).then((info) => {
      if (props.gameId === id) { setEceIsDefault(info.ece_available); }
    }).catch(() => {});
    getGameSettings(id).then((s) => {
      if (props.gameId !== id) { return; }
      setEngine(s.engine ?? "");
      setGlshader(s.glshader ?? "");
      setFullscreen(s.fullscreen ?? "");
      setCycles(s.cycles ?? "");
      setCustomConf(s.custom_conf ?? "");
    }).catch(() => {});
  });

  const handleSave = async () => {
    if (props.gameId == null) { return; }
    setSaving(true);
    setSaveError("");
    try {
      const tree = svm();
      if (tree) {
        await setScummvmOptions(props.gameId, {
          variant: tree.selected.variant,
          sub: svmSub() || null,
          sound: svmSound() || null,
          subtitles: svmSubtitles(),
          aspect: svmAspect(),
        });
        await setGameSettings(props.gameId, null, null, fullscreen() || null, null, null);
      } else {
        await setGameSettings(
          props.gameId,
          engine() || null,
          glshader() || null,
          fullscreen() || null,
          cycles() || null,
          customConf() || null,
        );
      }
      props.onClose();
    } catch (e) {
      console.error("Failed to save game settings:", e);
      setSaveError(`Failed to save: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Show when={props.open}>
    <Dialog.Root
      open={props.open}
      onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}
    >
      <Portal>
        <Dialog.Backdrop class="game-settings-backdrop" />
        <Dialog.Positioner class="game-settings-positioner">
          <Dialog.Content class="game-settings-content">
            <Dialog.Title class="game-settings-title">
              Game Settings: {props.gameTitle}
            </Dialog.Title>

            <div class="game-settings-body">
              <Show when={svm()}>
                <Show when={svmSubs().length > 0}>
                  <div class="game-settings-row">
                    <label class="game-settings-label">Edition</label>
                    <select
                      class="game-settings-select"
                      value={svmSub()}
                      onChange={(e) => setSvmSub(e.currentTarget.value)}
                    >
                      <For each={svmSubs()}>{(s) => <option value={s.name}>{s.name}</option>}</For>
                    </select>
                  </div>
                </Show>
                <Show when={svmSounds().length > 0}>
                  <div class="game-settings-row">
                    <label class="game-settings-label">Sound</label>
                    <select
                      class="game-settings-select"
                      value={svmSound()}
                      onChange={(e) => setSvmSound(e.currentTarget.value)}
                    >
                      <For each={svmSounds()}>{(s) => <option value={s}>{s}</option>}</For>
                    </select>
                  </div>
                </Show>
                <Show when={svmHasSubtitles()}>
                  <div class="game-settings-row">
                    <label class="game-settings-label">Subtitles</label>
                    <select
                      class="game-settings-select"
                      value={svmSubtitles() ? "true" : "false"}
                      onChange={(e) => setSvmSubtitles(e.currentTarget.value === "true")}
                    >
                      <option value="false">Off</option>
                      <option value="true">On</option>
                    </select>
                  </div>
                </Show>
                <div class="game-settings-row">
                  <label class="game-settings-label">Aspect ratio</label>
                  <select
                    class="game-settings-select"
                    value={svmAspect() ? "true" : "false"}
                    onChange={(e) => setSvmAspect(e.currentTarget.value === "true")}
                  >
                    <option value="true">Corrected (4:3)</option>
                    <option value="false">Pixel-exact</option>
                  </select>
                </div>
                <p class="game-settings-note">
                  The version itself is picked in the game's panel; these are
                  the menus eXo would ask about at launch.
                </p>
              </Show>

              <Show when={!svm() && eceIsDefault()}>
                <div class="game-settings-row">
                  <label class="game-settings-label">Emulator</label>
                  <select
                    class="game-settings-select"
                    value={engine()}
                    onChange={(e) => setEngine(e.currentTarget.value)}
                  >
                    <option value="">eXo's choice (DOSBox ECE)</option>
                    <option value="staging">DOSBox Staging</option>
                  </select>
                </div>
                <p class="game-settings-note">
                  eXo tuned this game for DOSBox ECE. Staging adds shaders and
                  the newer feature set, but the game was not tested with it -
                  and for the handful of games that print, ECE is the only
                  engine that can.
                </p>
              </Show>

              <Show when={!svm()}>
              <div class="game-settings-row">
                <label class="game-settings-label">CRT Shader</label>
                <select
                  class="game-settings-select"
                  value={glshader()}
                  disabled={usesEce()}
                  onChange={(e) => setGlshader(e.currentTarget.value)}
                >
                  <option value="">Default (global)</option>
                  <option value="crt-auto">On</option>
                  <option value="sharp">Off</option>
                </select>
              </div>
              <Show when={usesEce()}>
                <p class="game-settings-note">
                  This game runs under DOSBox ECE, which has no shader support.
                  Shaders are a DOSBox Staging feature, so neither this setting
                  nor the global one applies. Switch the emulator above to
                  DOSBox Staging if you want the CRT look.
                </p>
              </Show>
              </Show>

              <div class="game-settings-row">
                <label class="game-settings-label">Fullscreen</label>
                <select
                  class="game-settings-select"
                  value={fullscreen()}
                  onChange={(e) => setFullscreen(e.currentTarget.value)}
                >
                  <option value="">Default (global)</option>
                  <option value="true">On</option>
                  <option value="false">Off</option>
                </select>
              </div>

              <Show when={!svm()}>
              <div class="game-settings-row">
                <label class="game-settings-label">CPU Cycles</label>
                <div class="game-settings-cycles">
                  <select
                    class="game-settings-select"
                    value={cycles().match(/^\d+$/) ? "fixed" : cycles()}
                    onChange={(e) => {
                      const v = e.currentTarget.value;
                      setCycles(v === "fixed" ? "10000" : v);
                    }}
                  >
                    <option value="">Default (game's own)</option>
                    <option value="auto">Auto</option>
                    <option value="max">Max</option>
                    <option value="fixed">Fixed</option>
                  </select>
                  <Show when={cycles().match(/^\d+$/) || cycles() === "fixed"}>
                    <input
                      type="number"
                      class="game-settings-cycles-input"
                      value={cycles().match(/^\d+$/) ? cycles() : "10000"}
                      onInput={(e) => setCycles(e.currentTarget.value)}
                      min="100"
                      max="100000"
                      step="500"
                    />
                  </Show>
                </div>
              </div>

              <div class="game-settings-custom">
                <label class="game-settings-label">Custom DOSBox Config</label>
                <textarea
                  class="game-settings-textarea"
                  value={customConf()}
                  onInput={(e) => setCustomConf(e.currentTarget.value)}
                  placeholder={"[cpu]\ncycles = max\n\n[sblaster]\nsbtype = sb16"}
                  spellcheck={false}
                />
              </div>
              </Show>
            </div>

            <div class="game-settings-actions">
              <Show when={saveError()}>
                <span class="game-settings-error">{saveError()}</span>
              </Show>
              <Button variant="secondary" onClick={props.onClose}>Cancel</Button>
              <Button variant="primary" loading={saving()} loadingLabel="Saving…" onClick={handleSave}>
                Save
              </Button>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
    </Show>
  );
}
