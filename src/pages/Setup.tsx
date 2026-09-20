import { createSignal, onMount, Show } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { Progress } from "@ark-ui/solid/progress";
import {
  setupFromLocal,
  validateExodosDir,
  getDefaultDataDir,
  getAvailableCollections,
  setConfig,
  initDownloadManager,
  type ExodosValidation,
} from "../api/tauri";
import type { NetworkMode } from "../stores/network";
import { Button } from "../components/Button";
import { Toggle } from "../components/Toggle";
import exodiumIcon from "../assets/exodium-icon.png";

interface SetupProps {
  onComplete: () => void;
}

type Phase = "mode" | "scratch" | "import" | "network" | "importing" | "starting";

/** Which route the user took to get to the network step - it decides whether
 *  "offline" is even on the table (a from-scratch install has nothing to play
 *  without downloading it first). */
type Source = "scratch" | "import";

const IconDownload = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
    <path stroke-linecap="round" stroke-linejoin="round" d="M3 16.5v2.25A2.25 2.25 0 005.25 21h13.5A2.25 2.25 0 0021 18.75V16.5M16.5 12L12 16.5m0 0L7.5 12m4.5 4.5V3" />
  </svg>
);

const IconImport = () => (
  <svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
    <path stroke-linecap="round" stroke-linejoin="round" d="M2.25 12.75V12A2.25 2.25 0 014.5 9.75h15A2.25 2.25 0 0121.75 12v.75m-8.69-6.44l-2.12-2.12a1.5 1.5 0 00-1.061-.44H4.5A2.25 2.25 0 002.25 6v12a2.25 2.25 0 002.25 2.25h15A2.25 2.25 0 0021.75 18V9a2.25 2.25 0 00-2.25-2.25h-5.379a1.5 1.5 0 01-1.06-.44z" />
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

  // "network" phase state. The seeding box is pre-checked - sharing keeps the
  // swarm alive - but it is shown with its implications spelled out and can be
  // unchecked before setup finishes, so nobody uploads without having seen it.
  const [source, setSource] = createSignal<Source>("scratch");
  const [netMode, setNetMode] = createSignal<NetworkMode>("live");
  const [seeding, setSeeding] = createSignal(true);

  onMount(async () => {
    try {
      const dir = await getDefaultDataDir();
      if (dir) { setDataDir(dir); }
    } catch {}
  });

  const handleSelectDataDir = async () => {
    const selected = await open({
      title: "Select Exodium data folder",
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

  /** Both routes converge here: the network answers must be persisted before
   *  any backend call that might spin up a torrent session, because that is
   *  where Rust reads them (same invariant as `collections`). */
  const goToNetwork = (from: Source) => {
    setSource(from);
    setNetMode("live");
    setError("");
    setPhase("network");
  };

  const handleNetworkContinue = async () => {
    setError("");
    const offline = netMode() === "offline";
    try {
      await setConfig("network_mode", offline ? "offline" : "live");
      await setConfig("seeding_enabled", !offline && seeding() ? "1" : "0");
    } catch (e) {
      setError(`Failed to save network settings: ${e}`);
      return;
    }
    if (source() === "scratch") {
      await runScratchSetup();
    } else {
      await runImport();
    }
  };

  const runScratchSetup = async () => {
    if (!dataDir()) { return; }
    setPhase("starting");
    try {
      const available = await getAvailableCollections();
      const collectionsCSV = available.map((c) => c.id).join(",");
      await setConfig("data_dir", dataDir());
      await setConfig("collections", collectionsCSV);
      await initDownloadManager();
      props.onComplete();
    } catch (e) {
      setError(`Failed to initialize: ${e}`);
      setPhase("network");
    }
  };

  const runImport = async () => {
    if (!exodosDir() || !validation()?.valid) { return; }
    setPhase("importing");
    try {
      await setupFromLocal(exodosDir());
      // Re-initialize download managers via the standard path so DOSBox configs
      // are extracted and all collections get a robust manager setup. In
      // offline mode this only extracts the configs - no session is created.
      await initDownloadManager();
      props.onComplete();
    } catch (e) {
      setError(`Import failed: ${e}`);
      setPhase("network");
    }
  };

  const previewPath = () => {
    const dir = dataDir();
    if (!dir) { return ""; }
    const sep = dir.includes("\\") ? "\\" : "/";
    return `${dir}${sep}eXoDOS${sep}`;
  };

  return (
    <div class="setup-page">
      <div class="setup-card">
        <img src={exodiumIcon} alt="" class="setup-logo" />
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
              <span class="setup-mode-desc">Use your existing eXoDOS collection - nothing will be modified</span>
            </button>
          </div>
        </Show>

        {/* ── Start from scratch ── */}
        <Show when={phase() === "scratch"}>
          <p class="setup-subtitle">Where should Exodium keep its data?</p>
          <div class="setup-step">
            <label>Data folder</label>
            <div class="path-picker">
              <span class="setting-value">{dataDir() || "Not selected"}</span>
              <Button variant="small" onClick={handleSelectDataDir}>Browse</Button>
            </div>
            <Show when={dataDir()}>
              <div class="setup-preview">
                Exodium creates subfolders here: games go in{" "}
                <strong>{previewPath()}</strong>, covers and caches in{" "}
                <strong>content/</strong> next to it.
              </div>
            </Show>
          </div>
          <p class="setup-note">
            Games are downloaded from the eXoDOS BitTorrent network, one at a
            time, only when you ask for them.
          </p>
          <div class="setup-actions" style="margin-top:20px">
            <div style="display:flex;gap:8px">
              <Button variant="secondary" onClick={() => setPhase("mode")}>
                <IconBack /> Back
              </Button>
              <Button variant="primary" style="flex:1" onClick={() => goToNetwork("scratch")} disabled={!dataDir()}>
                Continue
              </Button>
            </div>
          </div>
        </Show>

        {/* ── Import eXoDOS ── */}
        <Show when={phase() === "import"}>
          <p class="setup-subtitle">Select your eXoDOS folder. Exodium will only read from it - your files are never modified.</p>
          <div class="setup-step">
            <label>eXoDOS folder</label>
            <div class="path-picker">
              <span class="setting-value">{exodosDir() || "Not selected"}</span>
              <Button variant="small" onClick={handleSelectExodosDir}>Browse</Button>
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
              <Button variant="secondary" onClick={() => setPhase("mode")}>
                <IconBack /> Back
              </Button>
              <Button variant="primary"
                style="flex:1"
                onClick={() => goToNetwork("import")}
                disabled={!validation()?.valid}
              >
                Continue
              </Button>
            </div>
          </div>
        </Show>

        {/* ── Network mode + seeding consent ── */}
        <Show when={phase() === "network"}>
          <p class="setup-subtitle">How should Exodium use the network?</p>

          {/* Offline only makes sense with games already on disk, so it is
              offered on the import route only. */}
          <Show when={source() === "import"}>
            <div class="setup-mode-grid">
              <button
                class={`setup-mode-btn${netMode() === "live" ? " is-selected" : ""}`}
                onClick={() => setNetMode("live")}
              >
                <span class="setup-mode-title">Online</span>
                <span class="setup-mode-desc">
                  Download games you don't have yet from the eXoDOS torrents.
                </span>
              </button>
              <button
                class={`setup-mode-btn${netMode() === "offline" ? " is-selected" : ""}`}
                onClick={() => setNetMode("offline")}
              >
                <span class="setup-mode-title">Offline</span>
                <span class="setup-mode-desc">
                  Launcher only. No torrent client is started and nothing is
                  downloaded or shared.
                </span>
              </button>
            </div>
          </Show>

          <Show when={netMode() === "live"}>
            <div style="margin-top:16px">
              <Toggle
                checked={seeding()}
                onChange={setSeeding}
                label="Share my downloads with other users (seeding)"
                hint="While Exodium runs, it uploads parts of the games you have to other users. That keeps the collection alive - but it also means you are distributing the files, which is a legal risk in some countries."
              />
            </div>
          </Show>

          <p class="setup-note">
            Both settings can be changed any time in Settings → Network.
          </p>

          <div class="setup-actions" style="margin-top:20px">
            <div style="display:flex;gap:8px">
              <Button variant="secondary" onClick={() => setPhase(source())}>
                <IconBack /> Back
              </Button>
              <Button variant="primary" style="flex:1" onClick={handleNetworkContinue}>
                {source() === "import" ? "Import" : "Continue"}
              </Button>
            </div>
          </div>
        </Show>

        {/* ── Starting (initializing session after scratch setup) ── */}
        <Show when={phase() === "starting"}>
          <p class="setup-subtitle">Setting up...</p>
          <div class="setup-step">
            <Progress.Root class="ark-progress">
              <Progress.Track class="ark-progress-track">
                <Progress.Range class="ark-progress-range indeterminate" />
              </Progress.Track>
            </Progress.Root>
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
