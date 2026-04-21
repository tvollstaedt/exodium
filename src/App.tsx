import { createSignal, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { open } from "@tauri-apps/plugin-dialog";
import { Dialog } from "@ark-ui/solid/dialog";
import { Tooltip } from "@ark-ui/solid/tooltip";
import { Library } from "./pages/Library";
import { Setup } from "./pages/Setup";
import { SearchBar } from "./components/SearchBar";
import { WelcomeModal } from "./components/WelcomeModal";
import { ContentPackSettings } from "./components/ContentPackSettings";
import { DownloadIndicator } from "./components/DownloadIndicator";
import {
  getSetupStatus,
  initDownloadManager,
  factoryReset,
  getConfig,
  setConfig,
  scanInstalledGames,
} from "./api/tauri";
import { fetchGames } from "./stores/games";
import { loadThumbnailDir } from "./stores/thumbnails";
import { refreshInstalledPacks } from "./stores/contentPacks";
import "./styles/main.css";

type AppPhase = "loading" | "setup" | "ready";

function App() {
  const [phase, setPhase] = createSignal<AppPhase>("loading");
  const [showSettings, setShowSettings] = createSignal(false);
  const [settingsTab, setSettingsTab] = createSignal<"general" | "packs">("general");
  const [showWelcomeModal, setShowWelcomeModal] = createSignal(false);
  const [dataDir, setDataDir] = createSignal("");
  const [resetError, setResetError] = createSignal("");

  // Derived: the actual game storage folder shown to the user.
  const gameFolderPath = () => {
    const dir = dataDir();
    if (!dir) return "";
    const sep = dir.includes("\\") ? "\\" : "/";
    return dir.replace(/[/\\]$/, "") + sep + "eXoDOS";
  };

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
        if (dir) { setDataDir(dir); }
        loadThumbnailDir();
        refreshInstalledPacks();
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
    if (dir) { setDataDir(dir); }
    loadThumbnailDir();
    refreshInstalledPacks();
    fetchGames();

    // Show the welcome modal if the user hasn't seen it yet.
    const welcomeSeen = await getConfig("welcome_seen");
    if (welcomeSeen !== "1") {
      setShowWelcomeModal(true);
    }
  };

  const handleChangeDataDir = async () => {
    const selected = await open({ title: "Select new data directory", directory: true });
    if (!selected) return;
    await setConfig("data_dir", selected);
    setDataDir(selected);
    await initDownloadManager();
  };

  const [scanning, setScanning] = createSignal(false);
  const [scanResult, setScanResult] = createSignal("");

  const handleRescan = async () => {
    setScanning(true);
    setScanResult("");
    try {
      const count = await scanInstalledGames();
      setScanResult(`${count} game${count !== 1 ? "s" : ""} marked as installed`);
      fetchGames();
    } catch (e) {
      setScanResult(`Error: ${e}`);
    } finally {
      setScanning(false);
    }
  };

  const [showResetDialog, setShowResetDialog] = createSignal(false);
  const [deleteGameData, setDeleteGameData] = createSignal(false);

  // Global launch-time overrides (persisted via DB config table, read by the
  // Rust launch_game command, layered as a last-wins -conf fragment).
  const [crtAuto, setCrtAuto] = createSignal(false);
  const [defaultFullscreen, setDefaultFullscreen] = createSignal(false);

  const loadGameDefaults = async () => {
    try {
      const [shader, fs] = await Promise.all([
        getConfig("global_glshader"),
        getConfig("default_fullscreen"),
      ]);
      setCrtAuto(shader == null || shader === "crt-auto");
      setDefaultFullscreen(fs === "fullscreen");
    } catch (e) {
      console.warn("[settings] failed to load game defaults:", e);
    }
  };

  const handleToggleCrtAuto = async (next: boolean) => {
    setCrtAuto(next);
    try {
      await setConfig("global_glshader", next ? "crt-auto" : "default");
    } catch (e) {
      console.error("[settings] failed to save global_glshader:", e);
      setCrtAuto(!next); // revert on failure
    }
  };

  const handleToggleFullscreen = async (next: boolean) => {
    setDefaultFullscreen(next);
    try {
      await setConfig("default_fullscreen", next ? "fullscreen" : "window");
    } catch (e) {
      console.error("[settings] failed to save default_fullscreen:", e);
      setDefaultFullscreen(!next);
    }
  };

  const confirmReset = async () => {
    const doDelete = deleteGameData();
    setShowResetDialog(false);
    setDeleteGameData(false);
    setResetError("");
    console.log("[reset] calling factoryReset, deleteGameData=", doDelete);
    try {
      await factoryReset(doDelete);
      console.log("[reset] factoryReset succeeded, switching to setup");
      setPhase("setup");
      setShowSettings(false);
      setDataDir("");
    } catch (e) {
      console.error("[reset] factoryReset failed:", e);
      setResetError(`Reset failed: ${e}`);
      setShowSettings(true);
    }
  };

  return (
    <>
      <Show when={phase() === "loading"}>
        <div class="loading">Loading...</div>
      </Show>

      <Show when={phase() === "setup"}>
        <Setup onComplete={handleSetupComplete} />
      </Show>

      <Show when={phase() === "ready"}>
        <div class="top-bar">
          <div class="top-bar-center">
            <SearchBar />
          </div>
          <div class="top-bar-actions">
            <DownloadIndicator />
            <Tooltip.Root openDelay={400}>
              <Tooltip.Trigger asChild={(props) =>
                <button {...props()} class="icon-btn" onClick={() => setShowSettings(true)}>
                  &#9881;
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Settings</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
          </div>
        </div>

        <Show when={showSettings()}>
        <Dialog.Root open={showSettings()} onOpenChange={(e) => {
          setShowSettings(e.open);
          if (e.open) { loadGameDefaults(); setSettingsTab("general"); }
        }}>
          <Portal>
            <Dialog.Backdrop class="ark-dialog-backdrop" />
            <Dialog.Positioner class="ark-dialog-positioner">
              <Dialog.Content class="ark-dialog-content ark-dialog-settings">
                <Dialog.Title class="ark-dialog-title">Settings</Dialog.Title>
                <div class="settings-tabs">
                  <button
                    class={`settings-tab ${settingsTab() === "general" ? "active" : ""}`}
                    onClick={() => setSettingsTab("general")}
                  >General</button>
                  <button
                    class={`settings-tab ${settingsTab() === "packs" ? "active" : ""}`}
                    onClick={() => setSettingsTab("packs")}
                  >Content Packs</button>
                </div>

                <div class="settings-tab-body">
                  <Show when={settingsTab() === "general"}>
                    <div class="settings-body">
                      <section class="settings-section">
                        <h3 class="settings-section-title">Library</h3>
                        <div class="setting-row">
                          <span class="setting-label">Game folder</span>
                          <span class="setting-value">{gameFolderPath() || "Not set"}</span>
                          <button class="btn-small" onClick={handleChangeDataDir}>Change</button>
                        </div>
                        <div class="setting-row">
                          <span class="setting-label">Installed games</span>
                          <span class="setting-hint">Re-scan disk to detect already-downloaded games</span>
                          <button class="btn-small" onClick={handleRescan} disabled={scanning()}>
                            {scanning() ? "Scanning…" : "Scan"}
                          </button>
                        </div>
                        <Show when={scanResult()}>
                          <div class="setting-hint" style="margin-top:4px">{scanResult()}</div>
                        </Show>
                      </section>

                      <section class="settings-section">
                        <h3 class="settings-section-title">Game Defaults</h3>
                        <p class="settings-section-hint">Applied as a last-wins DOSBox config on every launch. Overrides per-game settings without modifying eXoDOS's bundled configs.</p>
                        <label class="setting-toggle">
                          <input
                            type="checkbox"
                            checked={crtAuto()}
                            onChange={(e) => handleToggleCrtAuto(e.currentTarget.checked)}
                          />
                          <span class="setting-toggle-info">
                            <span class="setting-toggle-label">Auto CRT shaders</span>
                            <span class="setting-toggle-hint">DOSBox Staging picks a CRT shader matched to each game's video mode and your display resolution.</span>
                          </span>
                        </label>
                        <label class="setting-toggle">
                          <input
                            type="checkbox"
                            checked={defaultFullscreen()}
                            onChange={(e) => handleToggleFullscreen(e.currentTarget.checked)}
                          />
                          <span class="setting-toggle-info">
                            <span class="setting-toggle-label">Launch in fullscreen</span>
                            <span class="setting-toggle-hint">Start every game fullscreen instead of windowed. Alt+Enter still toggles at runtime.</span>
                          </span>
                        </label>
                      </section>

                      <section class="settings-section danger">
                        <h3 class="settings-section-title">Danger Zone</h3>
                        <div class="setting-row">
                          <span class="setting-label">Factory Reset</span>
                          <span class="setting-hint">Clears all data and returns to setup</span>
                          <button class="btn-danger" onClick={() => setShowResetDialog(true)}>Reset…</button>
                        </div>
                        <Show when={resetError()}>
                          <div class="error" style="margin-top:8px">{resetError()}</div>
                        </Show>
                      </section>
                    </div>
                  </Show>

                  <Show when={settingsTab() === "packs"}>
                    <div class="settings-body">
                      <ContentPackSettings />
                    </div>
                  </Show>
                </div>

                <div class="ark-dialog-actions">
                  <Dialog.CloseTrigger class="btn-secondary">Close</Dialog.CloseTrigger>
                </div>
              </Dialog.Content>
            </Dialog.Positioner>
          </Portal>
        </Dialog.Root>
        </Show>

        <Show when={showResetDialog()}>
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
                  <span>Also delete game folder{gameFolderPath() ? ` (${gameFolderPath()})` : ""}</span>
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
        </Show>

        <Library />

        <WelcomeModal
          open={showWelcomeModal()}
          onClose={() => setShowWelcomeModal(false)}
        />
      </Show>
    </>
  );
}

export default App;
