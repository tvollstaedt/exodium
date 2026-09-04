//! eXoWin9x support-file pipeline.
//!
//! Win9x games boot Windows 95/98 from VHD images: the shared parent OS
//! images and the emulators that read them (eXo's DOSBox-X "x98" build,
//! 86Box + ROMs) ship in the torrent's `eXo/util/utilWin9x.zip` (2.5 GB),
//! nested inside its `EXTWin9x.zip` (2.47 GB) - the same matryoshka shape as
//! eXoDOS's util.zip. The payload is required on EVERY platform (the parent
//! VHDs are data, not binaries), so unlike the ECE build this is not
//! Windows-gated. `emulators/PCBox/` (Windows-only fork, unsupported) and
//! `emulators/audio/` (foobar2000) are never extracted.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, State};

use super::TorrentState;
use crate::models::Game;

static WIN9X_EXTRACTION_RUNNING: AtomicBool = AtomicBool::new(false);

/// Latched when the watcher gives up (3 failed attempts, e.g. disk full):
/// lets `get_win9x_support_status` answer "failed" instead of leaving the
/// panel on an eternal "Setting up…". Cleared when a watcher is (re-)armed.
static WIN9X_EXTRACTION_FAILED: AtomicBool = AtomicBool::new(false);

/// The subtrees extracted from EXTWin9x.zip into `<torrent_root>/eXo/`.
/// `emulators/dosbox/` holds the x98 tree (parent VHDs, differencing
/// children, base conf) plus options9x.conf/config9x.bat at its root;
/// `emulators/86Box98/` holds 86Box, its ROMs and its parents.
const EXTRACT_PREFIXES: [&str; 2] = ["emulators/dosbox/", "emulators/86box98/"];

/// Support files a game of the given dosbox_variant needs before launch.
/// x98 (DOSBox-X) games read the x98 tree; every 86Box flavor reads 86Box98.
pub(crate) fn win9x_support_ready(torrent_root: &Path, variant: Option<&str>) -> bool {
    let x98_ready = torrent_root.join("eXo/emulators/dosbox/x98/parent").exists();
    match variant {
        Some(v) if v.starts_with("86box") => {
            torrent_root.join("eXo/emulators/86Box98/parent").exists()
        }
        _ => x98_ready,
    }
}

/// Extract the Win9x support payload from utilWin9x.zip (blocking).
fn extract_win9x_support(util_zip: &Path, torrent_root: &Path) -> Result<usize, String> {
    if WIN9X_EXTRACTION_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("extraction already running".to_string());
    }
    let result = do_extract_win9x_support(util_zip, torrent_root);
    WIN9X_EXTRACTION_RUNNING.store(false, Ordering::SeqCst);
    result
}

fn do_extract_win9x_support(util_zip: &Path, torrent_root: &Path) -> Result<usize, String> {
    // Unique temp names so a leftover from a killed run can't collide.
    let pid = std::process::id();
    let tmp_path = util_zip.with_extension(format!("extwin9x_tmp_{pid}"));
    let staging_root = torrent_root.join("eXo").join(format!(".win9x_staging_{pid}"));

    let result = (|| {
        let file = std::fs::File::open(util_zip).map_err(|e| e.to_string())?;
        let mut outer = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        {
            let mut inner_entry = outer
                .by_name("EXTWin9x.zip")
                .map_err(|e| format!("EXTWin9x.zip not found inside utilWin9x.zip: {}", e))?;
            let mut tmp = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut inner_entry, &mut tmp).map_err(|e| e.to_string())?;
        }

        let tmp = std::fs::File::open(&tmp_path).map_err(|e| e.to_string())?;
        let mut inner = zip::ZipArchive::new(tmp).map_err(|e| e.to_string())?;
        let mut extracted = 0usize;
        for i in 0..inner.len() {
            let mut entry = inner.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            let lower = name.to_ascii_lowercase();
            if !EXTRACT_PREFIXES.iter().any(|p| lower.starts_with(p))
                || name.contains("..")
                || entry.is_dir()
            {
                continue;
            }
            let out_path = staging_root.join(&name);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            extracted += 1;
        }
        if extracted == 0 {
            return Err("no emulator entries found in EXTWin9x.zip".to_string());
        }

        // Move each fully-staged subtree into place with atomic renames -
        // the readiness gates test directory EXISTENCE, so a half-written
        // parent-VHD tree from a mid-extraction kill must never land.
        let dest_root = torrent_root.join("eXo");
        let staged_emulators = staging_root.join("emulators");
        let entries = std::fs::read_dir(&staged_emulators).map_err(|e| e.to_string())?;
        for entry in entries.filter_map(|e| e.ok()) {
            let rel = PathBuf::from("emulators").join(entry.file_name());
            let dst = dest_root.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if dst.exists() {
                std::fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
            }
            std::fs::rename(entry.path(), &dst)
                .map_err(|e| format!("moving {} into place: {}", rel.display(), e))?;
        }
        add_parent_case_aliases(&dest_root);
        Ok(extracted)
    })();

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_dir_all(&staging_root);
    result
}

/// The pack's play.confs reference the x98 parent VHDs in inconsistent case
/// (`win98jap` / `Win98Jap`, `Win95dx8` / `win95Dx8` / `Win95DX8`), which is
/// invisible on Windows/macOS but breaks ~24 games on Linux's case-sensitive
/// filesystems. Symlink every observed conf spelling to the real file. No-op
/// for aliases that already exist and on non-Unix platforms.
fn add_parent_case_aliases(dest_root: &Path) {
    #[cfg(unix)]
    {
        let parent_dir = dest_root.join("emulators/dosbox/x98/parent");
        const ALIASES: [(&str, &str); 5] = [
            ("win98jap.vhd", "win98Jap.vhd"),
            ("Win98Jap.vhd", "win98Jap.vhd"),
            ("Win95dx8.vhd", "Win95DX8.vhd"),
            ("win95Dx8.vhd", "Win95DX8.vhd"),
            ("win98chinese.vhd", "Win98Chinese.vhd"),
        ];
        for (alias, target) in ALIASES {
            let link = parent_dir.join(alias);
            if parent_dir.join(target).exists() && !link.exists() {
                if let Err(e) = std::os::unix::fs::symlink(target, &link) {
                    log::warn!("Failed to create parent-VHD case alias {}: {}", alias, e);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dest_root;
    }
}

/// Watch utilWin9x.zip until it finishes downloading, then extract the
/// support payload. Own task for the same reason as the MT-32 watcher: the
/// frontend only polls while a game download is active, and the 2.5 GB util
/// zip routinely finishes long after the game that triggered it.
pub(crate) fn spawn_win9x_support_watcher(
    mgr: std::sync::Arc<crate::torrent::manager::DownloadManager>,
    util_index: usize,
) {
    tauri::async_runtime::spawn(async move {
        WIN9X_EXTRACTION_FAILED.store(false, Ordering::SeqCst);
        let torrent_root = mgr.torrent_root();
        let expected_size = mgr.index().files.get(util_index).map(|f| f.size).unwrap_or(0);
        let mut failures = 0u32;
        // Generous ceiling: 6 h at 10 s per check for slow swarms.
        for _ in 0..2160 {
            if win9x_support_ready(&torrent_root, None)
                && win9x_support_ready(&torrent_root, Some("86box"))
            {
                return; // someone else finished the job
            }
            let Some(zip_path) = mgr.file_output_path(util_index) else {
                return;
            };
            // Stats-based completion PLUS a disk-size fallback (librqbit's
            // per-file stat can stall short of total; after a restart the
            // handle may be gone entirely) - see the MT-32 watcher.
            let stats_complete = mgr.is_file_complete(util_index).await;
            let disk_complete = expected_size > 0
                && std::fs::metadata(&zip_path).is_ok_and(|m| m.len() >= expected_size);
            if stats_complete || disk_complete {
                let root = torrent_root.clone();
                let zp = zip_path.clone();
                let outcome = tauri::async_runtime::spawn_blocking(move || {
                    extract_win9x_support(&zp, &root)
                })
                .await;
                match outcome {
                    Ok(Ok(n)) => {
                        log::info!("Extracted {} Win9x support files from utilWin9x.zip", n);
                        return;
                    }
                    Ok(Err(e)) if e == "extraction already running" => return,
                    Ok(Err(e)) => {
                        failures += 1;
                        log::error!(
                            "Failed to extract Win9x support files (attempt {}): {}",
                            failures, e
                        );
                        if failures >= 3 {
                            WIN9X_EXTRACTION_FAILED.store(true, Ordering::SeqCst);
                            return;
                        }
                    }
                    Err(e) => {
                        log::error!("Win9x extraction task panicked: {}", e);
                        WIN9X_EXTRACTION_FAILED.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
}

/// Queue utilWin9x.zip on the collection's manager (if not already selected)
/// and (re)arm the extraction watcher. Called from download_game when a
/// Win9x game is requested and the support tree is not on disk yet.
pub(crate) async fn ensure_win9x_support_queued(
    mgr: &std::sync::Arc<crate::torrent::manager::DownloadManager>,
) {
    let Some(util) = mgr.index().find_by_suffix("util/utilWin9x.zip") else {
        log::warn!("utilWin9x.zip not found in the eXoWin9x torrent index");
        return;
    };
    let util_index = util.index;
    if !mgr.is_file_selected(util_index).await {
        let _ = mgr.download_files(vec![util_index]).await;
        log::info!(
            "Also downloading utilWin9x.zip ({:.1} GB, one-time: Windows 9x OS images + emulators)",
            util.size as f64 / 1e9
        );
    }
    spawn_win9x_support_watcher(std::sync::Arc::clone(mgr), util_index);
}

/// Re-arm the extraction watcher after an app restart, so a utilWin9x.zip
/// that finishes downloading in a later session still gets extracted.
/// Called from init_download_manager once the eXoWin9x manager is hydrated.
pub(crate) async fn rearm_win9x_support(
    mgr: &std::sync::Arc<crate::torrent::manager::DownloadManager>,
) {
    let root = mgr.torrent_root();
    if win9x_support_ready(&root, None) && win9x_support_ready(&root, Some("86box")) {
        return;
    }
    let Some(util) = mgr.index().find_by_suffix("util/utilWin9x.zip") else {
        return;
    };
    let util_index = util.index;
    let selected = mgr.is_file_selected(util_index).await;
    let on_disk = mgr
        .file_output_path(util_index)
        .and_then(|p| std::fs::metadata(p).ok())
        .is_some_and(|m| m.len() > 0);
    if !selected && !on_disk {
        return; // support files were never requested - nothing to resume
    }
    log::info!(
        "Re-arming Win9x support extraction watcher (utilWin9x.zip {})",
        if selected { "still selected" } else { "present on disk" }
    );
    spawn_win9x_support_watcher(std::sync::Arc::clone(mgr), util_index);
}

// ── Launch ───────────────────────────────────────────────────────────────────

/// How a resolved engine is invoked: a binary on disk, or DOSBox-X's Flatpak
/// on Linux (no official Linux binaries exist for DOSBox-X).
enum EngineCmd {
    Direct(PathBuf),
    Flatpak(&'static str),
}

impl EngineCmd {
    /// Build a Command; `grant` is a directory the Flatpak sandbox must see.
    fn command(&self, grant: &Path) -> (Command, PathBuf) {
        match self {
            EngineCmd::Direct(bin) => (Command::new(bin), bin.clone()),
            EngineCmd::Flatpak(id) => {
                let mut cmd = Command::new("flatpak");
                cmd.arg("run")
                    .arg(format!("--filesystem={}", grant.display()))
                    .arg(id);
                (cmd, PathBuf::from("flatpak"))
            }
        }
    }
}

/// A Command for console-subsystem helpers (`where`, `powershell`). Exodium
/// is a GUI-subsystem exe on Windows, so without CREATE_NO_WINDOW every such
/// child gets its own console - the panel's engine probe flashed two CMD
/// windows per open.
fn hidden_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn binary_exists_on_path(name: &str) -> bool {
    let checker = if cfg!(windows) { "where" } else { "which" };
    hidden_command(checker)
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Bundled-resource probe: <resource_dir>/<sub>, falling back to the dev
/// tree's src-tauri/resources/<sub> (same convention as resolve_dosbox).
fn resource_candidate(app: &AppHandle, sub: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join(sub));
    }
    if let Some(res) = crate::commands::setup::RESOURCE_DIR.get() {
        candidates.push(res.join(sub));
    }
    // Dev builds: the emulators are no longer in bundle.resources, so tauri
    // does not stage them into target/debug - probe the dev tree directly
    // (get-emulators.sh fills src-tauri/resources/).
    if cfg!(debug_assertions) {
        candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join(sub));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Downloaded emulator pack probe: <data_dir>/content/emulators/<sub>.
///
/// Deliberately a filesystem check, not a ledger lookup: factory_reset with
/// kept game data wipes the config table (and with it the content_packs
/// ledger) while `content/` survives - launching must keep working before
/// the next `list_content_packs` re-adopts the pack.
fn pack_candidate(data_dir: &str, sub: &str) -> Option<PathBuf> {
    if data_dir.is_empty() {
        return None;
    }
    let p = Path::new(data_dir).join("content/emulators").join(sub);
    p.exists().then_some(p)
}

/// Resolve a possibly bare command name to its absolute PATH location.
#[cfg(target_os = "linux")]
fn absolutize_on_path(bin: &Path) -> Option<PathBuf> {
    if bin.is_absolute() {
        return Some(bin.to_path_buf());
    }
    let out = Command::new("which").arg(bin).output().ok()?;
    let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!resolved.is_empty()).then(|| PathBuf::from(resolved))
}

/// Does this binary carry CAP_NET_RAW? Asked of the file, not our process:
/// the capability sits on the emulator, and Exodium itself never has it.
#[cfg(target_os = "linux")]
fn has_cap_net_raw(bin: &Path) -> bool {
    let Some(abs) = absolutize_on_path(bin) else { return false };
    // getcap lives in /usr/sbin on Debian-family systems, which is not on a
    // desktop user's PATH. Missing everywhere = treat as "no capability".
    ["getcap", "/usr/sbin/getcap", "/sbin/getcap"].iter().any(|g| {
        Command::new(g)
            .arg(&abs)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("cap_net_raw"))
            .unwrap_or(false)
    })
}

/// A system-installed DOSBox-X that already holds CAP_NET_RAW - the one
/// binary pcap multiplayer can run through. The pack's AppImage can never be
/// that binary: a file capability puts the loader into secure-execution mode,
/// which ignores the LD_LIBRARY_PATH its bundled libraries need, and the
/// capability would be lost on every pack update anyway.
#[cfg(target_os = "linux")]
fn path_dosbox_x_with_cap() -> Option<PathBuf> {
    let abs = absolutize_on_path(Path::new("dosbox-x"))?;
    has_cap_net_raw(&abs).then_some(abs)
}

/// Is the DOSBox-X Flatpak installed? (Linux fallback of last resort.)
fn flatpak_dosbox_x_available() -> bool {
    Command::new("flatpak")
        .args(["info", "com.dosbox_x.DOSBox-X"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// DOSBox-X for x98 games. On Windows eXo's own "x98" build (extracted from
/// EXTWin9x.zip) is the intended emulator, exactly like the ECE precedent, and
/// nothing is bundled for it - see `resolve_86box` for why. macOS and Linux
/// use the downloaded emulator pack (pinned 2025.02.01, near eXo's own x98
/// build), falling back to the pre-pack bundled copy, then PATH, then the
/// Flatpak on Linux.
fn resolve_dosbox_x(app: &AppHandle, torrent_root: &Path, data_dir: &str) -> Option<EngineCmd> {
    if cfg!(windows) {
        let exo_build = torrent_root.join("eXo/emulators/dosbox/x98/dosbox-x.exe");
        if exo_build.exists() {
            return Some(EngineCmd::Direct(exo_build));
        }
    }
    // A system copy already holding CAP_NET_RAW outranks the pack: the user
    // set it up for pcap multiplayer, and the pack's AppImage cannot carry
    // the capability (see path_dosbox_x_with_cap).
    #[cfg(target_os = "linux")]
    if let Some(bin) = path_dosbox_x_with_cap() {
        return Some(EngineCmd::Direct(bin));
    }
    let packed = if cfg!(target_os = "macos") {
        pack_candidate(data_dir, "dosbox-x/dosbox-x.app/Contents/MacOS/dosbox-x")
    } else if cfg!(target_os = "linux") {
        pack_candidate(data_dir, "dosbox-x/DOSBox-X.AppImage")
    } else {
        None
    };
    if let Some(bin) = packed {
        return Some(EngineCmd::Direct(bin));
    }
    let bundled = if cfg!(target_os = "macos") {
        resource_candidate(app, "dosbox-x/dosbox-x.app/Contents/MacOS/dosbox-x")
    } else {
        None
    };
    if let Some(bin) = bundled {
        return Some(EngineCmd::Direct(bin));
    }
    if binary_exists_on_path("dosbox-x") {
        return Some(EngineCmd::Direct(PathBuf::from("dosbox-x")));
    }
    if cfg!(target_os = "linux") && flatpak_dosbox_x_available() {
        return Some(EngineCmd::Flatpak("com.dosbox_x.DOSBox-X"));
    }
    None
}

/// 86Box for 86box* games. On Windows eXo's own build is used, for the same
/// reason as DOSBox-X: it comes out of EXTWin9x.zip together with the parent
/// VHDs, and `win9x_support_ready` already refuses to launch without those -
/// so a bundled Windows build can never be the reason a launch succeeds, and
/// cost 68 MB of installer to never run. macOS/Linux need their own builds
/// (the pack ships .exe only). PATH fallback on every platform.
fn resolve_86box(app: &AppHandle, torrent_root: &Path, data_dir: &str) -> Option<EngineCmd> {
    if cfg!(windows) {
        let exo_build = torrent_root.join("eXo/emulators/86Box98/86Box.exe");
        if exo_build.exists() {
            return Some(EngineCmd::Direct(exo_build));
        }
    }
    let sub = if cfg!(target_os = "macos") {
        Some("86box/86Box.app/Contents/MacOS/86Box")
    } else if cfg!(target_os = "linux") {
        Some("86box/86Box.AppImage")
    } else {
        None
    };
    if let Some(sub) = sub {
        if let Some(bin) = pack_candidate(data_dir, sub) {
            return Some(EngineCmd::Direct(bin));
        }
        if let Some(bin) = resource_candidate(app, sub) {
            return Some(EngineCmd::Direct(bin));
        }
    }
    if binary_exists_on_path("86Box") {
        return Some(EngineCmd::Direct(PathBuf::from("86Box")));
    }
    None
}

/// Which emulator pack a Win9x variant needs: the auto-queue and the panel
/// button both key off this. pcbox has no pack (Windows-only emulator we do
/// not ship); Windows needs no pack at all (eXo's EXTWin9x.zip carries both
/// builds next to the parent VHDs).
pub(crate) fn emulator_pack_for_variant(variant: Option<&str>) -> Option<&'static str> {
    match variant {
        Some("pcbox") => None,
        Some(v) if v.starts_with("86box") => Some("86box"),
        _ => Some("dosbox-x"),
    }
}

/// Would launching this variant find its emulator right now? Wraps the same
/// resolvers `launch_win9x_game` uses, so the auto-queue and the panel note
/// can never disagree with what launch would actually do.
pub(crate) fn win9x_engine_resolvable(
    app: &AppHandle,
    torrent_root: &Path,
    data_dir: &str,
    variant: Option<&str>,
) -> bool {
    match variant {
        Some("pcbox") => false,
        Some(v) if v.starts_with("86box") => resolve_86box(app, torrent_root, data_dir).is_some(),
        _ => resolve_dosbox_x(app, torrent_root, data_dir).is_some(),
    }
}

/// Per-variant 86Box wiring, from eXo's 9xlaunch86Box*.bat files: which
/// disposable child VHD to recreate, off which parent, and which per-game
/// cfg to copy into the emulator dir.
fn e86box_variant_files(variant: &str) -> (&'static str, &'static str, &'static str) {
    match variant {
        "86boxME" => ("ME-C.vhd", "ME-P.vhd", "play.cfg"),
        "86boxNetHost" => ("W98-Host.vhd", "W98-NetHost.vhd", "Host.cfg"),
        "86boxNetJoin" => ("W98-Join.vhd", "W98-NetJoin.vhd", "Join.cfg"),
        _ => ("W98-C.vhd", "W98-P.vhd", "play.cfg"),
    }
}

/// Case-insensitive lookup of a file name inside a directory (the conf dirs
/// mix "play.cfg"/"Play.cfg" and the pack was authored case-insensitively).
fn find_file_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    let lower = name.to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).find_map(|e| {
        (e.file_name().to_string_lossy().to_ascii_lowercase() == lower).then(|| e.path())
    })
}

/// The single top-level directory a zip wraps everything in, if it does.
///
/// eXo's convention is files at the zip ROOT - the game's desktop shortcut
/// points straight at `E:\<GAME>.EXE`. A zip that wraps them in a folder puts
/// the executable one level too deep and the shortcut dies with "drive or
/// network connection is unavailable" (Chinese Checkers: `CC32/CCHECK11.EXE`
/// against a shortcut for `E:\CCHECK11.EXE`, after eXo repackaged a newer
/// build).
fn zip_wrapper_dir(zip_path: &Path) -> Option<String> {
    let file = std::fs::File::open(zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut wrapper: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).ok()?;
        let name = entry.name().replace('\\', "/");
        let top = name.split('/').next()?.to_string();
        if !name[top.len()..].starts_with('/') {
            return None;
        }
        match &wrapper {
            Some(w) if *w != top => return None,
            Some(_) => {}
            None => wrapper = Some(top),
        }
    }
    wrapper
}

/// Rewrite `MOUNT <letter> "<...>.zip"` to mount an extracted copy instead.
///
/// Two reasons, both measured:
///
/// 1. **A zip mount crashes DOSBox-X on exit.** Booting a guest OS converts
///    every mounted host drive into an emulated FAT disk (`convertdrivefat`),
///    and tearing that down walks into PhysFS - the zip layer - after it is
///    gone: `PHYSFS_close <- physfsFile::Close <- fatFromDOSDrive::~ <-
///    FreeBIOSDiskList`, SIGSEGV, in three of three crash reports. That
///    aborts the teardown loop, so disks later in the list - including the
///    game's own save VHD - never get closed cleanly. A directory mount has
///    no PhysFS layer and shuts down normally.
/// 2. A zip that wraps its files in one directory mounts them a level too
///    deep for the game's desktop shortcut (see `zip_wrapper_dir`).
///
/// The extracted copy lives next to the zip and is reused on later launches.
fn extract_zip_mounts(conf: &str, exo_dir: &Path) -> String {
    let mount_target = |line: &str| -> Option<(String, String)> {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix("MOUNT ").or_else(|| trimmed.strip_prefix("mount "))?;
        let (letter, target) = rest.trim_start().split_once(char::is_whitespace)?;
        let target = target.trim().trim_matches('"');
        target
            .to_ascii_lowercase()
            .ends_with(".zip")
            .then(|| (letter.trim().to_string(), target.to_string()))
    };

    conf.lines()
        .map(|line| {
            let Some((letter, target)) = mount_target(line) else {
                return line.to_string();
            };
            let zip_path = if Path::new(&target).is_absolute() {
                PathBuf::from(&target)
            } else {
                exo_dir.join(target.trim_start_matches("./"))
            };
            let dest = zip_path.with_extension("exodium_mount");
            let inner = match zip_wrapper_dir(&zip_path) {
                Some(wrapper) => dest.join(wrapper),
                None => dest.clone(),
            };
            if !inner.is_dir() {
                let Ok(file) = std::fs::File::open(&zip_path) else {
                    return line.to_string();
                };
                let extracted = zip::ZipArchive::new(file)
                    .and_then(|mut a| a.extract(&dest))
                    .is_ok();
                if !extracted || !inner.is_dir() {
                    let _ = std::fs::remove_dir_all(&dest);
                    return line.to_string();
                }
                log::info!("Extracted {} for mounting as a directory", zip_path.display());
            }
            format!("MOUNT {} \"{}\"", letter, inner.display())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Can this process capture raw packets? DOSBox-X's `pcap` backend bridges
/// the guest NIC onto a real interface, which is what eXo's remote-multiplayer
/// titles need: the guest dials a PPTP tunnel to a community-run IPX gateway
/// (the ipxbox project, which pairs an IPX server with a PPTP endpoint for
/// Win9x clients), and PPTP rides on GRE - a protocol user-mode NAT cannot
/// carry.
///
/// Windows gets this for free (the pack's setup installs npcap). On macOS the
/// `/dev/bpf*` nodes are root-only unless Wireshark's ChmodBPF helper is
/// installed; on Linux it takes CAP_NET_RAW. Both are one-time, user-side
/// decisions we must not make for them - so we detect and adapt instead.
#[cfg(all(unix, not(target_os = "linux")))]
fn can_capture_packets() -> bool {
    (0..4).any(|i| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("/dev/bpf{i}"))
            .is_ok()
    })
}

/// Can the emulator we would launch open a capture handle?
///
/// macOS: the /dev/bpf* nodes are user-visible state, and a child process
/// inherits our access - probing from Exodium's own process answers for the
/// emulator too. Linux is different: CAP_NET_RAW is a FILE capability on the
/// emulator binary, not on us, so an AF_PACKET probe from this process says
/// nothing about DOSBox-X (it answered false even after a successful setcap).
/// Ask the binary instead - and because `resolve_dosbox_x` prefers a
/// capability-holding system copy, "some dosbox-x on PATH has the cap" is
/// exactly "the binary we will launch has the cap".
#[cfg(unix)]
fn host_can_bridge() -> bool {
    #[cfg(target_os = "linux")]
    {
        path_dosbox_x_with_cap().is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        can_capture_packets()
    }
}

/// Whether an interface can carry a second machine's MAC address.
///
/// Wi-Fi cannot: a station is associated with exactly one MAC, and the access
/// point neither forwards frames from a foreign source MAC nor delivers frames
/// addressed to one (802.11 has no client-side bridging outside WDS). The
/// emulated NE2000 card has its own MAC, so on Wi-Fi its DHCP request is
/// simply dropped - the guest ends up with no address at all, which is worse
/// than the NAT it had before. Measured: Windows 98 answers "Error 752, the
/// host name you dialed could not be found".
#[cfg(unix)]
fn is_wired_interface(name: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(out) = Command::new("networksetup")
            .arg("-listallhardwareports")
            .output()
        else {
            return false;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut port = "";
        for line in text.lines() {
            if let Some(p) = line.strip_prefix("Hardware Port:") {
                port = p.trim();
            } else if line.strip_prefix("Device:").map(str::trim) == Some(name) {
                let p = port.to_ascii_lowercase();
                return !["wi-fi", "airport", "bluetooth", "iphone", "thunderbolt bridge"]
                    .iter()
                    .any(|w| p.contains(w));
            }
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        let base = std::path::Path::new("/sys/class/net").join(name);
        base.exists() && !base.join("wireless").exists() && !base.join("phy80211").exists()
    }
}

/// The host interface to bridge onto, i.e. the one carrying the default route.
/// eXo's confs name a Windows adapter (`realnic = Rea…`), which means nothing
/// here, so pcap needs an explicit local answer.
#[cfg(unix)]
fn default_interface() -> Option<String> {
    let (prog, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("route", &["-n", "get", "default"])
    } else {
        ("ip", &["-o", "route", "get", "1.1.1.1"])
    };
    let out = Command::new(prog).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if cfg!(target_os = "macos") {
        text.lines()
            .find_map(|l| l.trim().strip_prefix("interface:"))
            .map(|s| s.trim().to_string())
    } else {
        let mut parts = text.split_whitespace();
        while let Some(word) = parts.next() {
            if word == "dev" {
                return parts.next().map(|s| s.to_string());
            }
        }
        None
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Win9xNetworkStatus {
    /// True once the host lets us bridge the guest onto a real interface.
    pub enabled: bool,
    /// False where nothing can be done from inside the app (Flatpak DOSBox-X,
    /// missing PolicyKit) - the UI then shows `manual_hint` instead of a button.
    pub can_enable: bool,
    /// Platform-specific one-liner for what enabling actually grants.
    pub detail: String,
    /// Command to run by hand when `can_enable` is false.
    pub manual_hint: Option<String>,
}

/// Whether eXo's remote-multiplayer titles can reach their IPX gateway, and
/// whether Exodium can obtain that permission for the user.
#[tauri::command]
pub async fn win9x_network_status() -> Result<Win9xNetworkStatus, String> {
    #[cfg(unix)]
    {
        let captures = host_can_bridge();
        let wired = default_interface().is_some_and(|n| is_wired_interface(&n));
        // Linux grants the capability to a system-installed dosbox-x; the
        // downloaded pack's AppImage cannot hold it (secure-exec would break
        // its bundled libraries), so without a PATH copy there is nothing to
        // enable and the row has to say why.
        #[cfg(target_os = "linux")]
        let system_bin = binary_exists_on_path("dosbox-x");
        #[cfg(not(target_os = "linux"))]
        let system_bin = true;
        #[cfg(target_os = "linux")]
        let tool = binary_exists_on_path("pkexec");
        #[cfg(not(target_os = "linux"))]
        let tool = true;
        Ok(Win9xNetworkStatus {
            enabled: captures && wired,
            can_enable: !captures && wired && tool && system_bin,
            detail: match (captures, wired) {
                (true, true) => "Enabled.".into(),
                (_, false) => "Online play needs a wired connection - Wi-Fi cannot bridge \
                               the emulated network card."
                    .into(),
                (false, true) if !system_bin => {
                    "Multiplayer needs a system-installed DOSBox-X - the downloaded \
                     emulator pack cannot hold packet-capture permission. Install \
                     DOSBox-X from your distribution's packages."
                        .into()
                }
                (false, true) => "Needs packet access, like Wireshark.".into(),
            },
            manual_hint: (!captures && wired && system_bin && !tool)
                .then(|| "sudo setcap cap_net_raw+ep $(which dosbox-x)".to_string()),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(Win9xNetworkStatus {
            enabled: true,
            can_enable: false,
            detail: "Multiplayer uses npcap, which ships with the eXoWin9x support files."
                .into(),
            manual_hint: None,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Win9xMultiplayerInfo {
    /// The game boots one of eXo's network parent images (67 titles).
    pub multiplayer: bool,
    /// "ready" | "needs_permission" | "needs_wired" | "unknown"
    pub state: String,
    /// Whether Play should offer the one-time setup before launching.
    pub prompt: bool,
}

/// Is the default route on a wireless link? None when it cannot be told.
///
/// Bridging over Wi-Fi is a DOSBox-X limitation, not a platform one: its own
/// documentation notes that the pcap backend "needs very low level access to
/// your real network adapter, which can be problematic with wireless
/// adapters", and recommends slirp there. Windows with npcap is no exception,
/// so the check runs on all three platforms.
fn on_wireless_link() -> Option<bool> {
    #[cfg(unix)]
    {
        default_interface().map(|n| !is_wired_interface(&n))
    }
    #[cfg(windows)]
    {
        let out = hidden_command("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "$i=(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric | \
                 Select-Object -First 1).InterfaceIndex; \
                 (Get-NetAdapter -InterfaceIndex $i).PhysicalMediaType",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        let media = String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase();
        if media.is_empty() {
            return None;
        }
        Some(media.contains("802.11") || media.contains("wireless"))
    }
}

/// What online play looks like for this game on this machine.
///
/// The panel needs more than a yes/no: a title that cannot go online because
/// the host is on Wi-Fi gets a note but no dialog (there is nothing to grant),
/// while one that only lacks the permission gets the offer on Play. Silence in
/// both cases is what leaves players wondering why the in-game dial fails.
#[tauri::command]
pub async fn win9x_multiplayer_info(
    db_state: State<'_, super::DbState>,
    id: i64,
) -> Result<Win9xMultiplayerInfo, String> {
    let not_multiplayer = Win9xMultiplayerInfo {
        multiplayer: false,
        state: "unknown".into(),
        prompt: false,
    };
    let (game, data_dir, asked) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = crate::db::queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {id} not found"))?;
        let data_dir = crate::db::queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let asked = crate::db::queries::get_config(&conn, "win9x_network_prompt")
            .map_err(|e| e.to_string())?;
        (game, data_dir, asked)
    };
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    if !crate::commands::setup::collection_def(source).is_some_and(|c| c.year_subdirs) {
        return Ok(not_multiplayer);
    }
    let Some(app_path) = game.application_path.as_deref() else {
        return Ok(not_multiplayer);
    };
    let torrent_root = crate::commands::setup::game_root(&data_dir);
    let Some(conf_dir) = app_path
        .replace('\\', "/")
        .rsplit_once('/')
        .map(|(dir, _)| torrent_root.join(dir))
    else {
        return Ok(not_multiplayer);
    };
    let Some(play_conf) = find_file_ci(&conf_dir, "play.conf") else {
        return Ok(not_multiplayer);
    };
    let conf = std::fs::read_to_string(&play_conf).unwrap_or_default();
    if !conf.to_ascii_lowercase().contains("w98-c-net") {
        return Ok(not_multiplayer);
    }

    // Wireless is the one answer that holds on every platform. Everything
    // else differs: Unix has to be granted raw capture, Windows gets it from
    // the npcap that ships with eXo's support files.
    let state = match on_wireless_link() {
        Some(true) => "needs_wired",
        _ => {
            #[cfg(unix)]
            {
                if bridgeable_interface().is_some() {
                    "ready"
                } else {
                    "needs_permission"
                }
            }
            #[cfg(not(unix))]
            {
                "ready"
            }
        }
    };
    Ok(Win9xMultiplayerInfo {
        multiplayer: true,
        prompt: state == "needs_permission" && asked.as_deref() != Some("off"),
        state: state.into(),
    })
}

/// Remember that the user does not want to be asked about multiplayer again.
#[tauri::command]
pub async fn dismiss_win9x_network_prompt(
    db_state: State<'_, super::DbState>,
) -> Result<(), String> {
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
    crate::db::queries::set_config(&conn, "win9x_network_prompt", "off").map_err(|e| e.to_string())
}

/// Ask the operating system - not the user's shell - for the permission that
/// bridging needs. macOS shows its own authentication sheet; Linux shows
/// PolicyKit's. Nothing here runs without that dialog being accepted.
#[tauri::command]
pub async fn enable_win9x_network(app: AppHandle) -> Result<Win9xNetworkStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = &app;
        install_bpf_daemon_macos().await?;
    }
    #[cfg(target_os = "linux")]
    {
        let _ = &app;
        grant_cap_net_raw_linux().await?;
    }
    #[cfg(windows)]
    {
        let _ = &app;
    }
    win9x_network_status().await
}

/// Give the permission back. Same consent dialog, opposite direction - a
/// grant the user cannot revoke from the same place they made it is a trap.
#[tauri::command]
pub async fn disable_win9x_network(app: AppHandle) -> Result<Win9xNetworkStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = &app;
        let label = "com.redfox.exodium.bpf";
        let dest = format!("/Library/LaunchDaemons/{label}.plist");
        run_privileged_macos(&format!(
            "#!/bin/sh\n\
             launchctl unload '{dest}' 2>/dev/null || true\n\
             rm -f '{dest}'\n\
             chown root:wheel /dev/bpf* && chmod 600 /dev/bpf*\n"
        ))
        .await?;
    }
    #[cfg(target_os = "linux")]
    {
        let _ = &app;
        let bin = resolved_dosbox_x_path()?;
        let status = Command::new("pkexec")
            .arg("setcap")
            .arg("-r")
            .arg(&bin)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() && status.code() != Some(126) {
            return Err(format!("could not revoke packet access on {}", bin.display()));
        }
    }
    #[cfg(windows)]
    {
        let _ = &app;
    }
    win9x_network_status().await
}

/// Install a boot-time helper that hands this user the BPF devices.
///
/// Same shape as Wireshark's ChmodBPF, with one deliberate difference: the
/// nodes are chowned to the current user rather than opened up to a shared
/// `access_bpf` group. It is the narrower grant, and it takes effect
/// immediately - a new group membership would only apply after a re-login,
/// which reads as "the button did nothing".
#[cfg(target_os = "macos")]
async fn install_bpf_daemon_macos() -> Result<(), String> {
    let user = std::env::var("USER").map_err(|_| "cannot determine the current user")?;
    if !user.chars().all(|c| c.is_alphanumeric() || "._-".contains(c)) {
        return Err(format!("unexpected user name: {user}"));
    }
    let label = "com.redfox.exodium.bpf";
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>RunAtLoad</key><true/>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string><string>-c</string>
    <string>chown {user} /dev/bpf* &amp;&amp; chmod 600 /dev/bpf*</string>
  </array>
</dict>
</plist>
"#
    );
    let tmp_dir = std::env::temp_dir().join(format!("exodium_bpf_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp_plist = tmp_dir.join("daemon.plist");
    std::fs::write(&tmp_plist, plist).map_err(|e| e.to_string())?;

    let dest = format!("/Library/LaunchDaemons/{label}.plist");
    let script = tmp_dir.join("install.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nset -e\n\
             cp '{}' '{dest}'\n\
             chown root:wheel '{dest}'\n\
             chmod 644 '{dest}'\n\
             launchctl unload '{dest}' 2>/dev/null || true\n\
             launchctl load -w '{dest}'\n\
             chown {user} /dev/bpf* && chmod 600 /dev/bpf*\n",
            tmp_plist.display()
        ),
    )
    .map_err(|e| e.to_string())?;

    run_privileged_macos_script(&script).await
}

/// Run a shell script as root through macOS's own authentication sheet.
///
/// Piped stdio is what makes this fail INSIDE the app: a Tauri 2 GUI process
/// on macOS returns EBADF from posix_spawn when the child inherits
/// parent-derived descriptors (the same bug the emulator spawn works around).
/// So: null stdio, a shell does the redirection into files, and pre_exec
/// forces fork+exec, which tolerates the parent's fd state.
#[cfg(target_os = "macos")]
async fn run_privileged_macos_script(script: &Path) -> Result<(), String> {
    let tmp_dir = script.parent().ok_or("bad script path")?.to_path_buf();
    let applescript = tmp_dir.join("prompt.applescript");
    std::fs::write(
        &applescript,
        format!(
            "do shell script \"/bin/sh {}\" with administrator privileges\n",
            script.display()
        ),
    )
    .map_err(|e| e.to_string())?;

    let err_file = tmp_dir.join("stderr.txt");
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(format!(
            "osascript '{}' 2>'{}'",
            applescript.display(),
            err_file.display()
        ))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| Ok(()));
        }
    }
    let status = cmd.status().map_err(|e| e.to_string())?;
    let stderr = std::fs::read_to_string(&err_file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if !status.success() {
        // -128 is the user dismissing the authentication sheet.
        if stderr.contains("-128") || stderr.contains("User canceled") {
            return Err("cancelled".into());
        }
        return Err(format!(
            "the helper could not be changed: {}",
            stderr.trim().lines().last().unwrap_or("unknown error")
        ));
    }
    Ok(())
}

/// Write `body` to a temp script and run it as root.
#[cfg(target_os = "macos")]
async fn run_privileged_macos(body: &str) -> Result<(), String> {
    let tmp_dir = std::env::temp_dir().join(format!("exodium_priv_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let script = tmp_dir.join("run.sh");
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    run_privileged_macos_script(&script).await
}

/// Absolute path of the DOSBox-X a capability can sit on: a system install
/// from PATH, nothing else. The downloaded pack's AppImage is deliberately
/// not an option - a file capability puts the loader into secure-execution
/// mode, which ignores the LD_LIBRARY_PATH its bundled libraries need, and a
/// pack update would silently drop the grant. Flatpak cannot carry one at
/// all. (`resolve_dosbox_x` prefers a capability-holding PATH copy, so the
/// grant lands on the binary that then actually launches.)
#[cfg(target_os = "linux")]
fn resolved_dosbox_x_path() -> Result<PathBuf, String> {
    if let Some(bin) = absolutize_on_path(Path::new("dosbox-x")) {
        return Ok(bin);
    }
    if flatpak_dosbox_x_available() {
        return Err(
            "The Flatpak build of DOSBox-X cannot be granted packet access. Install \
             DOSBox-X from your distribution's packages to use multiplayer."
                .into(),
        );
    }
    Err(
        "Multiplayer needs a system-installed DOSBox-X - the emulator pack Exodium \
         downloads cannot hold packet-capture permission. Install DOSBox-X from your \
         distribution's packages."
            .into(),
    )
}

/// Put CAP_NET_RAW on the DOSBox-X binary the launcher actually resolves.
#[cfg(target_os = "linux")]
async fn grant_cap_net_raw_linux() -> Result<(), String> {
    let bin = resolved_dosbox_x_path()?;
    // Null stdio for the same reason as the macOS path: a GUI process must
    // not hand parent descriptors to a privileged child.
    let status = Command::new("pkexec")
        .arg("setcap")
        .arg("cap_net_raw+ep")
        .arg(&bin)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        // 126 is PolicyKit's "the dialog was dismissed".
        if status.code() == Some(126) {
            return Err("cancelled".into());
        }
        return Err(format!(
            "could not grant packet access to {} (exit {})",
            bin.display(),
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

/// The interface pcap can bridge onto. Wired only - see below.
///
/// Wired links carry any source address, so the guest keeps its own MAC and
/// gets its own DHCP lease. Wi-Fi cannot be made to work, and the reason is
/// worth recording because the obvious fix looks convincing until you capture
/// the traffic:
///
/// A station is associated under exactly one address and normal 3-address
/// frames have no field for a second one, so a guest with its own MAC gets
/// nothing through. Cloning the host's MAC into the emulated card does fix
/// that layer - but then the DHCP server, which keys leases on the MAC, hands
/// the guest THE HOST'S OWN IP. Both stacks now answer for one address, and
/// the host's kernel resets every connection the guest opens. Measured on
/// macOS Wi-Fi, the guest's PPTP dial:
///
///   guest > server:1723  [S]     ; SYN from 10.x.y.z (the host's own IP)
///   server > guest       [S.]    ; server answers
///   10.x.y.z > server    [R]     ; the HOST's stack resets it
///
/// The remaining fix - a static guest IP outside the DHCP pool - lives inside
/// eXo's Win9x image, not here. So on Wi-Fi we stay on slirp, which at least
/// gives the guest working TCP/UDP.
#[cfg(unix)]
fn bridgeable_interface() -> Option<String> {
    if !host_can_bridge() {
        return None;
    }
    default_interface().filter(|nic| is_wired_interface(nic))
}

/// Network-backend fragment appended to every DOSBox-X launch.
///
/// Windows keeps eXo's authored `pcap` setup verbatim. Elsewhere we bridge
/// with pcap when the host allows raw capture (then remote multiplayer works
/// as eXo intended) and otherwise fall back to slirp: user-mode NAT that
/// carries plain TCP/UDP, loads without a permission prompt, and above all
/// does not greet the player with an in-guest network error at boot.
fn ne2000_override() -> String {
    #[cfg(unix)]
    {
        if let Some(nic) = bridgeable_interface() {
            log::info!("Win9x networking: bridging the guest NIC onto {nic} (pcap)");
            return format!("[ne2000]\nbackend = pcap\n[ethernet, pcap]\nrealnic = {nic}\n");
        }
        "[ne2000]\nbackend = slirp\n".to_string()
    }
    #[cfg(not(unix))]
    {
        String::new()
    }
}

pub(crate) async fn launch_win9x_game(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    fullscreen: bool,
    per_game_config: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    let source = game.torrent_source.as_deref().unwrap_or("eXoWin9x");
    let torrent_root = crate::commands::setup::game_root(data_dir);
    let variant = game.dosbox_variant.clone().unwrap_or_else(|| "x98".to_string());
    let variant = variant.as_str();

    // Per-game fullscreen override, same tri-state the Staging path honors
    // ("" = global default). This path used to read only the global flag, so
    // the Game Settings toggle silently did nothing for Win9x games.
    let fullscreen = match per_game_config.get("fullscreen").map(String::as_str) {
        Some("true") => true,
        Some("false") => false,
        _ => fullscreen,
    };

    if variant == "pcbox" {
        return Err(format!(
            "'{}' needs PCBox, a Windows-only emulator Exodium does not ship yet.",
            game.title
        ));
    }

    if !win9x_support_ready(&torrent_root, Some(variant)) {
        return Err(
            "Windows 9x support files (OS images + emulators) are not installed yet. \
             They download automatically with the first Win9x game - check the \
             download progress, or re-download any Win9x game to restart it."
                .to_string(),
        );
    }

    let app_path = game
        .application_path
        .as_deref()
        .ok_or_else(|| format!("'{}' has no launcher path in the catalogue", game.title))?;
    // Conf dir: eXo/eXoWin9x/!win9x/<year>/<TitleDir>/ - the parent of the
    // per-game launcher bat.
    let rel_conf_dir = app_path
        .replace('\\', "/")
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .ok_or_else(|| format!("Unexpected launcher path: {}", app_path))?;
    let conf_dir = torrent_root.join(&rel_conf_dir);
    if !conf_dir.exists() {
        return Err(format!(
            "Game config folder not found: {}\nRe-install the game to restore it.",
            conf_dir.display()
        ));
    }

    // Auto-extract the game ZIP on first launch (imported installs may still
    // be zipped - mirrors the Staging path's behavior).
    let shortcode = game.shortcode.as_deref().unwrap_or("");
    if !shortcode.is_empty() {
        let game_dir = torrent_root.join(super::games::collection_rel_game_dir(
            source,
            shortcode,
            Some(app_path),
        ));
        if !game_dir.exists() {
            let game_name = Some(app_path)
                .and_then(crate::commands::setup::game_name_from_app_path)
                .unwrap_or_else(|| game.title.clone());
            let zip = torrent_root.join(super::games::collection_rel_zip(
                source,
                &game_name,
                Some(app_path),
            ));
            if zip.exists() {
                log::info!("Auto-extracting {} before launch", zip.display());
                let dest = zip.parent().map(PathBuf::from).unwrap_or_else(|| torrent_root.clone());
                let extract = tauri::async_runtime::spawn_blocking(move || {
                    super::games::extract_game_zip(&zip, &dest)
                })
                .await
                .map_err(|e| format!("extraction task failed: {e}"))?;
                if let Err(e) = extract {
                    // `zip.exists()` is true for almost the whole collection:
                    // librqbit allocates a 0-byte placeholder per torrent file,
                    // and a neighbouring download leaves piece-sized fragments
                    // (measured on this pack: 620 placeholders + 29 fragments
                    // of 664). So the "files not found" arm below is nearly
                    // unreachable and an undownloaded game arrives HERE, as a
                    // zip that won't open. Say so, and clear `installed` so the
                    // next click offers a download instead of repeating this.
                    let msg = e.to_string();
                    if msg.contains("EOCD")
                        || msg.contains("invalid Zip")
                        || msg.contains("Invalid archive")
                    {
                        if let Ok(conn) = app.state::<super::DbState>().0.lock() {
                            let _ = crate::db::queries::set_game_installed(&conn, id, false);
                        }
                        return Err(format!(
                            "Game ZIP for '{}' is incomplete or corrupted (torrent placeholder). \
                             Please re-download the game.",
                            game.title
                        ));
                    }
                    return Err(format!("Failed to extract game before launch: {msg}"));
                }
            } else {
                return Err(format!(
                    "Game files not found for '{}'. The game may need to be re-downloaded.",
                    game.title
                ));
            }
        }
    }

    // Working dir is <torrent_root>/eXo - every relative path in the confs
    // and eXo's own launch bats resolves from there.
    let exo_dir = torrent_root.join("eXo");

    if variant.starts_with("86box") {
        launch_86box(
            app,
            game,
            id,
            data_dir,
            &torrent_root,
            &exo_dir,
            &conf_dir,
            variant,
            fullscreen,
        )
    } else {
        launch_dosbox_x(
            app,
            game,
            id,
            data_dir,
            &torrent_root,
            &exo_dir,
            &conf_dir,
            fullscreen,
            per_game_config,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn launch_dosbox_x(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    torrent_root: &Path,
    exo_dir: &Path,
    conf_dir: &Path,
    fullscreen: bool,
    per_game_config: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    let Some(engine) = resolve_dosbox_x(app, torrent_root, data_dir) else {
        return Err(if cfg!(target_os = "linux") {
            "DOSBox-X is required for Windows 9x games but is not installed. \
             Download it from this game's page or Settings → Content Packs, \
             or install it via your package manager / Flatpak \
             (com.dosbox_x.DOSBox-X)."
                .to_string()
        } else {
            "DOSBox-X is required for Windows 9x games but is not installed. \
             Download it from this game's page or Settings → Content Packs."
                .to_string()
        });
    };

    let play_conf = find_file_ci(conf_dir, "play.conf")
        .ok_or_else(|| format!("play.conf not found in {}", conf_dir.display()))?;
    let options_conf = exo_dir.join("emulators/dosbox/options9x.conf");
    let base_conf = exo_dir.join("emulators/dosbox/x98/dosbox-x.conf");

    // One narrow exception to "the conf runs verbatim": `.\`-relative HOST
    // path tokens are rewritten to `./` form. DOSBox-X on POSIX opens
    // existing files through backslash paths fine, but CANNOT CREATE them -
    // `vhdmake` silently wrote nothing, every boot reused the shipped, dirty
    // child VHD, and games whose child isn't shipped at all (W95-C.vhd)
    // booted "Invalid system disk". Guest text is untouched - that is
    // rewrite_host_paths' contract (see the Win3x PATH lesson). A token is
    // only rewritten when its target (or, for files vhdmake will create, its
    // parent directory) exists under eXo/.
    let play_conf = {
        let content = std::fs::read_to_string(&play_conf)
            .map_err(|e| format!("Failed to read {}: {}", play_conf.display(), e))?;
        let patched = super::games::rewrite_host_paths(&content, &|body| {
            let fwd = body.replace('\\', "/");
            let target = exo_dir.join(&fwd);
            let creatable = target.parent().is_some_and(|p| p.is_dir());
            if target.exists() || creatable {
                format!("./{}", fwd)
            } else {
                format!(".\\{}", body)
            }
        });
        let patched = extract_zip_mounts(&patched, exo_dir);
        let patched_path =
            super::games::launch_conf_dir(app)?.join(format!("win9x_play_{}.conf", id));
        std::fs::write(&patched_path, &patched)
            .map_err(|e| format!("Failed to write patched play.conf: {e}"))?;
        patched_path
    };

    let (mut cmd, bin) = engine.command(torrent_root);
    cmd.current_dir(exo_dir);
    // eXo's own x98 exe runs in portable mode and auto-loads the base conf
    // sitting next to it; any other build needs it passed explicitly, FIRST,
    // so play.conf layers on top exactly as authored.
    let is_exo_build = matches!(
        &engine,
        EngineCmd::Direct(b) if b.starts_with(exo_dir)
    );
    if !is_exo_build && base_conf.exists() {
        cmd.arg("-conf").arg(&base_conf);
    }
    cmd.arg("-conf").arg(&play_conf);
    if options_conf.exists() {
        cmd.arg("-conf").arg(&options_conf);
    }

    // User preference overrides, applied last so they win. DOSBox-X shares
    // the [sdl] fullscreen key with vanilla DOSBox; glshader does not apply.
    // - windowresolution: eXo's options9x.conf default (1280x960) is in
    //   logical points and overflows a 1117-point MacBook screen with the
    //   image partly cut off; 1024x768 fits every common display.
    // - output opengl: the base conf's ttf/outputswitch combo is not
    //   user-resizable; opengl windows scale by dragging.
    // - ne2000 backend: see `ne2000_override`.
    let mut frag = format!(
        "[sdl]\nfullscreen = {}\nwindowresolution = 1024x768\noutput = opengl\n{}",
        fullscreen,
        ne2000_override()
    );
    if let Some(custom) = per_game_config.get("custom_conf") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            frag.push('\n');
            frag.push_str(trimmed);
            frag.push('\n');
        }
    }
    let frag_path = super::games::launch_conf_dir(app)?.join(format!("win9x_overrides_{}.conf", id));
    std::fs::write(&frag_path, &frag).map_err(|e| format!("Failed to write override conf: {e}"))?;
    cmd.arg("-conf").arg(&frag_path);

    // eXo's bats pass -nomenu; we deliberately keep the menu. DOSBox-X
    // 2025.02.01 renders the guest at a FIXED size and crops when the window
    // is dragged smaller (measured on macOS with opengl, surface and
    // openglpp alike - upstream issue #3661), so the Video menu is the only
    // runtime escape hatch: fullscreen scales correctly, and the output mode
    // can be switched to `surface`, which centres the whole guest screen
    // instead of cropping it. On macOS the menu lives in the global menu bar
    // and costs no window space.
    if cfg!(windows) {
        cmd.arg("-noconsole");
    }
    // Same belt-and-braces as launch_86box: the pack's own AppImage uses
    // uruntime (no FUSE needed, var ignored), but a user-supplied AppImage on
    // PATH may not.
    if cfg!(target_os = "linux") {
        cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    }

    log::info!(
        "Launching Win9x game {} via DOSBox-X ({})",
        game.title,
        bin.display()
    );
    super::games::spawn_emulator_and_track(app, cmd, &bin, &game, id)
}

#[allow(clippy::too_many_arguments)]
fn launch_86box(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    torrent_root: &Path,
    exo_dir: &Path,
    conf_dir: &Path,
    variant: &str,
    fullscreen: bool,
) -> Result<String, String> {
    let Some(engine) = resolve_86box(app, torrent_root, data_dir) else {
        return Err(if cfg!(windows) {
            // Nothing is bundled here, so the extracted support tree is the
            // only source - and it passed the readiness gate, meaning the
            // parent VHDs arrived but 86Box.exe did not.
            "86Box is required for this game but is missing from the Windows 9x \
             support files (expected eXo\\emulators\\86Box98\\86Box.exe). Place \
             86Box on your PATH, or re-download the support files."
                .to_string()
        } else {
            "86Box is required for this game but is not installed. Download it \
             from this game's page or Settings → Content Packs, or place 86Box \
             on your PATH."
                .to_string()
        });
    };

    let emul_dir = exo_dir.join("emulators/86Box98");
    let (child_name, parent_name, cfg_name) = e86box_variant_files(variant);

    // Recreate the disposable C: drive: a fresh differencing child of the
    // shared parent OS image, exactly what eXo's makevhd.exe does per launch.
    // Saves are unaffected - they live on the game's own VHD (drive D:).
    let child = emul_dir.join(child_name);
    if child.exists() {
        std::fs::remove_file(&child).map_err(|e| e.to_string())?;
    }
    let parent = emul_dir.join("parent").join(parent_name);
    crate::vhd::create_differencing(&child, &parent, &format!(r".\parent\{}", parent_name))?;

    // The per-game cfg is copied over the emulator's play.cfg (it references
    // the child VHD and the game's own VHD by relative path).
    let game_cfg = find_file_ci(conf_dir, cfg_name)
        .ok_or_else(|| format!("{} not found in {}", cfg_name, conf_dir.display()))?;
    let active_cfg = emul_dir.join("play.cfg");
    std::fs::copy(&game_cfg, &active_cfg).map_err(|e| e.to_string())?;

    let (mut cmd, bin) = engine.command(exo_dir);
    cmd.current_dir(exo_dir)
        .arg("-c")
        .arg(&active_cfg)
        // vmpath: where 86Box keeps/finds nvr state and (since v4) also
        // looks for roms/ - the extracted 86Box98 tree ships both.
        .arg("-P")
        .arg(&emul_dir);
    if fullscreen {
        cmd.arg("-F");
    }
    // AppImages need FUSE; the env var makes them self-extract when the
    // host has none (common in containers/minimal distros).
    if cfg!(target_os = "linux") {
        cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    }

    log::info!(
        "Launching Win9x game {} via 86Box ({}, variant {})",
        game.title,
        bin.display(),
        variant
    );
    super::games::spawn_emulator_and_track(app, cmd, &bin, &game, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_zip(path: &Path, entries: &[&str]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for name in entries {
            if name.ends_with('/') {
                zip.add_directory(name.trim_end_matches('/'), opts).unwrap();
            } else {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(b"x").unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn zip_mounts_become_directory_mounts() {
        let dir = std::env::temp_dir().join(format!("exodium_zipmount_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // One top-level dir: the game's files sit one level too deep for the
        // desktop shortcut, so the mount is redirected at the inner dir.
        let wrapped = dir.join("CC32.zip");
        write_zip(&wrapped, &["CC32/", "CC32/CCHECK11.EXE"]);
        let out = extract_zip_mounts(&format!("MOUNT e \"{}\"", wrapped.display()), &dir);
        let expected = format!("{}\"", dir.join("CC32.exodium_mount").join("CC32").display());
        assert!(out.ends_with(&expected), "{out}");
        assert!(dir.join("CC32.exodium_mount/CC32/CCHECK11.EXE").is_file());

        // A zip laid out eXo's way is extracted too - mounting the zip
        // itself is what crashes DOSBox-X on exit - but keeps its root layout.
        let flat = dir.join("MpgDec20.zip");
        write_zip(&flat, &["license.txt", "MPGDEC.DLL"]);
        let out = extract_zip_mounts(&format!("MOUNT e \"{}\"", flat.display()), &dir);
        assert!(out.ends_with("MpgDec20.exodium_mount\""), "{out}");
        assert!(dir.join("MpgDec20.exodium_mount/MPGDEC.DLL").is_file());

        // Non-zip mounts and other lines are never touched.
        let conf = "IMGMOUNT c ./x.vhd\nMOUNT e \"./games\"\nBOOT -l c";
        assert_eq!(extract_zip_mounts(conf, &dir), conf);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Win9xSupportStatus {
    /// "ready" | "downloading" | "missing" | "failed"
    pub phase: String,
    /// Download progress 0..1 while phase == "downloading".
    pub progress: f32,
    /// Size of utilWin9x.zip - lets the panel say what the one-time
    /// support download costs. 0 when the torrent index is unavailable.
    pub total_bytes: u64,
}

/// Whether the emulator a Win9x game needs is resolvable on this machine.
/// The launcher's own resolver answers, so the note in the detail panel can
/// never disagree with what launch would actually do. Mainly a Linux
/// concern: DOSBox-X has no official Linux binaries, so PATH/Flatpak may
/// genuinely be empty there.
#[tauri::command]
pub async fn win9x_engine_available(
    app: AppHandle,
    db_state: State<'_, super::DbState>,
    variant: Option<String>,
) -> Result<bool, String> {
    let variant = variant.unwrap_or_else(|| "x98".to_string());
    if variant == "pcbox" {
        return Ok(false);
    }
    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        crate::db::queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
    };
    let torrent_root = crate::commands::setup::game_root(&data_dir);
    Ok(win9x_engine_resolvable(&app, &torrent_root, &data_dir, Some(variant.as_str())))
}

/// Support-file state for the detail panel: lets it show "Windows 9x support
/// files still downloading (N%)" instead of a bare launch failure.
///
/// `variant` scopes the readiness check to the tree that game actually
/// boots from - the same scoping `download_game`'s queue gate uses. Without
/// it, an x98-ready/86Box-missing install reads "missing" for x98 games
/// whose download would never fetch anything.
#[tauri::command]
pub async fn get_win9x_support_status(
    torrent_state: State<'_, TorrentState>,
    variant: Option<String>,
) -> Result<Win9xSupportStatus, String> {
    let mgr = {
        let guard = torrent_state.0.read().await;
        guard.get("eXoWin9x").cloned()
    };
    let Some(mgr) = mgr else {
        return Ok(Win9xSupportStatus { phase: "missing".into(), progress: 0.0, total_bytes: 0 });
    };
    let root = mgr.torrent_root();
    let ready = match variant.as_deref() {
        Some(v) => win9x_support_ready(&root, Some(v)),
        None => win9x_support_ready(&root, None) && win9x_support_ready(&root, Some("86box")),
    };
    if ready {
        return Ok(Win9xSupportStatus { phase: "ready".into(), progress: 1.0, total_bytes: 0 });
    }
    let Some(util) = mgr.index().find_by_suffix("util/utilWin9x.zip") else {
        return Ok(Win9xSupportStatus { phase: "missing".into(), progress: 0.0, total_bytes: 0 });
    };
    if WIN9X_EXTRACTION_FAILED.load(Ordering::SeqCst) {
        return Ok(Win9xSupportStatus {
            phase: "failed".into(),
            progress: 1.0,
            total_bytes: util.size,
        });
    }
    if mgr.is_file_selected(util.index).await {
        let on_disk = mgr
            .file_output_path(util.index)
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        // Torrent pieces land out of order, so the sparse file's length
        // reaches full size long before the download is done - only a
        // verified-complete file may claim 100% (= "setting up" in the UI).
        let progress = if mgr.is_file_complete(util.index).await {
            1.0
        } else if util.size > 0 {
            (on_disk as f32 / util.size as f32).min(0.99)
        } else {
            0.0
        };
        return Ok(Win9xSupportStatus {
            phase: "downloading".into(),
            progress,
            total_bytes: util.size,
        });
    }
    Ok(Win9xSupportStatus { phase: "missing".into(), progress: 0.0, total_bytes: util.size })
}
