import { createSignal, onMount, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Dialog } from "@ark-ui/solid/dialog";
import { Tooltip } from "@ark-ui/solid/tooltip";
import { Toggle } from "./components/Toggle";
import { Library } from "./pages/Library";
import { Setup } from "./pages/Setup";
import { SearchBar } from "./components/SearchBar";
import { WelcomeModal } from "./components/WelcomeModal";
import { SeedingConsentDialog } from "./components/SeedingConsentDialog";
import { ActivityBadge } from "./components/ActivityBadge";
import { needsSeedingConsent, seedingOn, applySeeding, loadSeeding } from "./stores/seeding";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { ContentPackSettings } from "./components/ContentPackSettings";
import { WindowFrame } from "./components/WindowFrame";
import { ToastContainer } from "./components/ToastContainer";
import {
  getSetupStatus,
  initDownloadManager,
  factoryReset,
  getConfig,
  setConfig,
  setRateLimits,
  scanInstalledGames,
  dataDirIsEmpty,
  openLogFolder,
  pendingLayoutMigration,
  migrateLayout,
  skipLayoutMigration,
  type LayoutMigration,
  win9xNetworkStatus,
  enableWin9xNetwork,
  disableWin9xNetwork,
  type Win9xNetworkStatus,
} from "./api/tauri";
import { updateState, checkForAppUpdate, startUpdate, restartToUpdate } from "./stores/updater";
import { fetchGames } from "./stores/games";
import { applyNetworkMode, isOffline, loadNetworkMode } from "./stores/network";
import { loadThumbnailDir } from "./stores/thumbnails";
import { refreshInstalledPacks, initContentPackEvents } from "./stores/contentPacks";
import { showToast } from "./stores/toasts";
import { startTransferPolling } from "./stores/transfer";
import "./styles/main.css";
import { Button } from "./components/Button";

type AppPhase = "loading" | "setup" | "ready";

function App() {
  const [phase, setPhase] = createSignal<AppPhase>("loading");
  const [showSettings, setShowSettings] = createSignal(false);
  const [settingsTab, setSettingsTab] = createSignal<"general" | "packs">("general");
  const [showWelcomeModal, setShowWelcomeModal] = createSignal(false);
  const [showSeedingConsent, setShowSeedingConsent] = createSignal(false);
  const [dataDir, setDataDir] = createSignal("");
  const [rootFolder, setRootFolder] = createSignal("eXoDOS");
  /** Old per-collection folders waiting to be merged into the single root. */
  const [layoutMigration, setLayoutMigration] = createSignal<LayoutMigration | null>(null);
  const [migrating, setMigrating] = createSignal(false);
  /** Set once the user declined, so Settings can still offer the merge. */
  const [layoutSkipped, setLayoutSkipped] = createSignal(false);

  /** Move the old folders, then rebuild everything derived from them.
   *
   *  Blocking on purpose (see the overlay below): it renames thousands of
   *  entries and re-checks the library afterwards, and a launcher that looks
   *  idle while doing that invites a second click. */
  const [migrateStep, setMigrateStep] = createSignal("");
  const runLayoutMigration = async () => {
    setMigrating(true);
    try {
      setMigrateStep("Moving files…");
      const tally = await migrateLayout();
      setMigrateStep("Reconnecting downloads…");
      await initDownloadManager();
      setMigrateStep("Checking your library…");
      const installed = await scanInstalledGames().catch(() => 0);
      fetchGames();
      setLayoutMigration(null);
      setLayoutSkipped(false);
      const parts = [`${tally.moved} moved`];
      if (tally.deduped) { parts.push(`${tally.deduped} duplicates removed`); }
      if (tally.skipped) { parts.push(`${tally.skipped} left alone`); }
      showToast(parts.join(", "), "success", {
        detail: `${installed} game${installed !== 1 ? "s" : ""} available.`,
      });
    } catch (e) {
      showToast("Could not move the games", "error", { detail: String(e) });
    } finally {
      setMigrating(false);
      setMigrateStep("");
    }
  };
  const [resetError, setResetError] = createSignal("");
  const [logOpenError, setLogOpenError] = createSignal("");
  const [resetting, setResetting] = createSignal(false);

  // Derived: the actual game storage folder shown to the user.
  const gameFolderPath = () => {
    const dir = dataDir();
    if (!dir) return "";
    // "." means the data dir IS the game root (a pre-single-root install
    // whose folder was adopted as the root at startup).
    if (rootFolder() === ".") { return dir; }
    const sep = dir.includes("\\") ? "\\" : "/";
    return dir.replace(/[/\\]$/, "") + sep + rootFolder();
  };

  onMount(() => {
    // Suppress the webview's native right-click menu app-wide (Inspect,
    // Reload, ... don't belong in a launcher). Component-level custom menus
    // (GameCard) hook the same event and render their own UI, unaffected.
    // Kept enabled in dev so Inspect Element stays reachable.
    if (!import.meta.env.DEV) {
      const suppress = (e: MouseEvent) => {
        // Editable fields keep the native menu - it carries cut/copy/paste.
        const t = e.target as HTMLElement | null;
        if (t?.closest('input, textarea, [contenteditable="true"]')) { return; }
        e.preventDefault();
      };
      document.addEventListener("contextmenu", suppress);
      onCleanup(() => document.removeEventListener("contextmenu", suppress));
    }
  });

  onMount(async () => {
    // Linux may run WebKit's fallback renderer: lib.rs still disables the
    // DMA-BUF path on NVIDIA/X11, which is what the AppImage always is (see
    // CLAUDE.md §17). Backdrop blur is a per-frame CPU repaint there, and the
    // renderer in use is not observable from here, so main.css drops it for
    // all of Linux under this class.
    if (navigator.userAgent.includes("Linux")) {
      document.documentElement.classList.add("soft-render");
    }
    // Before anything else: the backend can start pack installs on its own
    // (Win9x emulator auto-queue), and only this listener makes them visible.
    initContentPackEvents().catch(() => {});
    try {
      const status = await getSetupStatus();
      if (status.ready) {
        setPhase("ready");
        await loadNetworkMode();
        try {
          await initDownloadManager();
        } catch (e) {
          console.error("Failed to init download manager:", e);
        }
        // After the manager exists, not before: answering the dialog applies
        // the choice to the running session, and a click landing in the gap
        // would be written to the DB but never reach librqbit.
        setShowSeedingConsent(await needsSeedingConsent());
        const dir = await getConfig("data_dir");
        if (dir) { setDataDir(dir); }
        // Read, never assumed: an imported eXo tree keeps whatever name the
        // user gave it, and a legacy install has its own folder adopted as the
        // root at startup.
        const root = await getConfig("root_folder");
        if (root) { setRootFolder(root); }
        loadThumbnailDir();
        refreshInstalledPacks();
        // No onCleanup: this runs after an await, where Solid has no owner to
        // attach it to, so it would silently never fire. The poll lives as
        // long as the app does.
        startTransferPolling();
        loadSeeding();
        // Re-check what is actually on disk. Install flags are stored per
        // game, so anything that moved, was deleted or was added behind the
        // app's back (a folder moved to another drive, a manual copy) would
        // otherwise stay wrong until the user found the button in Settings.
        // The backend refuses to run when the data dir holds no collection at
        // all, so an unmounted drive cannot wipe the library.
        scanInstalledGames().then(() => fetchGames()).catch(() => {});
        // Installs made before the single-root layout keep their games in
        // per-collection folders. Ask before touching them - it moves files.
        pendingLayoutMigration()
          .then((m) => { setLayoutMigration(m); if (m) { setLayoutSkipped(!m.prompt); } })
          .catch(() => {});
        // Update checks are network calls; offline mode means none are made.
        if (!isOffline()) {
          checkForAppUpdate();
          // Setup skips the content-pack offer while offline without marking it
          // seen, so pick it up on the first online start instead of dropping
          // it silently.
          getConfig("welcome_seen").then((seen) => {
            if (seen !== "1") { setShowWelcomeModal(true); }
          }).catch(() => {});
        }
      } else {
        setPhase("setup");
      }
    } catch {
      setPhase("setup");
    }
  });

  const handleSetupComplete = async () => {
    setPhase("ready");
    await loadNetworkMode();
    const dir = await getConfig("data_dir");
    if (dir) { setDataDir(dir); }
    loadThumbnailDir();
    refreshInstalledPacks();
    fetchGames();
    startTransferPolling();
    loadSeeding();
    if (!isOffline()) { checkForAppUpdate(); }

    // Show the welcome modal if the user hasn't seen it yet - but never in
    // offline mode: it exists to offer downloads, which the user just declined.
    // `welcome_seen` stays unwritten, and the startup path above re-offers it on
    // the first online session; the packs are in Settings either way.
    const welcomeSeen = await getConfig("welcome_seen");
    if (welcomeSeen !== "1" && !isOffline()) {
      setShowWelcomeModal(true);
    }
  };

  /** Point the app at a different game folder. */
  /** Folder the user picked but has not confirmed yet, or null. */
  const [pendingDataDir, setPendingDataDir] = createSignal<string | null>(null);

  const handleChangeDataDir = async () => {
    const selected = await open({ title: "Select new data directory", directory: true });
    if (!selected) return;
    // An EMPTY target is the signature of the misreading this setting invites:
    // Change points Exodium at a folder, it never moves anything into one. Ask
    // before leaving someone with an empty library and their games still on
    // the old disk. Only worth asking if they HAVE anything to leave behind -
    // a user with nothing downloaded is just choosing where to start.
    const [targetEmpty, currentEmpty] = await Promise.all([
      dataDirIsEmpty(selected).catch(() => false),
      dataDir() ? dataDirIsEmpty(dataDir()).catch(() => true) : Promise.resolve(true),
    ]);
    if (targetEmpty && !currentEmpty) {
      setPendingDataDir(selected);
      return;
    }
    await applyDataDir(selected);
  };

  /** Everything derived from the old location has to be rebuilt, and the
   *  install flags most of all: they are per-game rows in the database, so
   *  after a change every game still claims to live at the old path and Play
   *  fails with "not installed" until something re-checks the disk. Doing that
   *  here (and reporting the count) also answers the question the user
   *  actually has at this moment - did it find my games? */
  const applyDataDir = async (selected: string) => {
    await setConfig("data_dir", selected);
    setDataDir(selected);
    await initDownloadManager();
    loadThumbnailDir();
    refreshInstalledPacks();
    try {
      const count = await scanInstalledGames(true);
      showToast(`${count} game${count !== 1 ? "s" : ""} found in the new folder`, "success");
    } catch (e) {
      showToast("No games found in that folder", "error", { detail: String(e) });
    }
    fetchGames();
  };

  const [scanning, setScanning] = createSignal(false);
  const [scanResult, setScanResult] = createSignal("");

  const handleRescan = async () => {
    setScanning(true);
    setScanResult("");
    try {
      const count = await scanInstalledGames(true);
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
  // Initial values MUST mirror the backend defaults in launch_game (unset
  // global_glshader means crt-auto there), so the UI is truthful even
  // before loadGameDefaults resolves.
  const [crtAuto, setCrtAuto] = createSignal(true);
  const [defaultFullscreen, setDefaultFullscreen] = createSignal(false);

  // Windows 9x multiplayer needs packet-capture rights the OS withholds by
  // default. Null until the first probe answers, so the row can stay quiet
  // rather than flash a wrong state.
  const [netStatus, setNetStatus] = createSignal<Win9xNetworkStatus | null>(null);
  const [enablingNet, setEnablingNet] = createSignal(false);
  const loadWin9xNetwork = async () => {
    try { setNetStatus(await win9xNetworkStatus()); } catch { /* older backend */ }
  };
  const toggleWin9xNetwork = async (enable: boolean) => {
    setEnablingNet(true);
    try {
      setNetStatus(enable ? await enableWin9xNetwork() : await disableWin9xNetwork());
      showToast(
        enable ? "Windows 9x multiplayer enabled" : "Windows 9x multiplayer disabled",
        "success",
      );
    } catch (e) {
      const msg = String(e);
      // "cancelled" is the user dismissing the OS dialog - not a failure.
      if (!msg.includes("cancelled")) {
        showToast(enable ? "Could not enable multiplayer" : "Could not disable multiplayer",
          "error", { detail: msg });
      }
    } finally {
      setEnablingNet(false);
    }
  };

  // Kept as strings: an empty field means unlimited, which no number can say.
  const [limitDown, setLimitDown] = createSignal("");
  const [limitUp, setLimitUp] = createSignal("");
  const [limitError, setLimitError] = createSignal("");
  const loadGameDefaults = async () => {
    try {
      const [shader, fs, down, up] = await Promise.all([
        getConfig("global_glshader"),
        getConfig("default_fullscreen"),
        getConfig("rate_limit_down_kbps"),
        getConfig("rate_limit_up_kbps"),
      ]);
      // Sharing lives in its own store - the badge needs it before Settings
      // has ever been opened.
      loadSeeding();
      setLimitDown(down ?? "");
      setLimitUp(up ?? "");
      setCrtAuto(shader == null || shader === "crt-auto");
      setDefaultFullscreen(fs === "fullscreen");
    } catch (e) {
      console.warn("[settings] failed to load game defaults:", e);
    }
  };

  // Opening goes through this helper because Ark's onOpenChange only fires
  // for component-initiated changes (Escape, backdrop, close button) - not
  // when we flip the controlled `open` prop, so init logic there never ran.
  const openSettings = () => {
    loadGameDefaults();
    loadNetworkMode();
    loadWin9xNetwork();
    // Reports the folders even after a "not now", so the row below can offer
    // the merge later.
    pendingLayoutMigration()
      .then((m) => setLayoutSkipped(m != null))
      .catch(() => {});
    setLogOpenError("");
    setModeError("");
    setSettingsTab("general");
    setShowSettings(true);
  };

  const [switchingMode, setSwitchingMode] = createSignal(false);
  const [modeError, setModeError] = createSignal("");

  /** Flipping this rebuilds the torrent state: going offline drops every
   *  manager (which shuts the librqbit session down), going online creates a
   *  fresh session and re-adopts any interrupted downloads. */
  const handleToggleOnline = async (online: boolean) => {
    setModeError("");
    setSwitchingMode(true);
    try {
      const stopped = await applyNetworkMode(online ? "live" : "offline");
      // Two different fates, so they get two different sentences: torrent
      // downloads keep their file selection and pick up again, pack installs
      // are plain HTTP transfers that have to be restarted by hand.
      const notes: string[] = [];
      if (stopped.downloads > 0) {
        notes.push(`${stopped.downloads} game download${stopped.downloads === 1 ? "" : "s"} paused - resumes when you go back online`);
      }
      if (stopped.packs > 0) {
        notes.push(`${stopped.packs} content pack download${stopped.packs === 1 ? "" : "s"} cancelled`);
      }
      showToast(
        online ? "Online mode - downloads enabled" : "Offline mode - torrent client stopped",
        "info",
        notes.length > 0 ? { detail: `${notes.join("; ")}.` } : {},
      );
      // Offline installs are never asked about seeding, so going online is
      // where an old install finally owes the answer.
      if (online) { setShowSeedingConsent(await needsSeedingConsent()); }
    } catch (e) {
      setModeError(`Could not switch mode: ${e}`);
    } finally {
      setSwitchingMode(false);
    }
  };

  /** The answer from the one-time consent dialog. Errors propagate so the
   *  dialog can stay open and say so - a failed write here would otherwise
   *  leave the key unset and ask again on the next start. */
  const handleSeedingConsent = async (enabled: boolean) => {
    await applySeeding(enabled);
    setShowSeedingConsent(false);
    showToast(
      enabled ? "Sharing with other players is on" : "Sharing with other players is off",
      "info",
      { detail: "Change it any time in Settings → Network." },
    );
  };

  /** Saves on blur rather than per keystroke: applying a limit mid-typing
   *  would throttle to "5" on the way to "500". */
  const handleSaveLimits = async () => {
    setLimitError("");
    // Clamped to u32: the command's parameter type is u32, and anything larger
    // fails deserialization with an error about integers rather than about
    // speed limits.
    const MAX_KBPS = 4_000_000;
    const parse = (raw: string): number | null => {
      const v = parseInt(raw, 10);
      if (!Number.isFinite(v) || v <= 0) { return null; }
      return Math.min(v, MAX_KBPS);
    };
    const up = parse(limitUp());
    const down = parse(limitDown());
    // Normalise the fields to what was actually stored, so "0" or "abc"
    // visibly becomes unlimited instead of lingering as a rejected value.
    setLimitUp(up === null ? "" : String(up));
    setLimitDown(down === null ? "" : String(down));
    try {
      await setRateLimits(up, down);
    } catch (e) {
      setLimitError(`Could not apply the limits: ${e}`);
    }
  };

  const handleToggleSeeding = async (next: boolean) => {
    try {
      await applySeeding(next);
    } catch (e) {
      console.error("[settings] failed to save seeding preference:", e);
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

  const handleOpenLogFolder = async () => {
    setLogOpenError("");
    try {
      await openLogFolder();
    } catch (e) {
      setLogOpenError(`Could not open log folder: ${e}`);
    }
  };

  const confirmReset = async () => {
    const doDelete = deleteGameData();
    setShowResetDialog(false);
    setDeleteGameData(false);
    setResetError("");
    // Block the UI immediately so the user doesn't see a stale Library frame
    // while the reset (which may take seconds - DB clear + recursive deletes
    // for game folders + content packs) runs to completion. Closing the
    // settings dialog FIRST then setting `resetting()` puts the overlay over
    // whatever was behind the dialog (Library or Setup).
    setShowSettings(false);
    setResetting(true);
    console.log("[reset] calling factoryReset, deleteGameData=", doDelete);
    try {
      await factoryReset(doDelete);
      console.log("[reset] factoryReset succeeded, switching to setup");
      setPhase("setup");
      setDataDir("");
    } catch (e) {
      console.error("[reset] factoryReset failed:", e);
      setResetError(`Reset failed: ${e}`);
      setShowSettings(true);
    } finally {
      setResetting(false);
    }
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
          <div class="top-bar-center">
            <SearchBar />
          </div>
          <div class="top-bar-actions">
            <Show when={updateState()}>
              <button
                class={`update-pill update-pill-${updateState()!.status}`}
                disabled={updateState()!.status === "downloading"}
                title={
                  updateState()!.status === "available"
                    ? `Download and install Exodium ${updateState()!.version}`
                    : updateState()!.status === "ready"
                      ? "Restart Exodium to finish updating"
                      : "Downloading update…"
                }
                onClick={() =>
                  updateState()!.status === "available" ? startUpdate()
                  : updateState()!.status === "ready" ? restartToUpdate()
                  : undefined
                }
              >
                {updateState()!.status === "available" && `⬆ Update ${updateState()!.version}`}
                {updateState()!.status === "downloading" && "Downloading…"}
                {updateState()!.status === "ready" && "↻ Restart to update"}
              </button>
            </Show>
            {/* Offline is a mode with visible consequences (no downloads, no
                videos, no sharing), so it says so permanently rather than only
                inside Settings. */}
            <ActivityBadge onOpenSettings={openSettings} />
            <Tooltip.Root openDelay={400}>
              <Tooltip.Trigger asChild={(props) =>
                <button {...props()} class="icon-btn icon-btn-heart" onClick={() => openUrl("https://ko-fi.com/tvollstaedt")}>
                  &#9829;
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Support Exodium</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
            <Tooltip.Root openDelay={400}>
              <Tooltip.Trigger asChild={(props) =>
                <button {...props()} class="icon-btn" onClick={openSettings}>
                  &#9881;
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Settings</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
          </div>
        </div>

        <Show when={showSettings()}>
        <Dialog.Root open={showSettings()} onOpenChange={(e) => setShowSettings(e.open)}>
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
                        {/* "Change" is a POINTER, not a move - and nothing on
                            the row said so, which invites the reading that it
                            relocates a 282 GB library. */}
                        <div class="setting-row">
                          <span class="setting-label">Game folder</span>
                          <span class="setting-value">{gameFolderPath() || "Not set"}</span>
                          <Button variant="small" onClick={handleChangeDataDir}>Change…</Button>
                        </div>
                        <div class="setting-row setting-row-note">
                          <span class="setting-hint">
                            Points Exodium at an existing folder - your downloaded games are not moved.
                          </span>
                        </div>
                        <div class="setting-row">
                          <span class="setting-label">Installed games</span>
                          <span class="setting-hint">Re-scan disk to detect already-downloaded games</span>
                          <Button variant="small" onClick={handleRescan} disabled={scanning()}>
                            {scanning() ? "Scanning…" : "Scan"}
                          </Button>
                        </div>
                        <Show when={scanResult()}>
                          <div class="setting-hint" style="margin-top:4px">{scanResult()}</div>
                        </Show>
                        {/* The way back after declining the merge at startup -
                            without it, "not now" would mean "never". */}
                        <Show when={layoutSkipped()}>
                          <div class="setting-row">
                            <span class="setting-label">Folder layout</span>
                            <span class="setting-hint">
                              Windows games sit outside the folder Exodium reads
                            </span>
                            <Button
                              variant="small"
                              loading={migrating()}
                              loadingLabel="Moving…"
                              onClick={() => void runLayoutMigration()}
                            >
                              Merge
                            </Button>
                          </div>
                        </Show>
                      </section>

                      <section class="settings-section">
                        <h3 class="settings-section-title">Game Defaults</h3>
                        <p class="settings-section-hint">Applied as a last-wins DOSBox config on every launch. Overrides per-game settings without modifying eXoDOS's bundled configs.</p>
                        <Toggle
                          checked={crtAuto()}
                          onChange={handleToggleCrtAuto}
                          label="Auto CRT shaders"
                          hint="DOSBox Staging picks a CRT shader matched to each game's video mode and your display resolution. Games that run under DOSBox ECE (Windows only) have no shader support and are unaffected."
                        />
                        <Toggle
                          checked={defaultFullscreen()}
                          onChange={handleToggleFullscreen}
                          label="Launch in fullscreen"
                          hint="Start every game fullscreen instead of windowed. Alt+Enter still toggles at runtime."
                        />
                      </section>

                      <section class="settings-section">
                        <h3 class="settings-section-title">Network</h3>
                        <p class="settings-section-hint">Games are downloaded from the eXoDOS BitTorrent swarm.</p>
                        {/* A switch, not a checkbox: this one starts and stops
                            a network service, which is a mode rather than an
                            option among several. */}
                        <Toggle
                          checked={!isOffline()}
                          disabled={switchingMode()}
                          onChange={handleToggleOnline}
                          label={isOffline() ? "Offline mode" : "Online mode"}
                          hint={isOffline()
                            ? "The torrent client stays off - Exodium only launches games already on disk."
                            : "Games, previews and content packs are downloaded from the eXoDOS torrents."}
                        />
                        <Show when={modeError()}>
                          <div class="setting-hint" style="margin-top:4px">{modeError()}</div>
                        </Show>
                        {/* Kept visible but inert while offline: hiding it
                            would look like the setting disappeared, and its
                            state still matters for when you go back online. */}
                        <Toggle
                          checked={seedingOn() && !isOffline()}
                          disabled={isOffline()}
                          onChange={handleToggleSeeding}
                          label="Share with other players (seeding)"
                          hint={isOffline()
                            ? "Nothing is shared while offline. Your choice is kept for when you switch back."
                            : "Uploads parts of the games you have to other users while Exodium runs. Keeps the collection alive - but distributing game files carries legal risk in some countries. Off caps upload at 1 KB/s."}
                        />

                        {/* Windows 9x multiplayer. Separate from the torrent
                            settings above: this is about the emulated PC's
                            network card, and the grant is a system-wide one,
                            so the row says what it costs before asking. */}
                        <Show when={netStatus()}>
                          {(st) => (
                            <div class="setting-card">
                              <div class="setting-card-info">
                                <span class="setting-toggle-label">Windows 9x multiplayer</span>
                                <span class="setting-toggle-hint">{st().detail}</span>
                              </div>
                              <Show when={st().can_enable || st().enabled}>
                                <Button
                                  variant="small"
                                  loading={enablingNet()}
                                  loadingLabel="Waiting…"
                                  onClick={() => toggleWin9xNetwork(!st().enabled)}
                                >
                                  {st().enabled ? "Remove…" : "Enable…"}
                                </Button>
                              </Show>
                              <Show when={st().manual_hint}>
                                <code class="setting-code">{st().manual_hint}</code>
                              </Show>
                            </div>
                          )}
                        </Show>

                        {/* Caps apply to the whole session, both directions.
                            Empty means unlimited, which is what a torrent
                            client does by default. */}
                        <div class="setting-row setting-row--limits">
                          <span class="setting-label">Speed limits</span>
                          <div class="limit-inputs">
                            <label class="limit-field">
                              <span>Down</span>
                              <input
                                type="number"
                                min="1"
                                placeholder="∞"
                                disabled={isOffline()}
                                value={limitDown()}
                                onInput={(e) => setLimitDown(e.currentTarget.value)}
                                onChange={handleSaveLimits}
                              />
                              <span>KB/s</span>
                            </label>
                            <label class="limit-field">
                              <span>Up</span>
                              <input
                                type="number"
                                min="1"
                                placeholder="∞"
                                disabled={isOffline() || !seedingOn()}
                                value={limitUp()}
                                onInput={(e) => setLimitUp(e.currentTarget.value)}
                                onChange={handleSaveLimits}
                              />
                              <span>KB/s</span>
                            </label>
                          </div>
                        </div>
                        <Show when={limitError()}>
                          <div class="error" style="margin-top:6px">{limitError()}</div>
                        </Show>
                      </section>

                      <section class="settings-section">
                        <h3 class="settings-section-title">Diagnostics</h3>
                        <p class="settings-section-hint">If a download stalls or the app misbehaves, share <code>exodium.log</code> from the folder.</p>
                        <div class="setting-row">
                          <span class="setting-label">Log folder</span>
                          <span class="setting-hint">Open in your file explorer</span>
                          <Button variant="small" onClick={handleOpenLogFolder}>Open</Button>
                        </div>
                        <Show when={logOpenError()}>
                          <div class="error" style="margin-top:6px">{logOpenError()}</div>
                        </Show>
                      </section>

                      <section class="settings-section">
                        <h3 class="settings-section-title">Support Exodium</h3>
                        <p class="settings-section-hint">Exodium is free and open source. If it's useful to you, you can support its development.</p>
                        <div class="setting-row">
                          <span class="setting-label">Ko-fi</span>
                          <span class="setting-hint">One-time donation, no account needed</span>
                          <Button variant="small" onClick={() => openUrl("https://ko-fi.com/tvollstaedt")}>Open</Button>
                        </div>
                        <div class="setting-row">
                          <span class="setting-label">GitHub Sponsors</span>
                          <span class="setting-hint">One-time or monthly via GitHub</span>
                          <Button variant="small" onClick={() => openUrl("https://github.com/sponsors/tvollstaedt")}>Open</Button>
                        </div>
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
                  <Button variant="danger" onClick={confirmReset}>Reset</Button>
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

        {/* No cancel and no backdrop dismiss: half a move is the one state
            worth avoiding, so the app stays busy until it is done. */}
        <Show when={migrating()}>
          <Portal>
            <div class="ark-dialog-backdrop" />
            <div class="ark-dialog-positioner">
              <div class="ark-dialog-content playlist-dialog">
                <h2 class="ark-dialog-title">Migrating your games</h2>
                <p class="ark-dialog-desc">
                  Files are moved, not copied - this should only take a moment.
                </p>
                <div class="dialog-progress">
                  <span class="btn-spinner" />
                  <span>{migrateStep()}</span>
                </div>
              </div>
            </div>
          </Portal>
        </Show>

        {/* Layout merge: eXo ships one folder per pack but expects them
            merged, and Exodium now writes that same single tree. Older
            installs are asked once, because this moves their game files. */}
        <ConfirmDialog
          open={layoutMigration()?.prompt === true}
          title="Migrate your Windows games?"
          message="Exodium used to put your Windows games beside the DOS folder. eXoDOS has one root folder for all collections, which is now Exodium's default as well. Moving your games is instant, nothing is lost. Move now or do it later in Settings → Library."
          confirmLabel={migrating() ? "Moving…" : "Move now"}
          cancelLabel="Not now"
          onConfirm={() => void runLayoutMigration()}
          onClose={(declined) => {
            setLayoutMigration(null);
            // Declining is remembered rather than asked again at every start:
            // the merge stays available under Settings → Library, so nothing
            // is lost by taking the offer away here.
            if (declined) { void skipLayoutMigration().then(() => setLayoutSkipped(true)); }
          }}
        />
        {/* Change points at a folder, it never moves one - so the case worth
            catching is an empty target chosen by someone who meant to
            relocate. Naming the old path is the point: it says where the games
            actually stay. */}
        <ConfirmDialog
          open={pendingDataDir() !== null}
          title="That folder is empty"
          message={`Exodium will look for games in ${pendingDataDir() ?? ""}, but it does not move anything there. Your downloaded games stay in ${dataDir()} and keep using that space. Use the empty folder anyway?`}
          confirmLabel="Use it anyway"
          cancelLabel="Cancel"
          onConfirm={() => {
            const dir = pendingDataDir();
            setPendingDataDir(null);
            if (dir) { void applyDataDir(dir); }
          }}
          onClose={() => setPendingDataDir(null)}
        />
        <SeedingConsentDialog
          open={showSeedingConsent()}
          onDecide={handleSeedingConsent}
        />
      </Show>

      <ToastContainer />

      <Show when={resetting()}>
        <div class="reset-overlay">
          <div class="reset-overlay-card">
            <div class="reset-overlay-spinner" />
            <div class="reset-overlay-title">Resetting Exodium…</div>
            <div class="reset-overlay-hint">Clearing library, downloads and settings. This may take a few seconds.</div>
          </div>
        </div>
      </Show>
    </>
  );
}

export default App;
