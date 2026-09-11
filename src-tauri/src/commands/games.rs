use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;


use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::db::queries;
use crate::models::Game;

use super::TorrentState;
use super::paths::launch_conf_dir;
use super::collections::{collection_game_prefix, collection_lang_dir};
use super::install::{copy_dir_recursive, extract_before_launch};


pub struct DbState(pub Mutex<Connection>);

impl DbState {
    /// The connection guard; a poisoned mutex surfaces as a command error.
    pub fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.0.lock().map_err(|e| e.to_string())
    }
}

/// The configured data dir; setup has not run when it is missing.
pub(crate) fn configured_data_dir(conn: &Connection) -> Result<String, String> {
    queries::get_config(conn, "data_dir")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Data directory not configured. Run setup first.".to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct GameList {
    pub games: Vec<Game>,
    pub total: usize,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn get_games(
    state: State<'_, DbState>,
    page: Option<usize>,
    per_page: Option<usize>,
    query: Option<String>,
    genre: Option<String>,
    sort_by: Option<String>,
    collection: Option<String>,
    favorites_only: Option<bool>,
    playlist_id: Option<i64>,
    with_music: Option<bool>,
) -> Result<GameList, String> {
    let conn = state.lock()?;
    let page = page.unwrap_or(1);
    let per_page = per_page.unwrap_or(50).min(10000);
    let query = query.unwrap_or_default();
    let genre = genre.unwrap_or_default();
    let sort_by = sort_by.unwrap_or_default();
    let collection = collection.unwrap_or_default();

    let f = queries::GameFilter {
        query: &query,
        genre: &genre,
        sort_by: &sort_by,
        collection: &collection,
        favorites_only: favorites_only.unwrap_or(false),
        playlist_id,
        with_music: with_music.unwrap_or(false),
    };

    let total = queries::count_games_filtered(&conn, &f).map_err(|e| e.to_string())?;
    let games = queries::fetch_games_filtered(&conn, page, per_page, &f).map_err(|e| e.to_string())?;

    Ok(GameList { games, total })
}

#[tauri::command]
pub async fn get_genres(state: State<'_, DbState>, collection: Option<String>) -> Result<Vec<String>, String> {
    let conn = state.lock()?;
    let collection = collection.unwrap_or_default();
    queries::get_genres(&conn, &collection).map_err(|e| e.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn get_section_keys(
    state: State<'_, DbState>,
    sort_by: Option<String>,
    query: Option<String>,
    genre: Option<String>,
    collection: Option<String>,
    favorites_only: Option<bool>,
    playlist_id: Option<i64>,
    with_music: Option<bool>,
) -> Result<Vec<String>, String> {
    let conn = state.lock()?;
    let sort_by = sort_by.unwrap_or_default();
    let query = query.unwrap_or_default();
    let genre = genre.unwrap_or_default();
    let collection = collection.unwrap_or_default();
    let f = queries::GameFilter {
        query: &query,
        genre: &genre,
        sort_by: &sort_by,
        collection: &collection,
        favorites_only: favorites_only.unwrap_or(false),
        playlist_id,
        with_music: with_music.unwrap_or(false),
    };
    let result = queries::get_section_keys(&conn, &f).map_err(|e| e.to_string());
    log::debug!("get_section_keys: sort_by={:?} collection={:?} → {:?} keys", sort_by, collection, result.as_ref().map(|v| v.len()));
    result
}

#[tauri::command]
pub async fn get_game_variants(
    state: State<'_, DbState>,
    shortcode: String,
    collection: String,
) -> Result<Vec<Game>, String> {
    let conn = state.lock()?;
    let mut games =
        queries::fetch_game_variants(&conn, &shortcode, &collection).map_err(|e| e.to_string())?;
    // An overlay variant is installed as English-plus-patch, so its price is
    // both archives. The stored size is the patch plus the shared GameData.
    let mut dependents_by_base: Vec<(i64, String)> = Vec::new();
    for game in &games {
        let names: Vec<String> = crate::commands::lp_overlay::dependents_of(&conn, game, true)
            .into_iter()
            .map(|g| g.title)
            .collect();
        if let (Some(id), false) = (game.id, names.is_empty()) {
            dependents_by_base.push((id, names.join(", ")));
        }
    }
    for game in &mut games {
        if let Some(base) = crate::commands::lp_overlay::base_archive_size(&conn, game) {
            game.requires_base = true;
            game.download_size = Some(game.download_size.unwrap_or(0) + base as i64);
        }
        game.installed_with = dependents_by_base
            .iter()
            .find(|(id, _)| Some(*id) == game.id)
            .map(|(_, names)| names.clone());
    }
    Ok(games)
}

#[tauri::command]
pub async fn get_installed_games(state: State<'_, DbState>) -> Result<Vec<Game>, String> {
    let conn = state.lock()?;
    queries::fetch_installed_games(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_favorite(state: State<'_, DbState>, id: i64) -> Result<bool, String> {
    let conn = state.lock()?;
    queries::toggle_favorite(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_game(state: State<'_, DbState>, id: i64) -> Result<Option<Game>, String> {
    let conn = state.lock()?;
    queries::fetch_game_by_id(&conn, id).map_err(|e| e.to_string())
}


#[tauri::command]
pub async fn get_config(state: State<'_, DbState>, key: String) -> Result<Option<String>, String> {
    let conn = state.lock()?;
    queries::get_config(&conn, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_config(
    app: tauri::AppHandle,
    state: State<'_, DbState>,
    key: String,
    value: String,
) -> Result<(), String> {
    {
        let conn = state.lock()?;
        queries::set_config(&conn, &key, &value).map_err(|e| e.to_string())?;
    }
    // The asset-protocol scope must cover the user-chosen data dir (covers,
    // screenshots, manuals); lib.rs grants the stored value at startup.
    if key == "data_dir" {
        crate::allow_asset_dir(&app, std::path::Path::new(&value));
    }
    Ok(())
}

/// Open a game file in the system viewer. Only paths under the data dir are
/// allowed; the webview has no opener capability of its own.
#[tauri::command]
pub async fn open_manual(
    app: AppHandle,
    state: State<'_, DbState>,
    path: String,
) -> Result<(), String> {
    let data_dir = {
        let conn = state.lock()?;
        configured_data_dir(&conn)?
    };
    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| format!("Cannot open '{}': {}", path, e))?;
    let base = std::fs::canonicalize(&data_dir).map_err(|e| e.to_string())?;
    if !canonical.starts_with(&base) {
        return Err(format!("Refusing to open path outside the data directory: {}", path));
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(canonical.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Can this install apply updates via the tauri updater? Linux deb/rpm
/// installs cannot (AppImage-only there) - the frontend suppresses the
/// update pill for them instead of offering a download that can't install.
#[tauri::command]
pub fn update_check_supported() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var("APPIMAGE").is_ok()
    } else {
        true
    }
}

/// Toggle seeding (uploading to the swarm). Persists the choice and applies
/// it live to the shared torrent session.
#[tauri::command]
pub async fn set_seeding_enabled(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    enabled: bool,
) -> Result<(), String> {
    {
        let conn = db_state.lock()?;
        queries::set_config(&conn, "seeding_enabled", if enabled { "1" } else { "0" })
            .map_err(|e| e.to_string())?;
    }
    apply_stored_limits(&db_state, &torrent_state).await;
    Ok(())
}

/// Set the user's transfer caps in KB/s; `None` (or 0 from the UI) means
/// unlimited. Persisted and applied live.
#[tauri::command]
pub async fn set_rate_limits(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    up_kbps: Option<u32>,
    down_kbps: Option<u32>,
) -> Result<(), String> {
    {
        let conn = db_state.lock()?;
        // Store "" for unlimited rather than deleting the row: the reader
        // treats unparseable and absent alike, and a present key documents
        // that the user has been here.
        let write = |key: &str, v: Option<u32>| {
            queries::set_config(&conn, key, &v.map_or(String::new(), |k| k.to_string()))
        };
        write("rate_limit_up_kbps", up_kbps).map_err(|e| e.to_string())?;
        write("rate_limit_down_kbps", down_kbps).map_err(|e| e.to_string())?;
    }
    apply_stored_limits(&db_state, &torrent_state).await;
    Ok(())
}

/// Seeding and rate caps share one librqbit knob, so both are re-applied
/// together (`DownloadManager::apply_limits`). No manager: nothing to do,
/// a new session reads the preferences itself.
async fn apply_stored_limits(db_state: &State<'_, DbState>, torrent_state: &State<'_, TorrentState>) {
    let seeding = crate::commands::setup::seeding_enabled(&db_state.0);
    let (up, down) = crate::commands::setup::rate_limits(&db_state.0);
    // All managers share one session - applying via any of them is enough.
    let mgr = { torrent_state.0.read().await.values().next().cloned() };
    if let Some(mgr) = mgr {
        mgr.apply_limits(seeding, up, down);
    }
}

/// Live transfer figures shown in the network badge.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TransferStats {
    pub download_bps: u64,
    pub upload_bps: u64,
    /// Uploaded since the session started - librqbit keeps no lifetime total.
    pub uploaded_bytes: u64,
    /// Peers currently connected. The readout that answers "is anything
    /// happening" while the rates sit at zero: connections are a standing
    /// state, transfer is event-driven.
    pub peers: u32,
    /// False when no torrent is live anywhere - the difference between "idle"
    /// and "nothing running", which the badge shows differently.
    pub active: bool,
}

/// Session-wide transfer rates: one read, and a peer serving two collections
/// is not counted twice.
#[tauri::command]
pub async fn get_transfer_stats(torrent_state: State<'_, TorrentState>) -> Result<TransferStats, String> {
    let managers: Vec<_> = { torrent_state.0.read().await.values().cloned().collect() };
    let Some(first) = managers.first() else {
        return Ok(TransferStats::default());
    };
    let t = first.session_transfer();
    // Liveness is per torrent, so it still needs every manager: the session
    // exists in offline-to-online transitions before any torrent is running.
    let mut active = false;
    for mgr in &managers {
        if mgr.status().await.live {
            active = true;
            break;
        }
    }
    Ok(TransferStats {
        download_bps: t.download_bps,
        upload_bps: t.upload_bps,
        uploaded_bytes: t.uploaded_bytes,
        peers: t.peers,
        active,
    })
}


/// Currently-running games (keyed by shortcode:language, or id when no
/// shortcode exists). Inserted at spawn, removed by the reaper task when
/// DOSBox exits. Lets uninstall refuse while the game's files are open.
pub(crate) fn running_games() -> &'static Mutex<std::collections::HashSet<String>> {
    static RUNNING: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Emulator pid per launched game id, so "Stop game" can end it. Inserted at
/// spawn, removed by the reaper - a stale entry can therefore never outlive
/// the process by more than the reaper's turnaround.
pub(crate) fn running_pids() -> &'static Mutex<std::collections::HashMap<i64, u32>> {
    static PIDS: OnceLock<Mutex<std::collections::HashMap<i64, u32>>> = OnceLock::new();
    PIDS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// End a running game's emulator, Steam-style. SIGTERM on Unix (Staging and
/// DOSBox-X shut down cleanly on it; for a booted Win9x guest this is the
/// same as closing the window - the Start-menu shutdown stays the clean way,
/// §5); `taskkill /T` on Windows so a bat-launched chain goes with it. The
/// reaper notices the exit and emits `game-exited` as for any other quit.
#[tauri::command]
pub async fn stop_game(id: i64) -> Result<(), String> {
    let pid = running_pids()
        .lock()
        .map_err(|e| e.to_string())?
        .get(&id)
        .copied()
        .ok_or("Game is not running")?;
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if rc != 0 {
            return Err(format!("Could not stop the emulator (pid {})", pid));
        }
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!(
                "Could not stop the emulator (pid {}): {}",
                pid,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    log::info!("Stopped game {} (pid {})", id, pid);
    Ok(())
}

pub(crate) fn running_game_key(game: &Game) -> String {
    match game.shortcode.as_deref() {
        Some(sc) => format!("{}:{}", sc, game.language),
        None => format!("id:{}", game.id.unwrap_or(-1)),
    }
}

/// Per-game mutual exclusion for launch, uninstall and download: an uninstall
/// during launch-time extraction would rename the dir under the extractor.
pub(crate) fn game_op_lock(id: i64) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let map = LOCKS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut map = map.lock().expect("game-op lock map poisoned");
    std::sync::Arc::clone(
        map.entry(id)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(()))),
    )
}


/// The conf enables eXo's virtual printer (`printer=true` or
/// `parallel1=printer`). Comment lines are skipped: one eXoWin3x conf quotes
/// the whole option documentation while disabling the port.
fn conf_requests_printer(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .any(|l| {
            let lower = l.to_ascii_lowercase();
            match lower.split_once('=') {
                Some((k, v)) => {
                    let (k, v) = (k.trim(), v.trim());
                    (k == "printer" && v == "true")
                        || (k.starts_with("parallel") && v.starts_with("printer"))
                }
                None => false,
            }
        })
}

/// Locate a game's dosbox.conf under the game root: the catalogue path
/// first, then the lang-scoped variants (LP rows inherit the EN path).
/// Shared by `launch_game` and `game_printing_unavailable`; the returned
/// root is `launch_game`'s working-dir base.
fn resolve_game_conf(data_dir: &str, dosbox_conf: &str) -> Option<(PathBuf, PathBuf)> {
    let rel = dosbox_conf.replace('\\', "/");
    let root = crate::commands::paths::game_root(data_dir);

    let direct = root.join(&rel);
    if direct.exists() {
        return Some((direct, root));
    }

    let prefix = collection_game_prefix("eXoDOS");
    let segment = crate::commands::collections::collection_def("eXoDOS")
        .map(|c| c.shortcode_segment)
        .unwrap_or("!dos");
    let shortcode = rel
        .strip_suffix("/dosbox.conf")
        .and_then(|p| p.rsplit('/').next())
        .filter(|s| !s.is_empty())?;
    for lang_dir in crate::commands::collections::COLLECTION_MAP.iter().filter_map(|c| c.lang_dir) {
        let alt = root.join(format!("{prefix}/{segment}/{lang_dir}/{shortcode}/dosbox.conf"));
        if alt.exists() {
            return Some((alt, root));
        }
    }
    None
}

/// Which engine runs a DOS game: `Some(path)` is eXo's DOSBox ECE build,
/// `None` is DOSBox Staging. The ONLY place that decides this - launch, the
/// panel label and the printing/shader notes all ask here (§9).
/// `engine = staging` in the game's config forces Staging (ECE has no shaders).
pub(crate) fn resolve_engine(
    dosbox_variant: Option<&str>,
    main_torrent_root: &std::path::Path,
    per_game_config: &std::collections::HashMap<String, String>,
) -> Option<PathBuf> {
    if per_game_config.get("engine").map(String::as_str) == Some("staging") {
        return None;
    }
    resolve_ece_binary(dosbox_variant, main_torrent_root)
}

/// The DOSBox ECE build eXo ships for this variant, when it is actually
/// runnable here: Windows only, and only once extracted from util.zip. None
/// means DOSBox Staging will run the game.
fn resolve_ece_binary(
    dosbox_variant: Option<&str>,
    main_torrent_root: &std::path::Path,
) -> Option<PathBuf> {
    let variant = dosbox_variant?;
    if !variant.starts_with("ece") || !cfg!(windows) {
        return None;
    }
    let base = main_torrent_root.join("eXo/emulators/dosbox");
    [
        base.join(variant).join("DOSBox.exe"),
        base.join("ece4230").join("DOSBox.exe"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

/// The two engine facts the UI needs, which are NOT the same question.
#[derive(Debug, serde::Serialize)]
pub struct GameEngineInfo {
    /// ECE could run this game here (platform + build on disk), override
    /// ignored - otherwise picking Staging would hide the way back.
    pub ece_available: bool,
    /// What will ACTUALLY run it, override included. Drives the engine label,
    /// the shader note and the printing note.
    pub uses_ece: bool,
}

/// Which engine this game gets and whether there is a choice. Answered by
/// `resolve_engine`, so it is false on Windows until the ECE build is on disk.
#[tauri::command]
pub async fn game_engine_info(
    db_state: State<'_, DbState>,
    id: i64,
) -> Result<GameEngineInfo, String> {
    let (dosbox_variant, data_dir, per_game_config) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
        let cfg = queries::get_all_game_config(&conn, id).unwrap_or_default();
        (game.dosbox_variant, data_dir, cfg)
    };
    let Some(data_dir) = data_dir else {
        return Ok(GameEngineInfo { ece_available: false, uses_ece: false });
    };
    let main_root = crate::commands::paths::game_root(&data_dir);
    Ok(GameEngineInfo {
        ece_available: resolve_engine(dosbox_variant.as_deref(), &main_root, &Default::default())
            .is_some(),
        uses_ece: resolve_engine(dosbox_variant.as_deref(), &main_root, &per_game_config)
            .is_some(),
    })
}

/// The conf requests eXo's virtual printer AND the engine that would run is
/// Staging, which has no printer emulation. Decided by `resolve_engine`.
#[tauri::command]
pub async fn game_printing_unavailable(
    db_state: State<'_, DbState>,
    id: i64,
) -> Result<bool, String> {
    let (dosbox_conf, dosbox_variant, data_dir, per_game_config) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
        let cfg = queries::get_all_game_config(&conn, id).unwrap_or_default();
        (game.dosbox_conf, game.dosbox_variant, data_dir, cfg)
    };
    let (Some(conf), Some(data_dir)) = (dosbox_conf, data_dir) else {
        return Ok(false);
    };
    let Some((conf_path, _)) = resolve_game_conf(&data_dir, &conf) else {
        return Ok(false);
    };
    let Ok(text) = std::fs::read_to_string(conf_path) else {
        return Ok(false);
    };
    if !conf_requests_printer(&text) {
        return Ok(false);
    }
    let main_root =
        crate::commands::paths::game_root(&data_dir);
    Ok(resolve_engine(dosbox_variant.as_deref(), &main_root, &per_game_config).is_none())
}

/// Create a directory link (symlink on Unix, junction on Windows - junctions
/// need no admin rights or developer mode).
#[cfg(unix)]
fn link_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}
#[cfg(windows)]
fn link_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    junction::create(src, dst)
}

/// Per-launch overlay root for an LP game (§10a): `<shortcode>` links to the
/// LP game dir, other root entries the autoexec names link to the real tree.
/// Rebuilt every launch; holds links only.
fn build_lp_overlay(
    working_dir: &std::path::Path,
    game_folder: &str,
    shortcode: &str,
    lang_dir: &str,
    lp_game_dir: &std::path::Path,
    en_conf: &str,
) -> Result<PathBuf, String> {
    let staging = working_dir
        .join(".exodium_lp")
        .join(format!("{}_{}", lang_dir.trim_start_matches('!'), shortcode));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .map_err(|e| format!("clearing {}: {}", staging.display(), e))?;
    }
    std::fs::create_dir_all(&staging).map_err(|e| format!("creating {}: {}", staging.display(), e))?;
    link_dir(lp_game_dir, &staging.join(shortcode))
        .map_err(|e| format!("linking {}: {}", shortcode, e))?;

    // Pass-through links for other referenced root entries.
    let real_root = working_dir.join(game_folder);
    let needle = format!("{}\\", game_folder);
    let autoexec = en_conf.split("[autoexec]").nth(1).unwrap_or("");
    for (idx, _) in autoexec.match_indices(&needle) {
        let rest = &autoexec[idx + needle.len()..];
        let entry: String = rest
            .chars()
            .take_while(|c| !"\\/\" \t\r\n".contains(*c))
            .collect();
        if entry.is_empty() || entry.eq_ignore_ascii_case(shortcode) {
            continue;
        }
        let dst = staging.join(&entry);
        let src = real_root.join(&entry);
        if !dst.exists() && src.exists() {
            if let Err(e) = link_dir(&src, &dst) {
                log::warn!("LP overlay: pass-through link {} failed: {}", entry, e);
            }
        }
    }
    Ok(staging)
}

/// Can the EN autoexec run against the LP dir through the overlay? Simulates
/// the cd chain and requires the launch command's program to exist there.
fn lp_autoexec_compatible(
    en_conf: &str,
    shortcode: &str,
    lp_game_dir: &std::path::Path,
    real_root: &std::path::Path,
) -> bool {
    let Some(autoexec) = en_conf.split("[autoexec]").nth(1) else {
        return false;
    };
    // cwd: None = the C: mount root. `mount c <target>` sets it - eXo's
    // confs mount the game directory itself, so a following `cd sub` is
    // relative to that, not to the staging dir the paths are rewritten to.
    let mut cwd: Option<PathBuf> = None;
    let mut mount_root: Option<PathBuf> = None;
    for line in autoexec.lines() {
        let t = line.trim();
        let t = t.strip_prefix('@').unwrap_or(t).trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let lower = t.to_ascii_lowercase();

        if lower == "cd" || lower == "cd." || lower == "cd.." {
            continue;
        }
        let cd_target = if let Some(r) = lower.strip_prefix("cd ") {
            Some(r)
        } else if let Some(r) = lower.strip_prefix("cd\\") {
            // "cd\FOO" is an absolute path from the mount root.
            cwd = None;
            Some(r)
        } else {
            None
        };
        if let Some(target) = cd_target {
            let target = target.trim().trim_matches('"');
            if target.is_empty() || target == "\\" || target == "/" || target == ".." {
                cwd = None;
                continue;
            }
            let next = match (&cwd, &mount_root) {
                (None, Some(root)) => root.join(target),
                (None, None) => {
                    if target.eq_ignore_ascii_case(shortcode) {
                        lp_game_dir.to_path_buf()
                    } else {
                        // Root-level cd into a non-game entry resolves
                        // through a pass-through link to the real tree.
                        real_root.join(target)
                    }
                }
                (Some(dir), _) => dir.join(target),
            };
            if !next.exists() {
                log::info!(
                    "LP launch: EN autoexec cd target '{}' missing under LP layout",
                    target
                );
                return false;
            }
            cwd = Some(next);
            continue;
        }

        // `mount c <host path>` decides what C:\ is. Only the form that
        // mounts the game's OWN directory is read here; every other target
        // (eXo's confs often mount the collection root) leaves the staging
        // dir as the root, which is what `cwd = None` already means.
        if let Some(rest) = lower.strip_prefix("mount c ") {
            let target = rest.trim().trim_matches('"').trim_end_matches(['\\', '/']);
            let leaf = target.rsplit(['\\', '/']).next().unwrap_or(target);
            if leaf.eq_ignore_ascii_case(shortcode) {
                mount_root = Some(lp_game_dir.to_path_buf());
            }
            cwd = None;
            continue;
        }

        // Housekeeping lines that never launch anything.
        let is_drive_switch = lower.len() == 2
            && lower.as_bytes()[1] == b':'
            && lower.as_bytes()[0].is_ascii_alphabetic();
        if is_drive_switch
            || ["mount ", "imgmount ", "echo ", "rem ", "set ", "config "]
                .iter()
                .any(|p| lower.starts_with(p))
            || ["cls", "exit", "pause", "echo", "echo."].contains(&lower.as_str())
        {
            continue;
        }

        // First real command is the launch line. Forms this cannot check
        // (boot images, drive-letter paths) are trusted.
        if lower == "boot" || lower.starts_with("boot ") {
            return true;
        }
        let base = t
            .strip_prefix("call ")
            .or_else(|| t.strip_prefix("CALL "))
            .or_else(|| t.strip_prefix("loadfix "))
            .unwrap_or(t);
        // Skip option tokens ("loadfix -32 game.exe") before picking the program.
        let base = base
            .split_whitespace()
            .find(|tok| !tok.starts_with('-'))
            .unwrap_or(base);
        if base.contains(':') || base.contains('\\') || base.contains('/') {
            return true;
        }
        let dir = match (&cwd, &mount_root) {
            (Some(d), _) => d.clone(),
            (None, Some(root)) => root.clone(),
            (None, None) => return true, // command at mount root - rare, trust it
        };
        let base_lower = base.to_ascii_lowercase();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                let stem = name.rsplitn(2, '.').last().unwrap_or(&name);
                if stem == base_lower || name == base_lower {
                    return true;
                }
            }
        }
        log::info!(
            "LP launch: EN launch command '{}' not present in {} - falling back",
            base,
            dir.display()
        );
        return false;
    }
    // No launch command at all (fully commented autoexec): the overlay still
    // works - the caller appends a find_lp_launch command.
    true
}

/// Rewrite eXo's `.\`-relative HOST paths and nothing else. `resolve` gets the
/// token after `.\` and returns the absolute replacement; quoted tokens run to
/// the closing quote. Everything else is GUEST text (`path=C:\;z:\`), which a
/// blanket backslash swap breaks (§15).
pub(crate) fn rewrite_host_paths(text: &str, resolve: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    // Byte offset of the current match in `text`, for the mount-line check.
    let mut consumed = 0usize;
    while let Some(idx) = rest.find(".\\") {
        out.push_str(&rest[..idx]);
        let quoted = out.ends_with('"');
        let tail = &rest[idx + 2..];
        let end = tail
            .find(|c: char| if quoted { c == '"' } else { c.is_whitespace() })
            .unwrap_or(tail.len());
        let replacement = resolve(&tail[..end]);
        // A data dir with spaces needs quotes on mount ARGUMENTS only; a
        // config property takes its value literally, quotes included.
        if !quoted && replacement.contains(' ') && on_mount_line(text, consumed + idx) {
            out.push('"');
            out.push_str(&replacement);
            out.push('"');
        } else {
            out.push_str(&replacement);
        }
        consumed += idx + 2 + end;
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Is the token at `pos` an argument of a `mount`/`imgmount` command?
/// Those are the lines whose host path is parsed as a whitespace-split
/// argument; everything else in a DOSBox conf reads its value verbatim.
fn on_mount_line(text: &str, pos: usize) -> bool {
    let line_start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let head = text[line_start..pos].trim_start().trim_start_matches('@');
    let cmd = head.split_whitespace().next().unwrap_or("");
    cmd.eq_ignore_ascii_case("mount") || cmd.eq_ignore_ascii_case("imgmount")
}

/// Drop trailing separators from a substituted host path: on Windows DOSBox
/// strips a trailing `\` before `stat()`ing a mount target but not `/`, so
/// `mount c .\eXoDOS\` failed silently for 1,570 configs (§10a).
fn trim_trailing_sep(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    // Never shorten a root ("/" or "G:/") into something else.
    if trimmed.is_empty() || trimmed.ends_with(':') {
        path.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Rewrite a conf's `.\`-relative host paths to absolute ones. For LP games
/// (`lp_info`) the EN conf runs verbatim against the overlay mount; only an
/// incompatible LP layout gets a generated autoexec (§10a).
fn patch_dosbox_conf(
    conf_path: &std::path::Path,
    working_dir: &std::path::Path,
    lp_info: Option<(&str, &str, &str, &std::path::Path)>, // (shortcode, lang_dir, game_folder, lp_game_dir)
    // false when launching under DOSBox ECE, which understands the original
    // ECE [midi] keys natively - translating them would break its MIDI.
    translate_for_staging: bool,
) -> Result<PathBuf, String> {
    let content = std::fs::read_to_string(conf_path)
        .map_err(|e| format!("Failed to read {}: {}", conf_path.display(), e))?;

    // Forward slashes even on Windows: DOSBox accepts them on every platform,
    // and it keeps the substituted host path free of backslashes that a later
    // reader could mistake for guest-side DOS text.
    let abs_prefix = format!("{}/", working_dir.to_string_lossy()).replace('\\', "/");
    // A `.\` token is a host path only when the target exists: eXo also
    // writes `.\` GUEST paths after a drive switch (11th Hour's
    // `imgmount d ".\cd\11HDISK1.cue"` means C:\cd on the mounted drive).
    let to_working_dir = |body: &str| {
        // Bare `.\` is guest text for "current directory" (OxydGold passes it
        // as a program argument) - it would resolve to the working dir, which
        // always exists, so the existence gate alone can't catch it.
        if body.is_empty() {
            return ".\\".to_string();
        }
        let resolved = format!("{}{}", abs_prefix, body.replace('\\', "/"));
        if std::path::Path::new(&resolved).exists() {
            trim_trailing_sep(&resolved)
        } else {
            format!(".\\{}", body)
        }
    };

    let patched = if let Some((shortcode, lang_dir, game_folder, game_dir)) = lp_info {
        // Strategy 1: overlay mount - eXo's autoexec runs as written, only
        // WHERE the files live changes. The link shadows an installed EN copy.
        let real_root = working_dir.join(game_folder);
        let overlay = if game_dir.exists()
            && lp_autoexec_compatible(&content, shortcode, game_dir, &real_root)
        {
            build_lp_overlay(working_dir, game_folder, shortcode, lang_dir, game_dir, &content)
                .map_err(|e| log::warn!("LP overlay build failed for {}: {}", shortcode, e))
                .ok()
        } else {
            None
        };

        if let Some(staging) = overlay {
            log::info!(
                "LP launch: overlay mount for {} ({} -> {})",
                shortcode,
                staging.display(),
                game_dir.display()
            );
            let staging_fwd = staging.to_string_lossy().replace('\\', "/");
            // Route eXoDOS-root references through the overlay, everything else
            // to the real working dir. Both only touch `.\`-relative host paths.
            let mut result = rewrite_host_paths(&content, &|body| {
                if let Some(tail) = body.replace('\\', "/").strip_prefix(game_folder) {
                    let staged = format!("{}{}", staging_fwd, tail);
                    if std::path::Path::new(&staged).exists() {
                        return trim_trailing_sep(&staged);
                    }
                }
                to_working_dir(body)
            });

            // If autoexec has no actual launch command (e.g., all commented out with #),
            // append one found by inspecting the LP game directory.
            if !autoexec_has_launch_cmd(&result) {
                log::info!("LP launch: autoexec has no launch cmd, appending find_lp_launch for {}", shortcode);
                if let Some((subdir, cmd)) = find_lp_launch(game_dir, Some(&content)) {
                    // Strip any trailing `exit` so our appended commands aren't skipped.
                    let trimmed = result.trim_end();
                    if trimmed.to_ascii_lowercase().ends_with("exit") {
                        result.truncate(trimmed.len() - "exit".len());
                        result.push('\n');
                    }
                    // The generated command runs from the mount root; enter the
                    // game dir (via the overlay link) first.
                    result.push_str(&format!("cd {}\n", shortcode));
                    if !subdir.is_empty() {
                        result.push_str(&format!("cd {}\n", subdir));
                    }
                    result.push_str("cls\n");
                    result.push_str(&format!("{}\n", cmd));
                    result.push_str("exit\n");
                }
            }
            result
        } else {
            // Strategy 2: Different directory structure - generate custom autoexec
            log::info!("LP launch: generating custom autoexec for {} (redirected path not found)", shortcode);
            let settings = content
                .split("[autoexec]")
                .next()
                .unwrap_or(&content);

            let mut patched = rewrite_host_paths(settings, &to_working_dir);

            let game_dir_abs = game_dir.to_string_lossy();
            patched.push_str("[autoexec]\n");
            patched.push_str(&format!("@mount c \"{}\"\n", game_dir_abs));
            patched.push_str("c:\n");

            // Find the game subdirectory and launch command
            if let Some((subdir, cmd)) = find_lp_launch(game_dir, Some(&content)) {
                if !subdir.is_empty() {
                    patched.push_str(&format!("cd {}\n", subdir));
                }
                patched.push_str("cls\n");
                patched.push_str(&format!("{}\n", cmd));
            }
            patched.push_str("exit\n");
            patched
        }
    } else {
        // EN game: rewrite host paths only - guest-side DOS text stays as authored.
        rewrite_host_paths(&content, &to_working_dir)
    };

    let patched = if translate_for_staging {
        translate_ide_for_staging(&translate_midi_for_staging(&patched))
    } else {
        patched
    };

    // Name the fragment after the game's conf dir: working_dir is SHARED
    // across every game in a collection, and a fixed name let two
    // concurrent launches read each other's patched conf (wrong game boots).
    let tag = conf_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "conf".to_string());
    let patched_path = working_dir.join(format!(".exodium_launch_{}.conf", tag));
    std::fs::write(&patched_path, &patched)
        .map_err(|e| format!("Failed to write patched config: {}", e))?;

    log::debug!("Patched config written to {}", patched_path.display());

    Ok(patched_path)
}

/// ECE's dotted `[midi]` keys (`mt32.romdir`, `fluid.soundfont`) become
/// Staging's `[mt32]` / `[fluidsynth]` sections; `mididevice = default`
/// becomes `auto`. Confs that already carry the sections pass through.
/// Runs after path rewriting, so captured paths are absolute.
fn translate_midi_for_staging(conf: &str) -> String {
    let lower = conf.to_ascii_lowercase();
    let has_ece_keys = lower.contains("mt32.") || lower.contains("fluid.");
    let has_default_device = lower.contains("mididevice");
    if !has_ece_keys && !has_default_device {
        return conf.to_string();
    }
    let has_mt32_section = lower.lines().any(|l| l.trim() == "[mt32]");
    let has_fluid_section = lower.lines().any(|l| l.trim() == "[fluidsynth]");

    let mut romdir: Option<String> = None;
    let mut soundfont: Option<String> = None;
    let mut out: Vec<String> = Vec::with_capacity(conf.lines().count() + 6);
    let mut section = String::new();

    for line in conf.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed.to_ascii_lowercase();
            out.push(line.to_string());
            continue;
        }
        if section == "[midi]" && !trimmed.starts_with('#') {
            if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                if key == "mt32.romdir" {
                    romdir = Some(value.to_string());
                    continue; // drop the ECE key
                }
                if key == "fluid.soundfont" {
                    soundfont = Some(value.to_string());
                    continue;
                }
                if key.starts_with("mt32.") || key.starts_with("fluid.") {
                    continue; // ECE tuning keys with no Staging equivalent
                }
                if key == "mididevice" && value.eq_ignore_ascii_case("default") {
                    out.push("mididevice = auto".to_string());
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }

    if !has_mt32_section {
        if let Some(dir) = romdir {
            out.push(String::new());
            out.push("[mt32]".to_string());
            out.push(format!("romdir = {}", dir));
            if !std::path::Path::new(&dir).exists() {
                log::warn!(
                    "MT-32 ROM dir {} not on disk yet - music will be missing until \
                     the DOSBox support files finish downloading",
                    dir
                );
            }
        }
    }
    if !has_fluid_section {
        if let Some(sf) = soundfont {
            out.push(String::new());
            out.push("[fluidsynth]".to_string());
            out.push(format!("soundfont = {}", sf));
            if !std::path::Path::new(&sf).exists() {
                log::warn!(
                    "Soundfont {} not on disk yet - General MIDI music will be missing \
                     until the DOSBox support files finish downloading",
                    sf
                );
            }
        }
    }

    let mut result = out.join("\n");
    result.push('\n');
    result
}

/// DOSBox-X's `[ide]` section becomes Staging's `-ide` flag on CD imgmounts
/// (slot form `-ide 2m` normalized): a guest booted from an HDD image reaches
/// the CD only through its own ATAPI driver (§15).
fn translate_ide_for_staging(conf: &str) -> String {
    // Comment lines are skipped, same as conf_requests_printer: one eXoWin3x
    // conf carries the whole option documentation as `#` comments.
    let has_ide_section = conf
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .any(|l| l.to_ascii_lowercase().starts_with("[ide"));
    if !has_ide_section {
        return conf.to_string();
    }
    let mut out: Vec<String> = Vec::with_capacity(conf.lines().count());
    for line in conf.lines() {
        let lower = line.to_ascii_lowercase();
        let cmd = lower.trim_start().trim_start_matches('@');
        // Token-wise `-t cdrom|iso` detection - a doubled space between the
        // flag and its value must not hide a CD mount from the translation.
        let toks: Vec<&str> = cmd.split_whitespace().collect();
        let is_cd_imgmount = cmd.starts_with("imgmount")
            && toks
                .windows(2)
                .any(|w| w[0] == "-t" && (w[1] == "cdrom" || w[1] == "iso"));
        if !is_cd_imgmount {
            out.push(line.to_string());
            continue;
        }
        // Standalone flag only: a data dir like `/mnt/games-ide/` must not
        // read as "already present". Lowercasing keeps byte offsets.
        if let Some(pos) = find_ide_flag(&lower) {
            let end = pos + "-ide".len();
            let rest = &line[end..];
            let after_ws = rest.trim_start();
            let token: String = after_ws.chars().take_while(|c| !c.is_whitespace()).collect();
            // DOSBox-X slot argument: "2m", "1s", "2" - short, digit-first.
            let is_slot = !token.is_empty()
                && token.len() <= 2
                && token.chars().next().is_some_and(|c| c.is_ascii_digit());
            if is_slot {
                out.push(format!("{}{}", &line[..end], &after_ws[token.len()..]));
            } else {
                out.push(line.to_string());
            }
        } else {
            out.push(format!("{} -ide", line.trim_end()));
        }
    }
    let mut result = out.join("\n");
    result.push('\n');
    result
}

/// Byte offset of a standalone `-ide` flag token: preceded by whitespace and
/// followed by whitespace or end-of-line. Substring hits inside path segments
/// or filenames (`/mnt/games-ide/`, `T-IDE.iso`) don't count.
fn find_ide_flag(lower: &str) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut start = 0;
    while let Some(p) = lower[start..].find("-ide") {
        let pos = start + p;
        let end = pos + "-ide".len();
        let before_ok = pos > 0 && bytes[pos - 1].is_ascii_whitespace();
        let after_ok = end >= lower.len() || bytes[end].is_ascii_whitespace();
        if before_ok && after_ok {
            return Some(pos);
        }
        start = end;
    }
    None
}

/// Launch command for an LP game whose layout differs from EN: the EN
/// autoexec's command if it exists here, else run.bat's target, else an
/// executable. Returns (subdir, command).
fn find_lp_launch(game_dir: &std::path::Path, en_conf: Option<&str>) -> Option<(String, String)> {
    // Strategy 0: the EN autoexec names the launcher - the only signal that
    // works for a bare root-level EXE without a .bat (Cobra Mission ES).
    if let Some(autoexec) = en_conf.and_then(|c| c.split("[autoexec]").nth(1)) {
        for line in autoexec.lines() {
            let t = line.trim();
            let t = t.strip_prefix('@').unwrap_or(t).trim();
            let t = t
                .strip_prefix("call ")
                .or_else(|| t.strip_prefix("CALL "))
                .unwrap_or(t)
                .trim();
            if t.is_empty() {
                continue;
            }
            let lower = t.to_ascii_lowercase();
            let is_drive_switch = lower.len() == 2
                && lower.as_bytes()[1] == b':'
                && lower.as_bytes()[0].is_ascii_alphabetic();
            let is_housekeeping = is_drive_switch
                || lower.starts_with('#')
                || ["mount ", "imgmount ", "echo ", "rem ", "cd ", "cd\\", "set "]
                    .iter()
                    .any(|p| lower.starts_with(p))
                || ["cls", "cd", "exit", "pause", "echo", "echo."]
                    .contains(&lower.as_str());
            if is_housekeeping {
                continue;
            }
            let base = t.split_whitespace().next().unwrap_or(t);
            let base_lower = base.to_ascii_lowercase();
            if let Ok(entries) = std::fs::read_dir(game_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let name_lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    let runnable = name_lower.ends_with(".exe")
                        || name_lower.ends_with(".com")
                        || name_lower.ends_with(".bat");
                    let stem = name_lower.rsplitn(2, '.').last().unwrap_or(&name_lower);
                    if runnable && (stem == base_lower || name_lower == base_lower) {
                        log::info!(
                            "LP launch: using EN autoexec command '{}' (found {})",
                            t,
                            entry.file_name().to_string_lossy()
                        );
                        return Some((String::new(), t.to_string()));
                    }
                }
            }
            // Only the FIRST real command is the launch line; later lines
            // (cleanup, exit chains) must not be mistaken for it.
            break;
        }
    }

    let mut search_dirs: Vec<(String, std::path::PathBuf)> =
        vec![("".to_string(), game_dir.to_path_buf())];

    if let Ok(entries) = std::fs::read_dir(game_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.path().is_dir() {
                search_dirs.push((
                    entry.file_name().to_string_lossy().to_string(),
                    entry.path(),
                ));
            }
        }
    }

    // Strategy 1: Parse run.bat to find the real executable
    for (subdir, dir) in &search_dirs {
        let run_bat = dir.join("run.bat");
        if let Ok(content) = std::fs::read_to_string(&run_bat) {
            // Look for "@call <program>" or just "<program>" lines that reference
            // a .com/.exe/.bat that exists in the directory
            for line in content.lines() {
                let trimmed = line.trim();
                let cmd = trimmed
                    .strip_prefix("@call ")
                    .or_else(|| trimmed.strip_prefix("@CALL "))
                    .or_else(|| trimmed.strip_prefix("@"))
                    .unwrap_or(trimmed);
                let cmd = cmd.trim();
                let cmd_lower = cmd.to_ascii_lowercase();

                // Skip control flow, echo, copy, config, choice, labels, etc.
                let skip_prefixes = [
                    ":", "echo", "cls", "copy", "config", "choice",
                    "if ", "goto", "exit", "rem ", "set ", "pause",
                ];
                if cmd.is_empty() || skip_prefixes.iter().any(|p| cmd_lower.starts_with(p)) {
                    continue;
                }

                // Check if this command corresponds to an actual file in the game dir
                let base = cmd.split_whitespace().next().unwrap_or(cmd);
                // Search directory for a case-insensitive match
                if let Ok(entries) = std::fs::read_dir(dir) {
                    let base_lower = base.to_ascii_lowercase();
                    for entry in entries.filter_map(|e| e.ok()) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let name_lower = name.to_ascii_lowercase();
                        let stem = name_lower.rsplitn(2, '.').last().unwrap_or(&name_lower);
                        if stem == base_lower || name_lower == base_lower {
                            log::info!("LP launch: found '{}' via run.bat in {}", base, subdir);
                            return Some((subdir.clone(), base.to_string()));
                        }
                    }
                }
            }
        }
    }

    // Strategy 2: Look for any .bat file that calls an exe/com (skip known utility names).
    // Returns the .bat itself as the command so all its steps run in sequence.
    const SKIP_BAT_STEMS: &[&str] = &[
        "anleit", "readme", "install", "setup", "help", "manual",
        "problem", "config", "uninstal", "uninst",
    ];
    for (subdir, dir) in &search_dirs {
        let dir_stem = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let mut candidates: Vec<String> = if let Ok(entries) = std::fs::read_dir(dir) {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    name.ends_with(".bat")
                        && name != "run.bat"
                        && !SKIP_BAT_STEMS.iter().any(|s| name.starts_with(s))
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        } else {
            vec![]
        };

        // Prefer .bat whose stem matches the directory name
        candidates.sort_by_key(|b| {
            let stem = b.rsplitn(2, '.').last().unwrap_or(b).to_lowercase();
            usize::from(stem != dir_stem)
        });

        for bat in &candidates {
            let bat_path = dir.join(bat);
            if let Ok(content) = std::fs::read_to_string(&bat_path) {
                let has_exe_call = content.lines().any(|line| {
                    let l = line.trim().to_ascii_lowercase();
                    !l.is_empty()
                        && !l.starts_with(':')
                        && !l.starts_with("rem ")
                        && (l.contains(".exe") || l.contains(".com"))
                });
                if has_exe_call {
                    log::info!("LP launch: found .bat launcher '{}' in '{}'", bat, subdir);
                    return Some((subdir.clone(), bat.clone()));
                }
            }
        }
    }

    // Strategy 3: Look for a .com file (more likely to be a DOS game than .exe)
    for (subdir, dir) in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.ends_with(".com") && !name.contains("mouse") {
                    return Some((
                        subdir.clone(),
                        entry.file_name().to_string_lossy().to_string(),
                    ));
                }
            }
        }
    }

    // Strategy 4: an .exe in a subdirectory, then at the root (skipping
    // utilities and installers).
    const SKIP_EXE_STEMS: &[&str] = &[
        "install", "setup", "uninst", "config", "cdtest", "showtext",
        // DOS/4GW and protected-mode extenders - not the game itself
        "rtm", "dos4gw", "dpmi", "cwsdpmi",
    ];
    let subdirs_then_root = search_dirs
        .iter()
        .filter(|(s, _)| !s.is_empty())
        .chain(search_dirs.iter().filter(|(s, _)| s.is_empty()));
    for (subdir, dir) in subdirs_then_root {
        let dir_stem = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let mut exes: Vec<String> = if let Ok(entries) = std::fs::read_dir(dir) {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    name.ends_with(".exe")
                        && !SKIP_EXE_STEMS.iter().any(|s| name.starts_with(s))
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        } else {
            vec![]
        };

        // Prefer exe whose stem matches the directory name
        exes.sort_by_key(|e| {
            let stem = e.rsplitn(2, '.').last().unwrap_or(e).to_lowercase();
            usize::from(stem != dir_stem)
        });

        if let Some(exe) = exes.first() {
            log::info!("LP launch: found .exe '{}' in '{}'", exe, subdir);
            return Some((subdir.clone(), exe.clone()));
        }
    }

    None
}

/// Returns true if the [autoexec] section of a dosbox conf contains at least one
/// line that looks like an actual game launch command (not just mounts, drive switches,
/// comments, or housekeeping).
fn autoexec_has_launch_cmd(conf: &str) -> bool {
    let autoexec = match conf.split("[autoexec]").nth(1) {
        Some(s) => s,
        None => return false,
    };
    autoexec.lines().any(|line| {
        let l = line.trim().to_ascii_lowercase();
        if l.is_empty() || l.starts_with('#') || l.starts_with("rem ") {
            return false;
        }
        // Drive-switch: single letter followed by colon (a: through z:)
        let is_drive_switch = l.len() >= 2
            && l.as_bytes()[1] == b':'
            && l.as_bytes()[0].is_ascii_alphabetic();
        if is_drive_switch {
            return false;
        }
        const NON_LAUNCH: &[&str] = &[
            "@echo", "@exit", "echo ", "mount ", "imgmount", "exit", "cls",
        ];
        !NON_LAUNCH.iter().any(|p| l.starts_with(p))
    })
}


/// The DOSBox Staging binary: `resource_dir/dosbox-bin/` (Windows, next to
/// its DLLs), then beside the main executable (macOS, Linux), then the
/// resource dir, then dev `binaries/`, then PATH.
fn resolve_dosbox(app: &AppHandle) -> PathBuf {
    use tauri::Manager;
    let bin = if cfg!(windows) { "dosbox-staging.exe" } else { "dosbox-staging" };

    // Bundled layout flattens to `dosbox-bin/`, dev keeps
    // `resources/dosbox-bin/`; on Windows the exe must sit beside its DLLs.
    const DOSBOX_RES_DIRS: &[&str] = &["dosbox-bin", "resources/dosbox-bin"];
    if let Ok(res_dir) = app.path().resource_dir() {
        for sub in DOSBOX_RES_DIRS {
            let dbs_in_res = res_dir.join(sub).join(bin);
            if dbs_in_res.exists() {
                log::info!("Using bundled DOSBox (resource bin dir): {}", dbs_in_res.display());
                return dbs_in_res;
            }
        }
    }

    // 2. Next to the main executable (macOS Contents/MacOS/, Linux install dir).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(bin);
            if candidate.exists() {
                log::info!("Using bundled DOSBox (exe dir): {}", candidate.display());
                return candidate;
            }
        }
    }

    // 3. Inside resource_dir directly (legacy packaging layouts).
    if let Ok(res_dir) = app.path().resource_dir() {
        let prod = res_dir.join(bin);
        if prod.exists() {
            log::info!("Using bundled DOSBox (resource dir): {}", prod.display());
            return prod;
        }

        // 4. Dev mode (pnpm tauri dev): resource_dir is src-tauri/; binary is in binaries/
        //    named with the Rust target triple, e.g. dosbox-staging-aarch64-apple-darwin.
        let binaries_dir = res_dir.join("binaries");
        if let Ok(entries) = std::fs::read_dir(&binaries_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("dosbox-staging") {
                    log::info!("Using bundled DOSBox (dev): {}", entry.path().display());
                    return entry.path();
                }
            }
        }
    }

    log::warn!("Bundled DOSBox not found, falling back to system PATH");
    PathBuf::from(bin)
}

/// Copy the bundled glshaders into DOSBox's user config dir. Without them
/// Staging aborts at startup ("Fallback shader 'interpolation/bilinear' not
/// found"). Checks the fallback shader FILE, not the dir: an empty dir has
/// been seen in the wild.
fn ensure_dosbox_shaders(app: &AppHandle) {
    use tauri::Manager;

    let user_shader_dir: Option<PathBuf> = if cfg!(target_os = "linux") {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
            .map(|b| b.join("dosbox").join("glshaders"))
    } else if cfg!(target_os = "windows") {
        std::env::var("LOCALAPPDATA")
            .ok()
            .map(|p| PathBuf::from(p).join("DOSBox").join("glshaders"))
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join("Library").join("Preferences").join("DOSBox").join("glshaders"))
    } else {
        None
    };

    let Some(user_shader_dir) = user_shader_dir else {
        log::warn!("Could not determine DOSBox user config dir; shaders not installed");
        return;
    };

    if user_shader_dir.join("interpolation").join("bilinear.glsl").is_file() {
        return;
    }

    let res_dir = match app.path().resource_dir() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("resource_dir() failed while installing DOSBox shaders: {}", e);
            return;
        }
    };
    // Production layout first ("glshaders"), then pre-0.8.4 bundles
    // ("dosbox-glshaders"), then the dev-mode staged source path.
    const SHADER_RES_DIRS: &[&str] =
        &["glshaders", "dosbox-glshaders", "resources/dosbox-glshaders"];
    let Some(bundled) = SHADER_RES_DIRS
        .iter()
        .map(|sub| res_dir.join(sub))
        .find(|p| p.join("interpolation").join("bilinear.glsl").is_file())
    else {
        log::warn!("No bundled DOSBox shaders found under {}", res_dir.display());
        return;
    };

    if let Some(parent) = user_shader_dir.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("Failed to create DOSBox config parent dir: {}", e);
            return;
        }
    }

    if let Err(e) = copy_dir_recursive(&bundled, &user_shader_dir) {
        log::warn!("Failed to install DOSBox shaders: {}", e);
    } else {
        log::info!("Installed DOSBox shaders to {}", user_shader_dir.display());
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct GameSettings {
    /// `staging` forces DOSBox Staging for an ECE game; None = eXo's choice.
    pub engine: Option<String>,
    pub glshader: Option<String>,
    pub fullscreen: Option<String>,
    pub cycles: Option<String>,
    pub custom_conf: Option<String>,
}

#[tauri::command]
pub async fn get_game_settings(state: State<'_, DbState>, id: i64) -> Result<GameSettings, String> {
    let conn = state.lock()?;
    let cfg = queries::get_all_game_config(&conn, id).map_err(|e| e.to_string())?;
    Ok(GameSettings {
        engine: cfg.get("engine").cloned(),
        glshader: cfg.get("glshader").cloned(),
        fullscreen: cfg.get("fullscreen").cloned(),
        cycles: cfg.get("cycles").cloned(),
        custom_conf: cfg.get("custom_conf").cloned(),
    })
}

#[tauri::command]
pub async fn set_game_settings(
    state: State<'_, DbState>,
    id: i64,
    engine: Option<String>,
    glshader: Option<String>,
    fullscreen: Option<String>,
    cycles: Option<String>,
    custom_conf: Option<String>,
) -> Result<(), String> {
    let conn = state.lock()?;
    // For each key: Some(value) = set, None = delete (inherit global)
    let pairs: &[(&str, &Option<String>)] = &[
        ("engine", &engine),
        ("glshader", &glshader),
        ("fullscreen", &fullscreen),
        ("cycles", &cycles),
        ("custom_conf", &custom_conf),
    ];
    for (key, val) in pairs {
        match val {
            Some(v) if !v.is_empty() => {
                queries::set_game_config(&conn, id, key, v).map_err(|e| e.to_string())?;
            }
            _ => {
                queries::delete_game_config(&conn, id, key).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_recently_played(state: State<'_, DbState>, limit: Option<usize>) -> Result<Vec<Game>, String> {
    let conn = state.lock()?;
    queries::fetch_recently_played(&conn, limit.unwrap_or(12)).map_err(|e| e.to_string())
}


#[tauri::command]
pub async fn launch_game(app: AppHandle, db_state: State<'_, DbState>, id: i64) -> Result<String, String> {
    // Serialize against uninstall/download of the same game (see game_op_lock).
    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;
    // Read everything we need from the DB and drop the lock before the heavy
    // DOSBox path resolution + process spawning below.
    let (game, data_dir, crt_auto_enabled, fullscreen_enabled, per_game_config) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = configured_data_dir(&conn)?;
        let global_glshader = queries::get_config(&conn, "global_glshader")
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "crt-auto".to_string());
        let default_fullscreen = queries::get_config(&conn, "default_fullscreen")
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "window".to_string());
        let per_game_config = queries::get_all_game_config(&conn, id).map_err(|e| e.to_string())?;
        // Record the launch timestamp for "Recently Played" shelf.
        if let Err(e) = queries::set_last_played(&conn, id) {
            log::warn!("Failed to update last_played for {}: {}", game.title, e);
        }
        (game, data_dir, global_glshader == "crt-auto", default_fullscreen == "fullscreen", per_game_config)
    }; // lock dropped here - before path resolution + DOSBox spawning

    if !game.installed {
        return Err(format!("{} is not installed. Download it first.", game.title));
    }

    // One instance per game: two emulators on the same VHDs corrupt them.
    if running_games()
        .lock()
        .map(|s| s.contains(&running_game_key(&game)))
        .unwrap_or(false)
    {
        return Err(format!("'{}' is already running.", game.title));
    }

    // Win9x and ScummVM have their own pipelines; the conf machinery below
    // is DOSBox Staging/ECE only.
    let launcher = crate::commands::collections::collection_def(
        game.torrent_source.as_deref().unwrap_or("eXoDOS"),
    )
    .map(|c| c.launcher)
    .unwrap_or(crate::commands::collections::Launcher::DosBox);
    match launcher {
        crate::commands::collections::Launcher::Win9x => {
            return crate::commands::win9x::launch_win9x_game(
                &app,
                game,
                id,
                &data_dir,
                fullscreen_enabled,
                &per_game_config,
            )
            .await;
        }
        crate::commands::collections::Launcher::ScummVm => {
            return crate::commands::scummvm::launch_scummvm_game(
                &app,
                game,
                id,
                &data_dir,
                fullscreen_enabled,
                &per_game_config,
            )
            .await;
        }
        crate::commands::collections::Launcher::DosBox => {}
    }

    let dosbox_conf = game
        .dosbox_conf
        .as_deref()
        .ok_or_else(|| {
            let msg = format!("Game '{}' (id={}, lang={}, shortcode={:?}) has no DOSBox config path",
                game.title, id, game.language, game.shortcode);
            log::error!("launch_game: {}", msg);
            msg
        })?;

    // Every collection lives in ONE root (eXo's own merged layout), so the
    // main tree and this game's tree are the same directory.
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let src_game_prefix = collection_game_prefix(source);
    let main_torrent_root = crate::commands::paths::game_root(&data_dir);
    let torrent_root = main_torrent_root.clone();
    // working_dir is the first path component of game_prefix (e.g. "eXo")
    let working_dir_name = src_game_prefix.split('/').next().unwrap_or("eXo");
    let options_conf = main_torrent_root.join("eXo/emulators/dosbox/options.conf");

    let Some((game_conf, conf_root)) = resolve_game_conf(&data_dir, dosbox_conf) else {
        let msg = format!(
            "Game config not found: {}\nMake sure the game is fully downloaded and extracted.",
            torrent_root.join(dosbox_conf.replace('\\', "/")).display()
        );
        log::error!("launch_game({}): {}", game.title, msg);
        return Err(msg);
    };
    let working_dir = conf_root.join(working_dir_name);

    if !working_dir.exists() {
        return Err(format!("Working directory not found: {}", working_dir.display()));
    }

    // For LP games, determine the language dir and game path for config patching.
    // The game_folder is the second component of game_prefix (e.g. "eXoDOS" from "eXo/eXoDOS").
    let shortcode = game.shortcode.as_deref().unwrap_or("");
    let game_folder = src_game_prefix.split('/').nth(1).unwrap_or("eXoDOS");

    if !shortcode.is_empty() {
        extract_before_launch(&app, &game, id, source, &torrent_root).await?;
    }
    let lp_info = collection_lang_dir(source).map(|ld| {
        let dir = torrent_root.join(format!("{}/{}/{}", src_game_prefix, ld, shortcode));
        (shortcode, ld, game_folder, dir)
    });

    let ece_bin = resolve_engine(
        game.dosbox_variant.as_deref(),
        &main_torrent_root,
        &per_game_config,
    );
    if let Some(ref variant) = game.dosbox_variant {
        if variant.starts_with("ece") && ece_bin.is_none() {
            if cfg!(windows) {
                log::info!(
                    "ECE build not on disk yet for '{}' - using Staging (fetched with util.zip on next MIDI/ECE download)",
                    game.title
                );
            } else {
                log::info!(
                    "Game '{}' is tuned for DOSBox ECE '{}' (Windows-only build). \
                     Running under DOSBox Staging - experience may vary.",
                    game.title, variant
                );
            }
        }
    }
    let use_ece = ece_bin.is_some();

    let patched_conf = patch_dosbox_conf(
        &game_conf,
        &working_dir,
        lp_info.as_ref().map(|(sc, ld, gf, dir)| (*sc, *ld, *gf, dir.as_path())),
        // ECE understands its native [midi] keys - only translate for Staging.
        !use_ece,
    )?;

    log::info!(
        "Launching: {} with config {} (patched: {}, engine: {})",
        game.title,
        game_conf.display(),
        patched_conf.display(),
        if use_ece { "DOSBox ECE" } else { "DOSBox Staging" }
    );

    // Every launch: one stat once installed, and it repairs an empty dir.
    ensure_dosbox_shaders(&app);

    let dosbox_bin = ece_bin.unwrap_or_else(|| resolve_dosbox(&app));
    let mut cmd = Command::new(&dosbox_bin);
    cmd.current_dir(&working_dir)
        .arg("-conf")
        .arg(&patched_conf);

    if options_conf.exists() {
        cmd.arg("-conf").arg(&options_conf);
    }

    // macOS with CRT off: `output = texture`, the proven non-shader path.
    #[cfg(target_os = "macos")]
    {
        if !crt_auto_enabled {
            let conf_path = launch_conf_dir(&app)?.join(format!("macos_dosbox_{}.conf", id));
            std::fs::write(&conf_path, "[sdl]\noutput = texture\n")
                .map_err(|e| format!("Failed to write macOS override conf: {e}"))?;
            cmd.arg("-conf").arg(&conf_path);
        }
    }

    // User preferences, applied LAST and written for both states: Staging
    // defaults glshader to crt-auto, so "off" has to be an explicit value.
    {
        let glshader_val = if crt_auto_enabled { "crt-auto" } else { "sharp" };
        let fullscreen_val = if fullscreen_enabled { "true" } else { "false" };
        // glshader is Staging-specific; under ECE only fullscreen applies.
        let frag = if use_ece {
            format!("[sdl]\nfullscreen = {fullscreen_val}\n")
        } else {
            format!(
                "[sdl]\nfullscreen = {fullscreen_val}\n[render]\nglshader = {glshader_val}\n"
            )
        };
        let conf_path = launch_conf_dir(&app)?.join(format!("global_overrides_{}.conf", id));
        std::fs::write(&conf_path, &frag)
            .map_err(|e| format!("Failed to write global override conf: {e}"))?;
        cmd.arg("-conf").arg(&conf_path);
    }

    // Per-game overrides (last-wins over global). Only written if the user has
    // configured game-specific settings via the Game Settings dialog.
    {
        let game_conf_path = launch_conf_dir(&app)?.join(format!("game_{}.conf", id));
        {
            let mut frag = String::new();
            if let Some(fs) = per_game_config.get("fullscreen") {
                frag.push_str(&format!("[sdl]\nfullscreen = {}\n", fs));
            }
            if let Some(gs) = per_game_config.get("glshader") {
                // glshader is Staging-specific - ECE would log unknown-key noise.
                if gs != "default" && !use_ece {
                    frag.push_str(&format!("[render]\nglshader = {}\n", gs));
                }
            }
            if let Some(cy) = per_game_config.get("cycles") {
                frag.push_str(&format!("[cpu]\ncycles = {}\n", cy));
            }
            if let Some(custom) = per_game_config.get("custom_conf") {
                let trimmed = custom.trim();
                if !trimmed.is_empty() {
                    frag.push('\n');
                    frag.push_str(trimmed);
                    frag.push('\n');
                }
            }
            if frag.is_empty() {
                // Drop a stale fragment. Keyed on the file, not on the
                // config being empty: `engine` lives there but is never
                // written here.
                let _ = std::fs::remove_file(&game_conf_path);
            } else {
                std::fs::write(&game_conf_path, &frag)
                    .map_err(|e| format!("Failed to write per-game conf: {e}"))?;
                cmd.arg("-conf").arg(&game_conf_path);
            }
        }
    }

    spawn_emulator_and_track(&app, cmd, &dosbox_bin, &game, id)
}

/// AppImage/linuxdeploy variables that point only into `$APPDIR`. Dropped for
/// emulator children; unset, each falls back to its default.
#[cfg(target_os = "linux")]
const APPIMAGE_ONLY_VARS: &[&str] = &[
    // AppImage runtime / Tauri's AppRun
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "APPDIR",
    "APPIMAGE",
    "OWD",
    "ARGV0",
    "PYTHONHOME",
    // linuxdeploy-plugin-gtk.sh
    "GDK_BACKEND",
    "GTK_DATA_PREFIX",
    "GTK_THEME",
    "GTK_EXE_PREFIX",
    "GTK_PATH",
    "GTK_IM_MODULE_FILE",
    "GDK_PIXBUF_MODULE_FILE",
    "GIO_EXTRA_MODULES",
    "GSETTINGS_SCHEMA_DIR",
    // linuxdeploy-plugin-gstreamer.sh + AppRun
    "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_PATH_1_0",
    "GST_PLUGIN_SCANNER_1_0",
    "GST_PTP_HELPER_1_0",
];

/// Variables the AppRun PREPENDS `$APPDIR` entries to, keeping the host value
/// behind them. Removing these outright would take the host's own entries with
/// them (`PATH` most obviously), so only the `$APPDIR` entries are stripped.
#[cfg(target_os = "linux")]
const APPIMAGE_PREFIXED_PATH_VARS: &[&str] = &[
    "PATH",
    "XDG_DATA_DIRS",
    "PERLLIB",
    "PYTHONPATH",
    "QT_PLUGIN_PATH",
];

/// Drop the `$APPDIR`-rooted entries from a colon-separated path list.
/// `None` means nothing but AppImage entries were left.
#[cfg(target_os = "linux")]
fn strip_appdir_entries(value: &str, appdir: &str) -> Option<String> {
    let kept: Vec<&str> = value
        .split(':')
        .filter(|e| !e.is_empty() && !Path::new(e).starts_with(appdir))
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(":"))
    }
}

/// Strip the AppImage's environment from an emulator child: with the bundled
/// `LD_LIBRARY_PATH` inherited, an emulator mixes bundled and host libraries
/// and hangs in teardown on window close. Gated on `APPIMAGE`.
#[cfg(target_os = "linux")]
fn sanitize_appimage_env(cmd: &mut Command) {
    if std::env::var_os("APPIMAGE").is_none() {
        return;
    }
    let appdir = std::env::var("APPDIR").ok();
    for var in APPIMAGE_ONLY_VARS {
        cmd.env_remove(var);
    }
    if let Some(appdir) = appdir.as_deref().filter(|d| !d.is_empty()) {
        for var in APPIMAGE_PREFIXED_PATH_VARS {
            if let Ok(value) = std::env::var(var) {
                match strip_appdir_entries(&value, appdir) {
                    Some(kept) => cmd.env(var, kept),
                    None => cmd.env_remove(var),
                };
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn sanitize_appimage_env(_cmd: &mut Command) {}

/// Emitted when the emulator process ends, so the frontend can undo what a
/// launch changed (the music player pauses for a running game).
#[derive(Clone, Serialize)]
struct GameExited {
    id: i64,
}

/// Stdio setup, spawn and reaping for every emulator process (Staging, ECE,
/// DOSBox-X, 86Box, ScummVM).
pub(crate) fn spawn_emulator_and_track(
    app: &AppHandle,
    mut cmd: Command,
    emulator_bin: &Path,
    game: &Game,
    id: i64,
) -> Result<String, String> {
    // id names the per-game emulator log file; macOS nulls stdio instead.
    #[cfg(target_os = "macos")]
    let _ = id;
    // macOS dev builds: the binary extracted from the .app DMG has a bundle-anchored
    // code signature that becomes invalid without the surrounding bundle. Re-sign
    // ad-hoc if the signature is broken so macOS doesn't SIGKILL the process.
    #[cfg(all(target_os = "macos", debug_assertions))]
    {
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(emulator_bin)
            .output();
        let sig_ok = std::process::Command::new("codesign")
            .arg("-v")
            .arg(emulator_bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !sig_ok {
            log::warn!("Emulator binary has invalid signature, re-signing ad-hoc: {}", emulator_bin.display());
            let _ = std::process::Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(emulator_bin)
                .output();
        }
    }

    // macOS: posix_spawn returns EBADF from a Tauri GUI build when stdio is a
    // File, so all three streams are null (Staging logs to
    // ~/Library/Preferences/DOSBox/ itself). Elsewhere: per-game log file.
    #[cfg(target_os = "macos")]
    {
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
    }
    #[cfg(not(target_os = "macos"))]
    {
        cmd.stdin(std::process::Stdio::null());
        let mut stdio_set = false;
        if let Some(log_dir) = crate::commands::paths::LOG_DIR.get() {
            let _ = std::fs::create_dir_all(log_dir);
            let dosbox_log_path = log_dir.join(format!("dosbox-{}.log", id));
            match std::fs::File::create(&dosbox_log_path) {
                Ok(stdout_file) => match stdout_file.try_clone() {
                    Ok(stderr_file) => {
                        cmd.stdout(std::process::Stdio::from(stdout_file));
                        cmd.stderr(std::process::Stdio::from(stderr_file));
                        log::info!("DOSBox output → {}", dosbox_log_path.display());
                        stdio_set = true;
                    }
                    Err(e) => log::warn!("DOSBox log handle clone failed: {e}"),
                },
                Err(e) => log::warn!(
                    "Failed to open DOSBox log file {}: {e}",
                    dosbox_log_path.display()
                ),
            }
        }
        if !stdio_set {
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());
        }
    }

    // macOS: a no-op pre_exec forces fork+exec, dodging the posix_spawn EBADF.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        unsafe { cmd.pre_exec(|| Ok(())); }
    }

    sanitize_appimage_env(&mut cmd);

    log::info!("Spawning emulator: {}", emulator_bin.display());
    let mut child = cmd.spawn().map_err(|e| {
        log::error!("Emulator spawn failed for {}: {} (raw_os_error={:?})",
            emulator_bin.display(), e, e.raw_os_error());
        format!(
            "Failed to launch emulator ({}): {}",
            emulator_bin.display(), e
        )
    })?;

    // Reap the child and track the running game, so uninstall can refuse
    // while the emulator holds files open (a live rename loses saves on
    // Windows).
    let run_key = running_game_key(game);
    running_games().lock().map(|mut s| s.insert(run_key.clone())).ok();
    running_pids().lock().map(|mut m| m.insert(id, child.id())).ok();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        match child.wait() {
            Ok(status) => log::info!("Emulator exited ({}) for {}", status, run_key),
            Err(e) => log::warn!("Emulator wait failed for {}: {}", run_key, e),
        }
        running_games().lock().map(|mut s| s.remove(&run_key)).ok();
        running_pids().lock().map(|mut m| m.remove(&id)).ok();
        use tauri::Emitter;
        let _ = app.emit("game-exited", GameExited { id });
    });

    Ok(format!("Launched: {}", game.title))
}

#[cfg(test)]
mod tests {

    /// eXo mounts the game directory as C: and then cds into a subdirectory
    /// of it. Reading the mount is what makes that `cd` resolvable; without
    /// it every such conf was rejected and the launch fell back to the
    /// generated autoexec (Alien Odyssey DE).
    #[test]
    fn lp_probe_follows_the_mount_target_into_a_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let lp = tmp.path().join("!german/AlienOdy");
        std::fs::create_dir_all(lp.join("ODYSSEY")).unwrap();
        std::fs::write(lp.join("ODYSSEY/run.bat"), b"").unwrap();
        let conf = "[autoexec]\ncd ..\ncd ..\nmount c .\\eXoDOS\\AlienOdy\nc:\n@cd odyssey\n@call run\nexit\n";
        assert!(lp_autoexec_compatible(conf, "AlienOdy", &lp, tmp.path()));
    }

    /// The launch command still has to exist: a variant whose files were
    /// restructured must keep falling through to `find_lp_launch`.
    #[test]
    fn lp_probe_still_rejects_a_launch_command_the_variant_lacks() {
        let tmp = tempfile::tempdir().unwrap();
        let lp = tmp.path().join("!german/AlienOdy");
        std::fs::create_dir_all(lp.join("ODYSSEY")).unwrap();
        let conf = "[autoexec]\nmount c .\\eXoDOS\\AlienOdy\nc:\n@cd odyssey\n@call run\nexit\n";
        assert!(!lp_autoexec_compatible(conf, "AlienOdy", &lp, tmp.path()));
    }
    // The AppImage's LD_LIBRARY_PATH is what makes an emulator hang on window
    // close, and PATH must survive the cleanup or nothing launches at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_cleanup_drops_the_library_overrides_but_keeps_path() {
        assert!(super::APPIMAGE_ONLY_VARS.contains(&"LD_LIBRARY_PATH"));
        assert!(super::APPIMAGE_ONLY_VARS.contains(&"GIO_EXTRA_MODULES"));
        assert!(super::APPIMAGE_ONLY_VARS.contains(&"GST_PLUGIN_SYSTEM_PATH_1_0"));
        assert!(!super::APPIMAGE_ONLY_VARS.contains(&"PATH"));
        assert!(super::APPIMAGE_PREFIXED_PATH_VARS.contains(&"PATH"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn strip_appdir_entries_keeps_the_host_half() {
        assert_eq!(
            super::strip_appdir_entries("/tmp/.mount_x/usr/share/:/usr/share:/usr/local/share", "/tmp/.mount_x"),
            Some("/usr/share:/usr/local/share".to_string())
        );
        // A path that merely starts with the same characters is not inside it.
        assert_eq!(
            super::strip_appdir_entries("/tmp/.mount_xy/usr/bin:/usr/bin", "/tmp/.mount_x"),
            Some("/tmp/.mount_xy/usr/bin:/usr/bin".to_string())
        );
        // Nothing but AppImage entries left → the child gets no variable at all.
        assert_eq!(super::strip_appdir_entries("/tmp/.mount_x/usr/bin:", "/tmp/.mount_x"), None);
    }


    use super::*;
    use std::fs;

    // ── translate_midi_for_staging ───────────────────────────────────────────

    #[test]
    fn midi_translate_converts_ece_keys_to_staging_sections() {
        // Shape of ~1,500 real eXoDOS configs after path rewriting.
        let conf = "[sdl]\nfullscreen = true\n\
                    [midi]\nmididevice = mt32\nmpu401 = intelligent\n\
                    mt32.romdir = /data/eXo/mt32\n\
                    fluid.soundfont = /data/eXo/mt32/SoundCanvas.sf2\n\
                    fluid.gain = 0.4\n\
                    [autoexec]\nmount c /data/eXo/eXoDOS/SQ5\n";
        let out = translate_midi_for_staging(conf);

        // ECE dotted keys removed from [midi], Staging keys kept.
        assert!(!out.contains("mt32.romdir"));
        assert!(!out.contains("fluid.soundfont"));
        assert!(!out.contains("fluid.gain"));
        assert!(out.contains("mididevice = mt32"));
        assert!(out.contains("mpu401 = intelligent"));

        // Staging sections appended with the captured values.
        assert!(out.contains("[mt32]\nromdir = /data/eXo/mt32"));
        assert!(out.contains("[fluidsynth]\nsoundfont = /data/eXo/mt32/SoundCanvas.sf2"));

        // Autoexec untouched.
        assert!(out.contains("mount c /data/eXo/eXoDOS/SQ5"));
    }

    #[test]
    fn midi_translate_maps_default_device_to_auto() {
        let conf = "[midi]\nmididevice = default\nmt32.romdir = /x/mt32\n";
        let out = translate_midi_for_staging(conf);
        assert!(out.contains("mididevice = auto"));
        assert!(!out.contains("default"));
    }

    #[test]
    fn midi_translate_leaves_staging_native_configs_alone() {
        // Shape of the ~750 Staging-authored eXoDOS configs.
        let conf = "[midi]\nmididevice = auto\n\
                    [mt32]\nromdir = /data/eXo/mt32\n\
                    [fluidsynth]\nsoundfont = /data/eXo/mt32/SoundCanvas.sf2\n";
        let out = translate_midi_for_staging(conf);
        assert_eq!(out.matches("[mt32]").count(), 1);
        assert_eq!(out.matches("[fluidsynth]").count(), 1);
        assert!(out.contains("romdir = /data/eXo/mt32"));
    }

    #[test]
    fn midi_translate_no_midi_config_is_passthrough() {
        let conf = "[sdl]\nfullscreen = true\n[autoexec]\nrunme.exe\n";
        assert_eq!(translate_midi_for_staging(conf), conf);
    }

    // ── extract_game_zip / launch_zip_candidates ─────────────────────────────


    // ── patch_dosbox_conf ────────────────────────────────────────────────────

    fn write_conf(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    /// The autoexec's `path=C:\;z:\;c:\windows\` is guest text; rewriting
    /// its backslashes broke 1,122 eXoWin3x games at `runexit`.
    #[test]
    fn patch_dosbox_conf_keeps_guest_dos_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        let game_dir = working_dir.join("eXoWin3x/20k3x/cd");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("cd.cue"), b"").unwrap();

        let conf_content = "[autoexec]\nmount c .\\eXoWin3x\\20k3x\n\
             imgmount d .\\eXoWin3x\\20k3x\\cd\\cd.cue -t cdrom\nc:\n\
             path=C:\\;z:\\;c:\\windows\\\n@cd 20000\n@win runexit 20000\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(&conf_path, working_dir, None, true).unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        // Guest-side DOS text is untouched.
        assert!(
            patched.contains("path=C:\\;z:\\;c:\\windows\\"),
            "DOS PATH must keep its backslashes: {}", patched
        );
        assert!(patched.contains("@win runexit 20000"), "launch line intact: {}", patched);
        // Host paths still become absolute and forward-slashed.
        let abs = format!("{}/", working_dir.to_string_lossy()).replace('\\', "/");
        assert!(
            patched.contains(&format!("mount c {}eXoWin3x/20k3x", abs)),
            "mount must be absolute: {}", patched
        );
        assert!(
            patched.contains(&format!("{}eXoWin3x/20k3x/cd/cd.cue -t cdrom", abs)),
            "imgmount must be absolute: {}", patched
        );
    }

    /// 11th Hour (DE): `imgmount d ".\cd\11HDISK1.cue"` after `c:` is a
    /// GUEST path - no eXo/cd exists on the host.
    #[test]
    fn patch_dosbox_conf_keeps_guest_imgmount_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        let cd_dir = working_dir.join("eXoDOS/11thHour/cd");
        fs::create_dir_all(&cd_dir).unwrap();
        fs::write(cd_dir.join("11HDISK1.cue"), b"").unwrap();

        let conf_content = "[autoexec]\necho off\nmount c .\\eXoDOS\\11thHour\nc:\n\
             imgmount d \".\\cd\\11HDISK1.cue\" -t iso\ngame.exe /9 .\\ .\\\n@call run\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(&conf_path, working_dir, None, true).unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        let abs = format!("{}/", working_dir.to_string_lossy()).replace('\\', "/");
        assert!(
            patched.contains(&format!("mount c {}eXoDOS/11thHour", abs)),
            "existing mount target still becomes absolute: {}", patched
        );
        assert!(
            patched.contains("imgmount d \".\\cd\\11HDISK1.cue\" -t iso"),
            "guest-relative imgmount must stay as authored: {}", patched
        );
        // Bare `.\` is a guest argument (OxydGold) - the working dir itself
        // always exists, so it must be excluded from the existence gate.
        assert!(
            patched.contains("game.exe /9 .\\ .\\"),
            "bare .\\ arguments must stay as authored: {}", patched
        );
    }

    /// eXoWin3x IDE games: the DOSBox-X `[ide]` section becomes Staging's
    /// `-ide` imgmount flag - without it a guest booted from an HDD image
    /// never sees the CD (its ATAPI driver finds no controller).
    #[test]
    fn ide_translate_adds_flag_to_cd_imgmounts() {
        let conf = "[dosbox]\nmemsize=32\n[ide, primary] \nenable=true \n\
             [ide, secondary] \nenable=true \n[autoexec]\n@echo off\n\
             imgmount c game/i100_203.img\nimgmount d game/cd/cd.cue -t cdrom \n\
             boot -l c\nexit\n";
        let out = translate_ide_for_staging(conf);
        assert!(
            out.contains("imgmount d game/cd/cd.cue -t cdrom -ide"),
            "CD imgmount must gain -ide: {}", out
        );
        // The HDD imgmount is not a CD mount - Staging's -ide only applies to CD drives.
        assert!(out.contains("imgmount c game/i100_203.img\n"), "hdd imgmount untouched: {}", out);
    }

    /// 6 eXoWin3x configs already carry the flag in DOSBox-X's argument form
    /// (`-ide 2m` = secondary master); Staging's flag takes no argument.
    #[test]
    fn ide_translate_normalizes_dosbox_x_slot_argument() {
        let conf = "[ide, secondary]\nenable=true\n[autoexec]\n\
             imgmount d \"game/cd/A Title (Pub).ISO\" -t iso -fs iso -ide 2m\nboot -l c\n";
        let out = translate_ide_for_staging(conf);
        assert!(
            out.contains("imgmount d \"game/cd/A Title (Pub).ISO\" -t iso -fs iso -ide\n"),
            "slot argument must be dropped: {}", out
        );
        assert!(!out.contains("-ide 2m"), "DOSBox-X form must not survive: {}", out);
    }

    /// Configs without an [ide] section keep their imgmounts as authored -
    /// 483 non-IDE Win3x games mount .cue sheets that work fine without a
    /// controller, and forcing one on them changes tested behavior.
    #[test]
    fn ide_translate_leaves_non_ide_configs_alone() {
        let conf = "[dosbox]\nmemsize=32\n[autoexec]\n\
             imgmount d game/cd/cd.cue -t cdrom\nwin runexit GAME\n";
        assert_eq!(translate_ide_for_staging(conf), conf);
        // Commented-out [ide] documentation (the TheCHAOS pattern) is not a
        // request for a controller either.
        let commented = "[dosbox]\n# [ide, primary] docs only\n[autoexec]\n\
             imgmount d game/cd/cd.cue -t cdrom\n";
        assert_eq!(translate_ide_for_staging(commented), commented);
    }

    /// The line holds an absolute REWRITTEN host path when this runs - `-ide`
    /// inside a path segment must not read as "flag already present", or a
    /// data dir like /mnt/games-ide/ silently disables the translation.
    #[test]
    fn ide_translate_ignores_ide_inside_paths() {
        let conf = "[ide, primary]\nenable=true\n[autoexec]\n\
             imgmount d /mnt/games-ide/eXoWin3x/T-IDE.iso -t iso\nboot -l c\n";
        let out = translate_ide_for_staging(conf);
        assert!(
            out.contains("imgmount d /mnt/games-ide/eXoWin3x/T-IDE.iso -t iso -ide\n"),
            "flag must still be appended: {}", out
        );
        // Doubled space between -t and its value is still a CD mount.
        let spaced = "[ide, primary]\nenable=true\n[autoexec]\n\
             imgmount d game/cd.cue -t  cdrom\n";
        assert!(
            translate_ide_for_staging(spaced).contains("-t  cdrom -ide\n"),
            "double-spaced -t value must still translate"
        );
    }

    /// Every collection resolves inside the single root; LP rows fall back
    /// to the lang-scoped conf when the EN path is absent.
    #[test]
    fn resolve_game_conf_probe_order() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_string_lossy().into_owned();
        let rel = "eXo/eXoWin3x/!win3x/GeoGeo/dosbox.conf";
        let root = tmp.path().join(crate::commands::paths::DEFAULT_ROOT_FOLDER);

        // Nothing on disk: no result.
        assert!(resolve_game_conf(&data_dir, rel).is_none());

        // A Win3x conf is found in the one root, not in a tree of its own.
        let conf_path = root.join(rel);
        fs::create_dir_all(conf_path.parent().unwrap()).unwrap();
        fs::write(&conf_path, "[autoexec]\n").unwrap();
        let (conf, found_root) = resolve_game_conf(&data_dir, rel).unwrap();
        assert_eq!(conf, conf_path);
        assert_eq!(found_root, root);

        // Lang-scoped alternate (LP rows): conf only under a language subdir.
        let lang_conf = root.join("eXo/eXoDOS/!dos/!german/DasAmt/dosbox.conf");
        fs::create_dir_all(lang_conf.parent().unwrap()).unwrap();
        fs::write(&lang_conf, "[autoexec]\n").unwrap();
        let (conf, found_root) =
            resolve_game_conf(&data_dir, "eXo/eXoDOS/!dos/DasAmt/dosbox.conf")
                .unwrap();
        assert_eq!(conf, lang_conf);
        assert_eq!(found_root, root);
    }

    // ── conf_requests_printer ───────────────────────────────────────────────

    #[test]
    fn printer_detection_matches_enabled_not_documentation() {
        // The 13 eXoDOS printer titles set both keys.
        assert!(conf_requests_printer("[parallel]\nparallel1=printer\n[printer]\nprinter=true\nprintoutput=printer\n"));
        // TheCHAOS (eXoWin3x) has the whole option documentation as comments
        // but disables the port - must NOT match.
        assert!(!conf_requests_printer(
            "[parallel]\nparallel1=disabled\nparallel2=disabled\n\
             # parallel1: parallel1-3 -- set type of device connected to lpt port.\n\
             #               printer (virtual dot-matrix printer, see [printer] section)\n"
        ));
    }

    #[test]
    fn patch_dosbox_conf_converts_windows_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        fs::create_dir_all(working_dir.join("eXoDOS/SQ5")).unwrap();

        let conf_content = "[sdl]\nfullscreen=false\n[autoexec]\n@mount c .\\eXoDOS\\SQ5\nc:\nSQ5.bat\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(&conf_path, working_dir, None, true).unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        // Backslash replaced with forward slash
        assert!(!patched.contains('\\'), "no backslashes should remain: {}", patched);
        // Relative .\ prefix replaced with absolute working dir. On Windows
        // the working dir itself contains backslashes, which the patcher
        // normalizes to forward slashes - normalize the expectation too.
        let abs_prefix = format!("{}/", working_dir.to_string_lossy()).replace('\\', "/");
        assert!(patched.contains(&abs_prefix), "absolute path prefix expected: {}", patched);
    }

    #[test]
    fn mount_targets_keep_no_trailing_separator() {
        // `mount c .\eXoDOS\` (1,570 confs): the trailing separator must go.
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        fs::create_dir_all(working_dir.join("eXoDOS/DOOMII")).unwrap();

        let conf_content = "[autoexec]\nmount c .\\eXoDOS\\\nc:\n@cd DOOMII\n@call run\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(&conf_path, working_dir, None, true).unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        let mount_line = patched
            .lines()
            .find(|l| l.contains("mount c "))
            .expect("mount line survives");
        assert!(
            !mount_line.trim_end().ends_with('/'),
            "mount target must not end in a separator: {}",
            mount_line
        );
        let expected = format!("{}/eXoDOS", working_dir.to_string_lossy()).replace('\\', "/");
        assert!(
            mount_line.contains(&expected),
            "mount should point at the eXoDOS root: {}",
            mount_line
        );
    }

    #[test]
    fn a_data_dir_with_spaces_gets_the_mount_argument_quoted() {
        // eXo writes the mount target unquoted because `.\eXoDOS\SQ5` has no
        // spaces. The substituted host path can - DOSBox would then mount
        // everything up to the first one.
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path().join("My Games/eXo");
        fs::create_dir_all(working_dir.join("eXoDOS/SQ5")).unwrap();

        let conf_content =
            "[midi]\nfluid.soundfont=.\\mt32\\SoundCanvas.sf2\n[autoexec]\n@mount c .\\eXoDOS\\SQ5\nc:\nSQ5.bat\nexit\n";
        fs::create_dir_all(working_dir.join("mt32")).unwrap();
        fs::write(working_dir.join("mt32/SoundCanvas.sf2"), b"").unwrap();
        let conf_path = write_conf(&working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(&conf_path, &working_dir, None, false).unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        let mount_line = patched.lines().find(|l| l.contains("mount c ")).unwrap();
        assert!(
            mount_line.contains("\"") && mount_line.trim_end().ends_with('"'),
            "mount argument with spaces must be quoted: {}",
            mount_line
        );
        // A config property is read verbatim - quotes there would land in the
        // path itself.
        let sf_line = patched
            .lines()
            .find(|l| l.starts_with("fluid.soundfont="))
            .unwrap();
        assert!(
            !sf_line.contains('"'),
            "config values stay unquoted: {}",
            sf_line
        );
    }

    #[test]
    fn patch_dosbox_conf_lp_overlay_direct_mount() {
        // EN conf mounts the game dir directly: mount target must be routed
        // through the overlay staging dir whose link points at the LP dir.
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        let lp_dir = working_dir.join("eXoDOS/!german/SQ5");
        fs::create_dir_all(&lp_dir).unwrap();
        fs::write(lp_dir.join("SQ5.BAT"), b"").unwrap();

        let conf_content = "[autoexec]\n@mount c .\\eXoDOS\\SQ5\nc:\nSQ5.bat\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(
            &conf_path,
            working_dir,
            Some(("SQ5", "!german", "eXoDOS", &lp_dir)),
            true,
        )
        .unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        assert!(
            patched.contains(".exodium_lp/german_SQ5"),
            "mount should be routed through the overlay: {}",
            patched
        );
        assert!(patched.contains("SQ5.bat"), "launch command must survive: {}", patched);
        // The overlay link resolves to the LP dir.
        let linked = working_dir.join(".exodium_lp/german_SQ5/SQ5");
        assert!(linked.join("SQ5.BAT").exists(), "overlay link should reach LP files");
    }

    #[test]
    fn patch_dosbox_conf_lp_overlay_root_mount_cd() {
        // Cobra Mission (ES) shape: EN conf mounts the eXoDOS root and cd's
        // into the game dir; the LP dir holds a bare root-level EXE.
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        fs::create_dir_all(working_dir.join("eXoDOS")).unwrap();
        let lp_dir = working_dir.join("eXoDOS/!spanish/cobmiss");
        fs::create_dir_all(&lp_dir).unwrap();
        fs::write(lp_dir.join("CM.EXE"), b"").unwrap();

        let conf_content =
            "[autoexec]\n@mount c .\\eXoDOS\\\nc:\ncls\ncd cobmiss\n@cm\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(
            &conf_path,
            working_dir,
            Some(("cobmiss", "!spanish", "eXoDOS", &lp_dir)),
            true,
        )
        .unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        assert!(
            patched.contains(".exodium_lp/spanish_cobmiss"),
            "root mount should be routed through the overlay: {}",
            patched
        );
        // The authored launch sequence survives verbatim.
        assert!(patched.contains("cd cobmiss"), "{}", patched);
        assert!(patched.contains("@cm"), "{}", patched);
        // And the overlay resolves cd cobmiss -> LP files.
        let linked = working_dir.join(".exodium_lp/spanish_cobmiss/cobmiss");
        assert!(linked.join("CM.EXE").exists(), "overlay link should reach LP files");
    }

    #[test]
    fn patch_dosbox_conf_lp_falls_back_when_exe_renamed() {
        // LP variant renamed the executable: the EN launch command can't be
        // validated, so the generated-autoexec fallback must kick in and
        // find the actual root-level EXE.
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();
        fs::create_dir_all(working_dir.join("eXoDOS")).unwrap();
        let lp_dir = working_dir.join("eXoDOS/!spanish/cobmiss");
        fs::create_dir_all(&lp_dir).unwrap();
        fs::write(lp_dir.join("JUEGO.EXE"), b"").unwrap();

        let conf_content =
            "[autoexec]\n@mount c .\\eXoDOS\\\nc:\ncd cobmiss\n@cm\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(
            &conf_path,
            working_dir,
            Some(("cobmiss", "!spanish", "eXoDOS", &lp_dir)),
            true,
        )
        .unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        assert!(
            patched.to_ascii_lowercase().contains("juego.exe"),
            "fallback should launch the real executable: {}",
            patched
        );
        assert!(
            patched.contains("!spanish/cobmiss"),
            "fallback mounts the LP dir directly: {}",
            patched
        );
    }

    // ── find_lp_launch ───────────────────────────────────────────────────────

    #[test]
    fn find_lp_launch_parses_run_bat() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path();

        // Create the target executable so the directory scan finds it
        fs::write(game_dir.join("sq5.exe"), b"").unwrap();

        let run_bat = "@call sq5.exe\n";
        fs::write(game_dir.join("run.bat"), run_bat).unwrap();

        let result = find_lp_launch(game_dir, None);
        assert!(result.is_some(), "run.bat parsing should find a launch command");
        let (subdir, cmd) = result.unwrap();
        assert_eq!(subdir, "", "game is in root of game_dir");
        assert_eq!(cmd, "sq5.exe");
    }

    #[test]
    fn find_lp_launch_finds_com_file_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path();

        // No run.bat, but a .com file exists
        fs::write(game_dir.join("game.com"), b"").unwrap();

        let result = find_lp_launch(game_dir, None);
        assert!(result.is_some(), ".com file should be found as fallback");
        let (_, cmd) = result.unwrap();
        assert!(cmd.to_lowercase().ends_with(".com"));
    }

    #[test]
    fn find_lp_launch_returns_none_for_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_lp_launch(tmp.path(), None).is_none());
    }

    #[test]
    fn find_lp_launch_uses_en_autoexec_command() {
        // Regression: Cobra Mission (ES) - bare root-level CM.EXE plus
        // INSTALL.EXE, no .bat. The EN autoexec names the launcher.
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path();
        fs::write(game_dir.join("CM.EXE"), b"").unwrap();
        fs::write(game_dir.join("INSTALL.EXE"), b"").unwrap();
        fs::write(game_dir.join("DAT.VOL"), b"").unwrap();

        let en_conf = "[sdl]\nfullscreen=false\n[autoexec]\n\
                       @mount c .\\eXoDOS\\\nc:\ncls\ncd cobmiss\n@cm\nexit\n";
        let (subdir, cmd) = find_lp_launch(game_dir, Some(en_conf)).unwrap();
        assert_eq!(subdir, "");
        assert_eq!(cmd, "cm");
    }

    #[test]
    fn find_lp_launch_falls_back_to_root_exe() {
        // No EN hint, no .bat/.com: the root-level EXE must still be found
        // (installers are skipped).
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path();
        fs::write(game_dir.join("CM.EXE"), b"").unwrap();
        fs::write(game_dir.join("INSTALL.EXE"), b"").unwrap();

        let (subdir, cmd) = find_lp_launch(game_dir, None).unwrap();
        assert_eq!(subdir, "");
        assert_eq!(cmd.to_ascii_lowercase(), "cm.exe");
    }

    #[test]
    fn find_lp_launch_en_hint_ignores_missing_program() {
        // EN autoexec references a program the LP dir doesn't have -
        // must fall through to the heuristics, not return a broken command.
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path();
        fs::write(game_dir.join("game.com"), b"").unwrap();

        let en_conf = "[autoexec]\nmount c .\\eXoDOS\\\nc:\ncd foo\n@other\nexit\n";
        let (_, cmd) = find_lp_launch(game_dir, Some(en_conf)).unwrap();
        assert_eq!(cmd.to_ascii_lowercase(), "game.com");
    }
}
