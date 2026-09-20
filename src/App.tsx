import { createSignal, onMount, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Dialog } from "@ark-ui/solid/dialog";
import { Tooltip } from "@ark-ui/solid/tooltip";
import { Library } from "./pages/Library";
import { Setup } from "./pages/Setup";
import { SearchBar } from "./components/SearchBar";
import { WelcomeModal } from "./components/WelcomeModal";
import { SeedingConsentDialog } from "./components/SeedingConsentDialog";
import { ActivityBadge } from "./components/ActivityBadge";
import { needsSeedingConsent, applySeeding, loadSeeding } from "./stores/seeding";
import { resumeDownloads, initDependencyDownloads } from "./stores/downloads";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { SettingsDialog, type SettingsSection } from "./components/SettingsDialog";
import { resetStorageCache } from "./components/StorageTab";
import { WindowFrame } from "./components/WindowFrame";
import { ToastContainer } from "./components/ToastContainer";
import {
  getSetupStatus,
  initDownloadManager,
  factoryReset,
  getConfig,
  setConfig,
  scanInstalledGames,
  dataDirIsEmpty,
  pendingLayoutMigration,
  migrateLayout,
  skipLayoutMigration,
  type LayoutMigration,
} from "./api/tauri";
import { updateState, checkForAppUpdate, startUpdate, restartToUpdate } from "./stores/updater";
import { fetchGames } from "./stores/games";
import { isOffline, loadNetworkMode } from "./stores/network";
import { loadThumbnailDir } from "./stores/thumbnails";
import { refreshInstalledPacks, initContentPackEvents } from "./stores/contentPacks";
import { showToast } from "./stores/toasts";
import { startTransferPolling } from "./stores/transfer";
import { NowPlayingBar } from "./components/NowPlayingBar";
import {
  initMusic, currentTrack, wantedTrack, playerHidden, hidePlayer, showPlayer, startShuffle, musicUnsupported,
} from "./stores/music";
import { IconMusicNote } from "./components/icons";
import "./styles/main.css";
import { Button } from "./components/Button";

type AppPhase = "loading" | "setup" | "ready";

function App() {
  const [phase, setPhase] = createSignal<AppPhase>("loading");
  const [showSettings, setShowSettings] = createSignal(false);
  const [settingsSection, setSettingsSection] = createSignal<SettingsSection>("general");
  const [showWelcomeModal, setShowWelcomeModal] = createSignal(false);
  const [showSeedingConsent, setShowSeedingConsent] = createSignal(false);
  const [dataDir, setDataDir] = createSignal("");
  const [rootFolder, setRootFolder] = createSignal("eXoDOS");
  /** Old per-collection folders waiting to be merged into the single root. */
  const [layoutMigration, setLayoutMigration] = createSignal<LayoutMigration | null>(null);
  const [migrating, setMigrating] = createSignal(false);
  /** Set once the user declined, so Settings can still offer the merge. */
  const [layoutSkipped, setLayoutSkipped] = createSignal(false);

  /** Merge the old folders and rebuild what derives from them. Blocking on
   *  purpose: an idle-looking launcher invites a second click. */
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
      void resumeDownloads();
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
    // No native context menu in production; components render their own.
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
    // Linux may be on WebKit's software renderer (§17), where backdrop blur
    // is a per-frame CPU repaint; main.css drops it under this class.
    if (navigator.userAgent.includes("Linux")) {
      document.documentElement.classList.add("soft-render");
    }
    // Before anything else: the backend can start pack installs on its own
    // (Win9x emulator auto-queue), and only this listener makes them visible.
    initContentPackEvents().catch(() => {});
    initDependencyDownloads().catch(() => {});
    initMusic().catch(() => {});
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
        // Re-check the disk: install flags are per game and go stale behind
        // the app's back. The backend refuses an empty data dir.
        // Downloads the session resumed by itself need their trackers back -
        // after the scan, which resets installs confirmed off a half-fetched archive.
        scanInstalledGames().then(() => { fetchGames(); void resumeDownloads(); }).catch(() => {});
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

    // Welcome modal once, never offline (it offers downloads); unwritten
    // `welcome_seen` re-offers it on the first online session.
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
    // An empty target usually means the user expected a move: ask, but only
    // when there is something to leave behind.
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

  /** Persist the dir and rebuild what derives from it; the rescan's count
   *  answers "did it find my games?". */
  const applyDataDir = async (selected: string) => {
    await setConfig("data_dir", selected);
    setDataDir(selected);
    resetStorageCache();
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

  const [showResetDialog, setShowResetDialog] = createSignal(false);
  const [deleteGameData, setDeleteGameData] = createSignal(false);

  const openSettings = (section: SettingsSection = "general") => {
    // Reports the folders even after a "not now", so Settings can offer the merge.
    pendingLayoutMigration()
      .then((m) => setLayoutSkipped(m != null))
      .catch(() => {});
    setSettingsSection(section);
    setShowSettings(true);
  };

  /** The answer from the one-time consent dialog. Errors propagate so the
   *  dialog can stay open and say so - a failed write here would otherwise
   *  leave the key unset and ask again on the next start. */
  const handleSeedingConsent = async (enabled: boolean) => {
    await applySeeding(enabled);
    setShowSeedingConsent(false);
    showToast(
      enabled ? "Sharing with other users is on" : "Sharing with other users is off",
      "info",
      { detail: "Change it any time in Settings → Network." },
    );
  };

  const confirmReset = async () => {
    const doDelete = deleteGameData();
    setShowResetDialog(false);
    setDeleteGameData(false);
    setResetError("");
    // Close the dialog first, then overlay whatever was behind it.
    setShowSettings(false);
    setResetting(true);
    console.log("[reset] calling factoryReset, deleteGameData=", doDelete);
    try {
      await factoryReset(doDelete);
      console.log("[reset] factoryReset succeeded, switching to setup");
      setPhase("setup");
      setDataDir("");
      resetStorageCache();
    } catch (e) {
      console.error("[reset] factoryReset failed:", e);
      setResetError(`Reset failed: ${e}`);
      setShowSettings(true);
    } finally {
      setResetting(false);
    }
  };

  /** With a track loaded (or being fetched) the button toggles the bar,
   *  otherwise it starts the shuffle. */
  const musicLoaded = () => currentTrack() != null || wantedTrack() != null;

  const musicButtonLabel = () => {
    if (!musicLoaded()) { return "Play music (shuffle)"; }
    return playerHidden() ? "Show player" : "Hide player";
  };

  const onMusicButton = () => {
    if (!musicLoaded()) { void startShuffle(); return; }
    if (playerHidden()) { showPlayer(); return; }
    hidePlayer();
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
            <ActivityBadge onOpenSettings={() => openSettings("network")} />
            <Show when={!musicUnsupported()}>
              <Tooltip.Root openDelay={400}>
                <Tooltip.Trigger asChild={(props) =>
                  <button {...props()} class="icon-btn" aria-label={musicButtonLabel()} onClick={onMusicButton}>
                    <IconMusicNote />
                  </button>
                } />
                <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">{musicButtonLabel()}</Tooltip.Content></Tooltip.Positioner></Portal>
              </Tooltip.Root>
            </Show>
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
                <button {...props()} class="icon-btn" data-testid="open-settings" onClick={() => openSettings()}>
                  &#9881;
                </button>
              } />
              <Portal><Tooltip.Positioner><Tooltip.Content class="ark-tooltip">Settings</Tooltip.Content></Tooltip.Positioner></Portal>
            </Tooltip.Root>
          </div>
        </div>

        <SettingsDialog
          open={showSettings()}
          onOpenChange={setShowSettings}
          section={settingsSection()}
          onSectionChange={setSettingsSection}
          gameFolderPath={gameFolderPath()}
          onChangeDataDir={() => void handleChangeDataDir()}
          layoutSkipped={layoutSkipped()}
          migrating={migrating()}
          onMergeLayout={() => void runLayoutMigration()}
          onFactoryReset={() => setShowResetDialog(true)}
          resetError={resetError()}
          onWentOnline={() => { needsSeedingConsent().then(setShowSeedingConsent).catch(() => {}); }}
        />

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

        {/* Last flex child on purpose: it becomes the bottom row of the shell
            without any fixed positioning, and the panel steps aside for it. */}
        <NowPlayingBar />

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
          title="Move your games into one folder?"
          message={"Exodium used to keep each collection in its own folder. It now uses eXoDOS's layout instead: one root folder for all of them. Everything you download from here on goes there, whether you move the old folders or not.\n\nMoving is instant and nothing is lost. If you skip it, Exodium only looks in the new root: the old folders stay invisible, their games read as not installed, and downloading one again writes a second copy while the old one still takes up the space. You can start the move later in Settings → Library."}
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
