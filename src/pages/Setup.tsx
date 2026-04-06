import { createSignal, onMount, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Progress } from "@ark-ui/solid/progress";
import { WindowControls } from "../components/WindowControls";
import {
  setupFromLocal,
  getDefaultDataDir,
  setConfig,
  initDownloadManager,
} from "../api/tauri";

interface SetupProps {
  onComplete: () => void;
}

export function Setup(props: SetupProps) {
  const [phase, setPhase] = createSignal<"pick_dir" | "local_import">("pick_dir");
  const [error, setError] = createSignal("");
  const [dataDir, setDataDir] = createSignal("");

  onMount(async () => {
    try {
      const dir = await getDefaultDataDir();
      if (dir) setDataDir(dir);
    } catch {}
  });

  const handleSelectDir = async () => {
    const selected = await open({
      title: "Select data directory for game downloads",
      directory: true,
    });
    if (selected) setDataDir(selected);
  };

  const handleContinue = async () => {
    if (!dataDir()) return;
    try {
      await setConfig("data_dir", dataDir());
      await setConfig("collections", "eXoDOS,eXoDOS_GLP,eXoDOS_SLP,eXoDOS_PLP");
      await initDownloadManager();
      props.onComplete();
    } catch (e) {
      setError(`Failed to initialize: ${e}`);
    }
  };

  const handleLocalImport = async () => {
    const exodosDir = await open({
      title: "Select existing eXoDOS directory",
      directory: true,
    });
    if (!exodosDir) return;

    let dir = dataDir();
    if (!dir) {
      const selected = await open({
        title: "Select data directory for game downloads",
        directory: true,
      });
      if (!selected) return;
      setDataDir(selected);
      dir = selected;
    }

    setPhase("local_import");
    setError("");
    try {
      await setupFromLocal(exodosDir, dir);
      props.onComplete();
    } catch (e) {
      setError(`Import failed: ${e}`);
      setPhase("pick_dir");
    }
  };

  return (
    <div class="setup-page" onMouseDown={(e) => {
      if ((e.target as HTMLElement).closest('.setup-card, .setup-window-controls')) return;
      getCurrentWindow().startDragging();
    }}>

      <div class="setup-window-controls"><WindowControls /></div>
      <div class="setup-card">
        <h2>Welcome to Exodian</h2>

        <Show when={error()}>
          <div class="error">{error()}</div>
        </Show>

        <Show when={phase() === "pick_dir"}>
          <div class="setup-step">
            <label>Choose where to store game downloads:</label>
            <div class="path-picker">
              <span class="setting-value">{dataDir() || "Not selected"}</span>
              <button class="btn-small" onClick={handleSelectDir}>Browse</button>
            </div>

            <div class="setup-actions">
              <button class="btn-primary" onClick={handleContinue} disabled={!dataDir()}>
                Continue
              </button>

              <div class="setup-divider"><span>or</span></div>

              <button class="btn-secondary full-width" onClick={handleLocalImport}>
                Import from existing eXoDOS directory
              </button>
              <div class="setup-hint">
                Use your own eXoDOS metadata (overrides bundled data)
              </div>
            </div>
          </div>
        </Show>

        <Show when={phase() === "local_import"}>
          <div class="setup-step">
            <label>Importing from local directory...</label>
            <Progress.Root class="ark-progress">
              <Progress.Track class="ark-progress-track">
                <Progress.Range class="ark-progress-range indeterminate" />
              </Progress.Track>
            </Progress.Root>
          </div>
        </Show>
      </div>
    </div>
  );
}
