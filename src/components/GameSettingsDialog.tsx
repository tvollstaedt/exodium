import { createSignal, createEffect, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { getGameSettings, setGameSettings } from "../api/tauri";

interface GameSettingsDialogProps {
  gameId: number | null;
  gameTitle: string;
  open: boolean;
  onClose: () => void;
}

export function GameSettingsDialog(props: GameSettingsDialogProps) {
  const [glshader, setGlshader] = createSignal<string>("");
  const [fullscreen, setFullscreen] = createSignal<string>("");
  const [cycles, setCycles] = createSignal<string>("");
  const [customConf, setCustomConf] = createSignal<string>("");
  const [saving, setSaving] = createSignal(false);

  createEffect(() => {
    if (!props.open || props.gameId == null) { return; }
    const id = props.gameId;
    getGameSettings(id).then((s) => {
      if (props.gameId !== id) { return; }
      setGlshader(s.glshader ?? "");
      setFullscreen(s.fullscreen ?? "");
      setCycles(s.cycles ?? "");
      setCustomConf(s.custom_conf ?? "");
    }).catch(() => {});
  });

  const handleSave = async () => {
    if (props.gameId == null) { return; }
    setSaving(true);
    try {
      await setGameSettings(
        props.gameId,
        glshader() || null,
        fullscreen() || null,
        cycles() || null,
        customConf() || null,
      );
      props.onClose();
    } catch (e) {
      console.error("Failed to save game settings:", e);
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
              <div class="game-settings-row">
                <label class="game-settings-label">CRT Shader</label>
                <select
                  class="game-settings-select"
                  value={glshader()}
                  onChange={(e) => setGlshader(e.currentTarget.value)}
                >
                  <option value="">Default (global)</option>
                  <option value="crt-auto">On</option>
                  <option value="sharp">Off</option>
                </select>
              </div>

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
              <button class="btn-secondary" onClick={props.onClose}>Cancel</button>
              <button class="btn-primary" onClick={handleSave} disabled={saving()}>
                {saving() ? "Saving…" : "Save"}
              </button>
            </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
    </Show>
  );
}
