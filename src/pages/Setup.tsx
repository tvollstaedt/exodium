import { createSignal, onMount, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Progress } from "@ark-ui/solid/progress";
import { WindowControls } from "../components/WindowControls";
import {
  setupFromLocal,
  validateExodosDir,
  getDefaultDataDir,
  getAvailableCollections,
  setConfig,
  initDownloadManager,
  type ExodosValidation,
} from "../api/tauri";

interface SetupProps {
  onComplete: () => void;
}

type Phase = "mode" | "scratch" | "import" | "importing";

const IconDownload = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
    <path stroke-linecap="round" stroke-linejoin="round" d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5M16.5 12L12 16.5m0 0L7.5 12m4.5 4.5V3" />
  </svg>
);

const IconImport = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
    <path stroke-linecap="round" stroke-linejoin="round" d="M3 7.5L7.5 3m0 0L12 7.5M7.5 3v13.5m13.5 0V6.75a2.25 2.25 0 00-2.25-2.25H6.75" />
  </svg>
);

const IconBack = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
    <path stroke-linecap="round" stroke-linejoin="round" d="M10.5 19.5L3 12m0 0l7.5-7.5M3 12h18" />
  </svg>
);

export function Setup(props: SetupProps) {
  const [phase, setPhase] = createSignal<Phase>("mode");
  const [error, setError] = createSignal("");

  // "scratch" phase state
  const [dataDir, setDataDir] = createSignal("");

  // "import" phase state
  const [exodosDir, setExodosDir] = createSignal("");
  const [validation, setValidation] = createSignal<ExodosValidation | null>(null);
  const [validating, setValidating] = createSignal(false);

  onMount(async () => {
    try {
      const dir = await getDefaultDataDir();
      if (dir) { setDataDir(dir); }
    } catch {}
  });

  const handleSelectDataDir = async () => {
    const selected = await open({
      title: "Select parent directory for game storage",
      directory: true,
    });
    if (selected) { setDataDir(selected as string); }
  };

  const handleSelectExodosDir = async () => {
    const selected = await open({
      title: "Select your eXoDOS folder",
      directory: true,
    });
    if (!selected) { return; }
    const path = selected as string;
    setExodosDir(path);
    setValidation(null);
    setValidating(true);
    try {
      const result = await validateExodosDir(path);
      setValidation(result);
    } catch (e) {
      setValidation({ valid: false, hint: String(e) });
    } finally {
      setValidating(false);
    }
  };

  const handleScratchContinue = async () => {
    if (!dataDir()) { return; }
    setError("");
    try {
      const available = await getAvailableCollections();
      const collectionsCSV = available.map((c) => c.id).join(",");
      await setConfig("data_dir", dataDir());
      await setConfig("collections", collectionsCSV);
      await initDownloadManager();
      props.onComplete();
    } catch (e) {
      setError(`Failed to initialize: ${e}`);
    }
  };

  const handleImport = async () => {
    if (!exodosDir() || !validation()?.valid) { return; }
    setPhase("importing");
    setError("");
    try {
      await setupFromLocal(exodosDir());
      props.onComplete();
    } catch (e) {
      setError(`Import failed: ${e}`);
      setPhase("import");
    }
  };

  const previewPath = () => {
    const dir = dataDir();
    if (!dir) { return ""; }
    const sep = dir.includes("\\") ? "\\" : "/";
    return `${dir}${sep}eXoDOS${sep}`;
  };

  return (
    <div class="setup-page" onMouseDown={(e) => {
      if ((e.target as HTMLElement).closest('.setup-card, .setup-window-controls')) { return; }
      getCurrentWindow().startDragging();
    }}>
      <div class="setup-window-controls"><WindowControls /></div>

      <div class="setup-card">
        <h2>Welcome to Exodium</h2>

        <Show when={error()}>
          <div class="error" style="margin-bottom:12px">{error()}</div>
        </Show>

        {/* ── Mode selection ── */}
        <Show when={phase() === "mode"}>
          <p class="setup-subtitle">How do you want to get started?</p>
          <div class="setup-mode-grid">
            <button class="setup-mode-btn" onClick={() => { setPhase("scratch"); setError(""); }}>
              <span class="setup-mode-icon"><IconDownload /></span>
              <span class="setup-mode-title">Start from scratch</span>
              <span class="setup-mode-desc">Download games on demand from the eXoDOS torrents</span>
            </button>
            <button class="setup-mode-btn" onClick={() => { setPhase("import"); setError(""); }}>
              <span class="setup-mode-icon"><IconImport /></span>
              <span class="setup-mode-title">Import eXoDOS Installation</span>
              <span class="setup-mode-desc">Use your existing eXoDOS collection — nothing will be modified</span>
            </button>
          </div>
        </Show>

        {/* ── Start from scratch ── */}
        <Show when={phase() === "scratch"}>
          <p class="setup-subtitle">Where should Exodium store your games?</p>
          <div class="setup-step">
            <label>Parent directory</label>
            <div class="path-picker">
              <span class="setting-value">{dataDir() || "Not selected"}</span>
              <button class="btn-small" onClick={handleSelectDataDir}>Browse</button>
            </div>
            <Show when={dataDir()}>
              <div class="setup-preview">
                Games will be stored in: <strong>{previewPath()}</strong>
              </div>
            </Show>
          </div>
          <div class="setup-actions" style="margin-top:20px">
            <div style="display:flex;gap:8px">
              <button class="btn-secondary" onClick={() => setPhase("mode")}>
                <IconBack /> Back
              </button>
              <button class="btn-primary" style="flex:1" onClick={handleScratchContinue} disabled={!dataDir()}>
                Continue
              </button>
            </div>
          </div>
        </Show>

        {/* ── Import eXoDOS ── */}
        <Show when={phase() === "import"}>
          <p class="setup-subtitle">Select your eXoDOS folder. Exodium will only read from it — your files are never modified.</p>
          <div class="setup-step">
            <label>eXoDOS folder</label>
            <div class="path-picker">
              <span class="setting-value">{exodosDir() || "Not selected"}</span>
              <button class="btn-small" onClick={handleSelectExodosDir}>Browse</button>
            </div>
            <Show when={validating()}>
              <div class="setup-validation setup-validation--checking">Checking...</div>
            </Show>
            <Show when={validation() && !validating()}>
              <div class={`setup-validation ${validation()!.valid ? "setup-validation--ok" : "setup-validation--err"}`}>
                {validation()!.valid ? "✓" : "✗"} {validation()!.hint}
              </div>
            </Show>
          </div>
          <div class="setup-actions" style="margin-top:20px">
            <div style="display:flex;gap:8px">
              <button class="btn-secondary" onClick={() => setPhase("mode")}>
                <IconBack /> Back
              </button>
              <button
                class="btn-primary"
                style="flex:1"
                onClick={handleImport}
                disabled={!validation()?.valid}
              >
                Import
              </button>
            </div>
          </div>
        </Show>

        {/* ── Importing ── */}
        <Show when={phase() === "importing"}>
          <p class="setup-subtitle">Importing from local directory...</p>
          <div class="setup-step">
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
