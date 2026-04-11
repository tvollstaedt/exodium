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
  const [deleteGameData, setDeleteGameData] = createSignal(false);

  const confirmReset = async () => {
    const doDelete = deleteGameData();
    setShowResetDialog(false);
    setDeleteGameData(false);
    await factoryReset(doDelete);
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
          <span class="top-bar-logo">Exodium</span>
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
                <button {...props()} class="icon-btn" onClick={() => setShowSettings(true)}>
                  &#9881;
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Settings</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
          </div>
          <WindowControls />
        </div>

        <Dialog.Root open={showSettings()} onOpenChange={(e) => setShowSettings(e.open)}>
          <Portal>
            <Dialog.Backdrop class="ark-dialog-backdrop" />
            <Dialog.Positioner class="ark-dialog-positioner">
              <Dialog.Content class="ark-dialog-content ark-dialog-settings">
                <Dialog.Title class="ark-dialog-title">Settings</Dialog.Title>
                <div class="settings-body">
                  <div class="setting-row">
                    <span class="setting-label">Data directory</span>
                    <span class="setting-value">{dataDir() || "Not set"}</span>
                    <button class="btn-small" onClick={handleChangeDataDir}>Change</button>
                  </div>
                  <div class="settings-divider" />
                  <div class="setting-row">
                    <span class="setting-label">Factory Reset</span>
                    <span class="setting-hint">Clears all data and returns to collection selection</span>
                    <button class="btn-danger" onClick={() => setShowResetDialog(true)}>Reset…</button>
                  </div>
                </div>
                <div class="ark-dialog-actions">
                  <Dialog.CloseTrigger class="btn-secondary">Close</Dialog.CloseTrigger>
                </div>
              </Dialog.Content>
            </Dialog.Positioner>
          </Portal>
        </Dialog.Root>

        <Dialog.Root open={showResetDialog()} onOpenChange={(e) => { setShowResetDialog(e.open); if (!e.open) { setDeleteGameData(false); } }}>
          <Portal>
            <Dialog.Backdrop class="ark-dialog-backdrop" />
            <Dialog.Positioner class="ark-dialog-positioner">
              <Dialog.Content class="ark-dialog-content">
                <Dialog.Title class="ark-dialog-title">Factory Reset</Dialog.Title>
                <Dialog.Description class="ark-dialog-desc">
                  Clears the Exodium database and all settings. Your downloaded game files stay on disk and can be re-imported later.
                </Dialog.Description>
                <label class="reset-option">
                  <input
                    type="checkbox"
                    checked={deleteGameData()}
                    onChange={(e) => setDeleteGameData(e.currentTarget.checked)}
                  />
                  <span>Also delete game data folder{dataDir() ? ` (${dataDir()})` : ""}</span>
                </label>
                <Show when={deleteGameData()}>
                  <p class="reset-warning">This will permanently delete all downloaded game files. This cannot be undone.</p>
                </Show>
                <div class="ark-dialog-actions">
                  <Dialog.CloseTrigger class="btn-secondary">Cancel</Dialog.CloseTrigger>
                  <button class="btn-danger" onClick={confirmReset}>Reset</button>
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
