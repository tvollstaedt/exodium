import { createSignal, createEffect, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { gameEngineInfo, getGameSettings, setGameSettings } from "../api/tauri";
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
  /** Whether ECE COULD run this game here (platform + extracted build), which
   *  is what decides whether the choice is offered. Asking what actually runs
   *  would hide the control as soon as someone picks Staging, leaving no way
   *  back to eXo's choice. */
  const [eceIsDefault, setEceIsDefault] = createSignal(false);
  /** ...and what would run it with the choice currently in the dialog, which
   *  is what the shader note has to reflect - switching the engine has to take
   *  the warning away before saving, or the two controls contradict. */
  const usesEce = () => eceIsDefault() && engine() !== "staging";

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
      await setGameSettings(
        props.gameId,
        engine() || null,
        glshader() || null,
        fullscreen() || null,
        cycles() || null,
        customConf() || null,
      );
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
              <Show when={eceIsDefault()}>
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
