import { createSignal, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { open } from "@tauri-apps/plugin-dialog";
import { Dialog } from "@ark-ui/solid/dialog";
import { Tooltip } from "@ark-ui/solid/tooltip";
import { Library } from "./pages/Library";
import { Setup } from "./pages/Setup";
import { WindowControls } from "./components/WindowControls";
import { WindowFrame } from "./components/WindowFrame";
import { SearchBar } from "./components/SearchBar";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  importGames,
  getSetupStatus,
  initDownloadManager,
  factoryReset,
  getConfig,
  setConfig,
} from "./api/tauri";
import { fetchGames } from "./stores/games";
import { loadThumbnailDir } from "./stores/thumbnails";
import "./styles/main.css";

type AppPhase = "loading" | "setup" | "ready";

function App() {
  const [phase, setPhase] = createSignal<AppPhase>("loading");
  const [showSettings, setShowSettings] = createSignal(false);
  const [dataDir, setDataDir] = createSignal("");
  const [importing, setImporting] = createSignal(false);
  const [importStatus, setImportStatus] = createSignal("");

  onMount(async () => {
    try {
      const status = await getSetupStatus();
      if (status.ready) {
        setPhase("ready");
        try {
          await initDownloadManager();
        } catch (e) {
          console.error("Failed to init download manager:", e);
        }
        const dir = await getConfig("data_dir");
        if (dir) setDataDir(dir);
        loadThumbnailDir();
      } else {
        setPhase("setup");
      }
    } catch {
      setPhase("setup");
    }
  });

  const handleSetupComplete = async () => {
    setPhase("ready");
    const dir = await getConfig("data_dir");
    if (dir) setDataDir(dir);
    loadThumbnailDir();
    fetchGames();
  };

  const handleImport = async () => {
    const selected = await open({
      title: "Select eXoDOS metadata ZIP",
      filters: [{ name: "ZIP archives", extensions: ["zip"] }],
      multiple: false,
    });
    if (!selected) return;
    setImporting(true);
    setImportStatus("Importing...");
    try {
      const count = await importGames(selected);
      setImportStatus(`${count} imported`);
      fetchGames();
    } catch (e) {
      setImportStatus(`Error: ${e}`);
    } finally {
      setImporting(false);
    }
  };

  const handleChangeDataDir = async () => {
    const selected = await open({ title: "Select new data directory", directory: true });
    if (!selected) return;
    await setConfig("data_dir", selected);
    setDataDir(selected);
    await initDownloadManager();
  };

  const [showResetDialog, setShowResetDialog] = createSignal(false);

  const confirmReset = async () => {
    setShowResetDialog(false);
    await factoryReset();
    setPhase("setup");
    setShowSettings(false);
    setDataDir("");
  };

  return (
    <>
      <WindowFrame />
      <Show when={phase() === "loading"}>
        <div class="loading">Loading...</div>
      </Show>

      <Show when={phase() === "setup"}>
        <Setup onComplete={handleSetupComplete} />
      </Show>

      <Show when={phase() === "ready"}>
        <div class="top-bar">
          <div class="drag-region" onMouseDown={() => getCurrentWindow().startDragging()} />
          <span class="top-bar-logo">Exodian</span>
          <div class="top-bar-center">
            <SearchBar />
          </div>
          <div class="top-bar-actions">
            {importStatus() && <span class="import-status">{importStatus()}</span>}
            <Tooltip.Root openDelay={400}>
              <Tooltip.Trigger asChild={(props) =>
                <button {...props()} class="icon-btn" onClick={handleImport} disabled={importing()}>
                  {importing() ? "..." : "+"}
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Import ZIP</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
            <Tooltip.Root openDelay={400}>
              <Tooltip.Trigger asChild={(props) =>
                <button {...props()} class="icon-btn" onClick={() => setShowSettings(!showSettings())}>
                  &#9881;
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Settings</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
          </div>
          <WindowControls />
        </div>
        <Show when={showSettings()}>
          <div class="settings-panel">
            <div class="setting-row">
              <span class="setting-label">Data directory:</span>
              <span class="setting-value">{dataDir() || "Not set"}</span>
              <button class="btn-small" onClick={handleChangeDataDir}>Change</button>
            </div>
            <div class="settings-divider" />
            <div class="setting-row">
              <span class="setting-label">Reset:</span>
              <button class="btn-danger" onClick={() => setShowResetDialog(true)}>Factory Reset</button>
              <span class="setting-hint">Clears all data and returns to collection selection</span>
            </div>
          </div>
        </Show>

        <Dialog.Root open={showResetDialog()} onOpenChange={(e) => setShowResetDialog(e.open)}>
          <Portal>
            <Dialog.Backdrop class="ark-dialog-backdrop" />
            <Dialog.Positioner class="ark-dialog-positioner">
              <Dialog.Content class="ark-dialog-content">
                <Dialog.Title class="ark-dialog-title">Factory Reset</Dialog.Title>
                <Dialog.Description class="ark-dialog-desc">
                  This will delete all imported game data and settings. This cannot be undone.
                </Dialog.Description>
                <div class="ark-dialog-actions">
                  <Dialog.CloseTrigger class="btn-secondary">Cancel</Dialog.CloseTrigger>
                  <button class="btn-danger" onClick={confirmReset}>Reset Everything</button>
                </div>
              </Dialog.Content>
            </Dialog.Positioner>
          </Portal>
        </Dialog.Root>

        <Library />
      </Show>
    </>
  );
}

export default App;
