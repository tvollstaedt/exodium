use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;

/// Per-game (last_retry_at, attempts) for stuck-download recovery in
/// `get_download_progress`. Module-scoped so the success branch can clear
/// the entry once the ZIP appears, preventing a stale counter from
/// surfacing a premature error if the same game gets stuck again.
static RETRY_STATE: OnceLock<
    Mutex<std::collections::HashMap<i64, (std::time::Instant, u32)>>,
> = OnceLock::new();

fn retry_state() -> &'static Mutex<std::collections::HashMap<i64, (std::time::Instant, u32)>> {
    RETRY_STATE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::db;
use crate::db::queries;
use crate::models::Game;
use crate::torrent::manager::DownloadProgress;

use super::TorrentState;

/// Resolve the data directory for a collection.
/// All collections share the same data directory (overlay model - no collection subdirectories).
pub fn collection_data_dir(data_dir: &str, _source: &str) -> PathBuf {
    std::path::Path::new(data_dir).to_path_buf()
}

/// Get the game directory prefix for a collection (path from inner_folder to game dirs).
fn collection_game_prefix(source: &str) -> &'static str {
    crate::commands::setup::collection_def(source)
        .map(|c| c.game_prefix)
        .unwrap_or("eXo/eXoDOS")
}

/// Get the language subdirectory for an LP collection, if any.
fn collection_lang_dir(source: &str) -> Option<&'static str> {
    crate::commands::setup::collection_def(source).and_then(|c| c.lang_dir)
}

/// The year directory a `year_subdirs` collection nests its games under,
/// read from the application_path (`eXo\eXoWin9x\!win9x\<year>\<TitleDir>\…`).
/// None for every other collection - and for a malformed path, in which case
/// callers fall back to the flat `<game_prefix>/<shortcode>` layout.
fn collection_year_dir(source: &str, app_path: Option<&str>) -> Option<String> {
    let def = crate::commands::setup::collection_def(source)?;
    if !def.year_subdirs {
        return None;
    }
    let normalized = app_path?.replace('\\', "/");
    let needle = format!("/{}/", def.shortcode_segment);
    let idx = normalized.find(&needle)?;
    let year = normalized[idx + needle.len()..].split('/').next()?;
    (year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit())).then(|| year.to_string())
}

/// Torrent-relative directory holding a game's installed files.
/// Standard: <game_prefix>[/<lang_dir>]/<shortcode>
/// year_subdirs (eXoWin9x): <game_prefix>/<year>/<shortcode> - the shortcode
/// IS the title directory there ("Connect4 (1995)").
pub(crate) fn collection_rel_game_dir(source: &str, shortcode: &str, app_path: Option<&str>) -> String {
    let prefix = collection_game_prefix(source);
    if let Some(year) = collection_year_dir(source, app_path) {
        return format!("{}/{}/{}", prefix, year, shortcode);
    }
    match collection_lang_dir(source) {
        Some(ld) => format!("{}/{}/{}", prefix, ld, shortcode),
        None => format!("{}/{}", prefix, shortcode),
    }
}

/// Torrent-relative path of a game's ZIP (same year/lang nesting as the dir).
pub(crate) fn collection_rel_zip(source: &str, game_name: &str, app_path: Option<&str>) -> String {
    let prefix = collection_game_prefix(source);
    if let Some(year) = collection_year_dir(source, app_path) {
        return format!("{}/{}/{}.zip", prefix, year, game_name);
    }
    match collection_lang_dir(source) {
        Some(ld) => format!("{}/{}/{}.zip", prefix, ld, game_name),
        None => format!("{}/{}.zip", prefix, game_name),
    }
}

/// Language subdirectories used in the eXoDOS file structure.
const LANG_DIRS: &[&str] = &["!german", "!polish", "!czech", "!slovak", "!spanish"];

pub struct DbState(pub Mutex<Connection>);

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
) -> Result<GameList, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
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
    };

    let total = queries::count_games_filtered(&conn, &f).map_err(|e| e.to_string())?;
    let games = queries::fetch_games_filtered(&conn, page, per_page, &f).map_err(|e| e.to_string())?;

    Ok(GameList { games, total })
}

#[tauri::command]
pub async fn get_genres(state: State<'_, DbState>, collection: Option<String>) -> Result<Vec<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let collection = collection.unwrap_or_default();
    queries::get_genres(&conn, &collection).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_section_keys(
    state: State<'_, DbState>,
    sort_by: Option<String>,
    query: Option<String>,
    genre: Option<String>,
    collection: Option<String>,
    favorites_only: Option<bool>,
    playlist_id: Option<i64>,
) -> Result<Vec<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
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
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_game_variants(&conn, &shortcode, &collection).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_installed_games(state: State<'_, DbState>) -> Result<Vec<Game>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_installed_games(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_favorite(state: State<'_, DbState>, id: i64) -> Result<bool, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::toggle_favorite(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_game(state: State<'_, DbState>, id: i64) -> Result<Option<Game>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_game_by_id(&conn, id).map_err(|e| e.to_string())
}


#[tauri::command]
pub async fn get_config(state: State<'_, DbState>, key: String) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
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
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, &key, &value).map_err(|e| e.to_string())?;
    }
    // The static asset-protocol scope only covers $RESOURCE/$APPDATA; the
    // user-chosen game dir (thumbnails, screenshots, manuals served via the
    // asset protocol) is granted at runtime - here on change, and at startup
    // in lib.rs for the stored value.
    if key == "data_dir" {
        crate::allow_asset_dir(&app, std::path::Path::new(&value));
    }
    Ok(())
}

/// Open a manual (or other game file) in the system viewer. The webview has
/// no opener:allow-open-path capability - this command re-validates that the
/// path lives under the configured data dir before handing it to the OS,
/// which also works for data dirs outside $HOME (external drives).
#[tauri::command]
pub async fn open_manual(
    app: AppHandle,
    state: State<'_, DbState>,
    path: String,
) -> Result<(), String> {
    let data_dir = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured")?
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
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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

/// Re-apply seeding preference and caps together. They share one knob, so a
/// change to either has to go through the same call - see
/// `DownloadManager::apply_limits`. No manager means no session, and the
/// preferences are read again when one is created.
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

/// Current transfer rates for the whole torrent session.
///
/// All four collections share one librqbit session, so this is a single cheap
/// read rather than a sum over managers - and it cannot double-count a peer
/// that serves two collections.
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

/// Queue a game for download via torrent.
#[tauri::command]
pub async fn download_game(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<String, String> {
    let game = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?
    };

    if game.installed {
        return Ok(format!("{} is already installed", game.title));
    }

    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;

    let game_idx = game
        .game_torrent_index
        .ok_or_else(|| format!("{} has no torrent index - cannot download", game.title))?
        as usize;

    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");

    // Clone Arc references and immediately drop the guard so we don't hold it across awaits.
    let (manager, main_mgr_opt) = {
        let guard = torrent_state.0.read().await;
        let manager = guard
            .get(source)
            .cloned()
            .ok_or_else(|| {
                if crate::commands::setup::is_offline(&db_state.0) {
                    "Offline mode is on - no games can be downloaded. \
                     Switch to online mode in Settings → Network."
                        .to_string()
                } else {
                    format!("Download manager for '{}' not initialized.", source)
                }
            })?;
        let main_mgr = guard.get("eXoDOS").cloned();
        (manager, main_mgr)
    };

    let is_win9x_collection = crate::commands::setup::collection_def(source)
        .is_some_and(|c| c.year_subdirs);

    // Win9x games need the shared support payload (parent OS VHDs +
    // emulators) from utilWin9x.zip before they can launch; queued further
    // down with the first Win9x game download, but the disk preflight has to
    // budget it here (2.5 GB zip + 2.5 GB inner temp + ~2.5 GB extracted).
    let win9x_support_missing = is_win9x_collection
        && !crate::commands::win9x::win9x_support_ready(
            &manager.torrent_root(),
            game.dosbox_variant.as_deref(),
        );

    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").ok().flatten()
    };

    // On macOS/Linux the emulator itself is a content pack; queue it with
    // the download the same way the support files are. Resolver-gated, so a
    // PATH/Flatpak/system install (or an already-installed pack) never pays
    // for it - and Windows never needs it (eXo's EXTWin9x.zip carries both
    // builds). None also when the manifest has no installable source yet.
    let win9x_emulator_pack = if is_win9x_collection && !cfg!(windows) {
        let dd = data_dir.clone().unwrap_or_default();
        crate::commands::win9x::emulator_pack_for_variant(game.dosbox_variant.as_deref())
            .filter(|_| {
                !crate::commands::win9x::win9x_engine_resolvable(
                    &app,
                    &manager.torrent_root(),
                    &dd,
                    game.dosbox_variant.as_deref(),
                )
            })
            .and_then(|pack_id| {
                crate::commands::content_packs::installable_pack(source, pack_id)
                    .map(|info| (pack_id, info))
            })
    } else {
        None
    };

    // Disk-space preflight: refusing upfront beats a multi-GB torrent (plus
    // ~equal-sized extraction) failing halfway with a partial install. Runs
    // BEFORE set_in_library so a refusal doesn't leave a phantom "My Games"
    // entry. Downloaded bytes on disk are credited ONCE (they only reduce
    // the remaining download; the extraction target still needs full size).
    if let Some(size) = game.download_size {
        if let Some(dir) = data_dir.as_deref() {
            let on_disk = manager
                .file_output_path(game_idx)
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            let mut needed = (size as u64)
                .saturating_mul(2)
                .saturating_sub(on_disk)
                + 500 * 1024 * 1024;
            if win9x_support_missing {
                needed += 8 * 1024 * 1024 * 1024;
            }
            if let Some((_, info)) = &win9x_emulator_pack {
                // Same 2.2x factor as the pack installer's own preflight
                // (archive + extracted copy).
                needed += (info.size_bytes as f64 * 2.2) as u64;
            }
            if let Ok(free) = fs4::available_space(std::path::Path::new(dir)) {
                if free < needed {
                    let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
                    return Err(format!(
                        "Not enough disk space for {}: needs about {:.1} GB free \
                         (download + extraction), but only {:.1} GB is available.",
                        game.title,
                        gib(needed),
                        gib(free)
                    ));
                }
            }
        }
    }

    let mut files = vec![game_idx];
    if let Some(gd_idx) = game.gamedata_torrent_index {
        files.push(gd_idx as usize);
    }

    // Queue the Win9x support payload with the first Win9x game download and
    // arm the extraction watcher (budgeted in the preflight above).
    if win9x_support_missing {
        crate::commands::win9x::ensure_win9x_support_queued(&manager).await;
    }

    // Queue the emulator pack alongside (see win9x_emulator_pack above).
    // "Install already in progress" is the normal second-download case.
    if let Some((pack_id, _)) = &win9x_emulator_pack {
        if let Err(e) =
            crate::commands::content_packs::start_pack_install(&app, source, pack_id).await
        {
            log::info!("Win9x emulator pack '{pack_id}' not queued: {e}");
        }
    }

    if let Some(ref main_mgr) = main_mgr_opt {
        // Queue !DOSmetadata.zip (DOSBox configs) if the configs tree is
        // missing (normally pre-created by the bundled configs zip).
        let main_prefix = collection_game_prefix("eXoDOS");
        let main_segment = crate::commands::setup::collection_def("eXoDOS")
            .map(|c| c.shortcode_segment)
            .unwrap_or("!dos");
        let dosbox_dir = main_mgr
            .torrent_root()
            .join(format!("{}/{}", main_prefix, main_segment));
        if !dosbox_dir.exists() {
            if let Some(dm) = main_mgr.index().find_dosbox_metadata_zip() {
                let _ = main_mgr.download_files(vec![dm.index]).await;
                log::info!("Also downloading !DOSmetadata.zip (DOSBox configs)");
            }
        }

        // Music support: the MT-32 ROMs + SoundCanvas soundfont live in
        // eXo/util/util.zip (~630 MB; NOT in !DOSmetadata.zip, which is
        // configs only). Fetch it once, when a game whose config actually
        // requests MIDI is downloaded - ~1/3 of the catalog does; the rest
        // never pays the download.
        let mt32_dir = main_mgr.torrent_root().join("eXo/mt32");
        let needs_midi_assets = !mt32_dir.exists()
            && game_requests_midi(&main_mgr.torrent_root(), game.dosbox_conf.as_deref());
        let needs_ece = cfg!(windows)
            && game
                .dosbox_variant
                .as_deref()
                .is_some_and(|v| v.starts_with("ece"))
            && !main_mgr
                .torrent_root()
                .join("eXo/emulators/dosbox/ece4230")
                .exists();
        if needs_midi_assets || needs_ece {
            if let Some(util) = main_mgr.index().find_by_suffix("util/util.zip") {
                let util_index = util.index;
                let util_size = util.size;
                if !main_mgr.is_file_selected(util_index).await {
                    let _ = main_mgr.download_files(vec![util_index]).await;
                    log::info!(
                        "Also downloading util.zip ({:.0} MB, one-time: MT-32 ROMs + SoundCanvas soundfont for MIDI music)",
                        util_size as f64 / 1e6
                    );
                }
                // Always (re)arm the watcher - it also covers the case where
                // util.zip finished in a previous run but extraction never
                // happened (nobody was polling when it completed).
                spawn_mt32_extraction_watcher(std::sync::Arc::clone(main_mgr), util_index);
            }
        }
    }

    manager
        .download_files(files)
        .await
        .map_err(|e| format!("Failed to queue download: {}", e))?;

    // Mark as in library only after the download is actually queued - doing
    // it earlier left a phantom "My Games" card when queueing failed.
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_in_library(&conn, id).map_err(|e| e.to_string())?;
    }

    Ok(format!("Downloading: {}", game.title))
}

/// Get download progress for a game. If complete, extract and mark installed.
#[tauri::command]
pub async fn get_download_progress(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<Option<DownloadProgress>, String> {
    let (game_idx, gamedata_idx, title, already_installed, source) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        match game.game_torrent_index {
            Some(idx) => (
                idx as usize,
                game.gamedata_torrent_index.map(|i| i as usize),
                game.title,
                game.installed,
                game.torrent_source.unwrap_or_else(|| "eXoDOS".to_string()),
            ),
            None => return Ok(None),
        }
    };

    // Clone Arc references and drop the guard immediately - the guard must not be held
    // across any .await point to avoid blocking concurrent writers.
    let (manager, main_mgr_opt) = {
        let guard = torrent_state.0.read().await;
        let manager = match guard.get(&source).cloned() {
            Some(m) => m,
            None => return Ok(None),
        };
        let main_mgr = guard.get("eXoDOS").cloned();
        (manager, main_mgr)
    };

    let mut progress = manager.file_progress(game_idx).await;

    // Log progress details for debugging
    if let Some(ref p) = progress {
        log::debug!(
            "Progress {}: idx={} {}/{} bytes ({:.1}%) finished={} installed={}",
            title, game_idx, p.downloaded_bytes, p.total_bytes,
            p.progress * 100.0, p.finished, already_installed
        );
    }

    // Attach installed status from DB
    if let Some(ref mut p) = progress {
        p.installed = already_installed;
    }

    // Attach extras (GameData) progress: it keeps downloading after the game
    // itself is installed - without this the second phase is invisible and
    // features that depend on it (manuals, videos) look broken meanwhile.
    if let (Some(ref mut p), Some(gd_idx)) = (progress.as_mut(), gamedata_idx) {
        if manager.is_file_selected(gd_idx).await {
            if let Some(gd) = manager.file_progress(gd_idx).await {
                // Disk fallback mirrors the game-file one above: librqbit's
                // per-file stat can stall short of total for fully-written
                // files, which would pin "downloading extras" forever.
                let disk_done = manager
                    .file_output_path(gd_idx)
                    .and_then(|zp| std::fs::metadata(zp).ok())
                    .is_some_and(|m| gd.total_bytes > 0 && m.len() >= gd.total_bytes);
                p.extras_progress = Some(gd.progress);
                p.extras_done = Some(gd.finished || disk_done);
            }
        } else {
            // Not selected (already complete in an earlier session, or the
            // game has no extras in flight) - report done so the UI settles.
            p.extras_done = Some(true);
        }
    }

    // Disk-based completion fallback: librqbit's in-memory file_progress can
    // stall short of total_bytes for files that are in fact fully written to
    // disk - observed when multiple parallel downloads share a torrent and
    // the per-file stat lags behind actual assembly. The bug manifests as
    // "Waiting for last pieces..." forever, only recovering on app restart
    // (when session state is reloaded from disk). If the target file exists
    // with the expected size, trust the disk over the stats.
    if let Some(ref mut p) = progress {
        if !p.finished && p.total_bytes > 0 && p.progress >= 0.99 {
            if let Some(zip_path) = manager.file_output_path(game_idx) {
                if let Ok(meta) = std::fs::metadata(&zip_path) {
                    if meta.len() >= p.total_bytes {
                        log::info!(
                            "Disk-based completion: {} fully assembled ({} bytes) but librqbit stats lagged at {}. Treating as finished.",
                            title, meta.len(), p.downloaded_bytes
                        );
                        p.downloaded_bytes = p.total_bytes;
                        p.progress = 1.0;
                        p.finished = true;
                    }
                }
            }
        }
    }

    // Extract !DOSmetadata.zip if it just finished downloading (check main eXoDOS manager)
    if let Some(ref main_mgr) = main_mgr_opt {
        if let Some(dosbox_meta) = main_mgr.index().find_dosbox_metadata_zip() {
            if main_mgr.is_file_complete(dosbox_meta.index).await {
                if let Some(zip_path) = main_mgr.file_output_path(dosbox_meta.index) {
                    let lock = zip_path.with_extension("extracted");
                    if zip_path.exists() && !lock.exists() && dosmeta_extract_try_begin() {
                        let torrent_root = main_mgr.torrent_root();
                        tauri::async_runtime::spawn_blocking(move || {
                            let result = (|| -> Result<(), String> {
                                let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
                                let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
                                archive.extract(&torrent_root).map_err(|e| e.to_string())?;
                                std::fs::write(&lock, "").map_err(|e| e.to_string())?;
                                Ok(())
                            })();
                            match &result {
                                Ok(()) => log::info!("Extracted DOSBox configs to {}", torrent_root.display()),
                                Err(e) => log::error!("Failed to extract DOSBox configs: {}", e),
                            }
                            dosmeta_extract_finish(result.is_ok());
                        });
                    }
                }
            }
        }
    }

    // If download is complete and not yet installed, extract the ZIP and mark installed.
    if let Some(ref p) = progress {
        if p.finished && !already_installed {
            let zip_out = manager.file_output_path(game_idx);
            log::debug!(
                "Extraction check for {}: zip_path={:?} exists={}",
                title, zip_out, zip_out.as_ref().map(|p| p.exists()).unwrap_or(false)
            );
            if let Some(zip_path) = zip_out {
                if zip_path.exists() {
                    // ZIP materialised - clear any stuck-download retry counter so a
                    // future stuck cycle on the same game id starts fresh from 0.
                    if let Ok(mut map) = retry_state().lock() {
                        map.remove(&id);
                    }
                    let lock_path = zip_path.with_extension("extracting");

                    // Clean up stale lock files (e.g., from crashed/interrupted extraction)
                    if lock_path.exists() {
                        if let Ok(age) = std::fs::metadata(&lock_path)
                            .and_then(|m| m.modified())
                            .and_then(|t| t.elapsed().map_err(std::io::Error::other))
                        {
                            if age.as_secs() > 300 {
                                log::warn!("Removing stale extraction lock: {}", lock_path.display());
                                let _ = std::fs::remove_file(&lock_path);
                            }
                        }
                    }

                    if std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lock_path)
                        .is_ok()
                    {
                        let extract_dir = zip_path.parent().unwrap().to_path_buf();
                        let game_id = id;
                        let db_path = {
                            let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                            conn.path().map(PathBuf::from)
                                .ok_or_else(|| "Cannot determine database path".to_string())?
                        };

                        tauri::async_runtime::spawn(async move {
                            // Serialize against uninstall/launch/download of the
                            // same game. Without this, Uninstall during a multi-GB
                            // extraction renames the half-extracted dir into !save
                            // (garbage backup) and the extractor then re-creates the
                            // dir and re-marks the game installed AFTER uninstall
                            // cleared the flag.
                            let op_lock = game_op_lock(game_id);
                            let _op_guard = op_lock.lock().await;
                            // Re-check: uninstall/cancel may have removed the ZIP
                            // while we waited for the lock.
                            if !zip_path.exists() {
                                let _ = std::fs::remove_file(&lock_path);
                                return;
                            }
                            log::info!("Extracting {} from {}", title, zip_path.display());
                            let extract_result = {
                                let (z, d) = (zip_path.clone(), extract_dir.clone());
                                tauri::async_runtime::spawn_blocking(move || extract_game_zip(&z, &d)).await
                            };
                            match extract_result {
                                Ok(Ok(())) => match db::open(&db_path) {
                                    Ok(conn) => {
                                        if let Err(e) = queries::set_game_installed(&conn, game_id, true) {
                                            log::error!("Failed to mark {} installed: {}", title, e);
                                        } else {
                                            log::info!("Installed: {}", title);
                                        }
                                    }
                                    Err(e) => log::error!("Failed to open DB for install update: {}", e),
                                },
                                Ok(Err(e)) => {
                                    log::error!("Failed to extract {}: {}", title, e);
                                    // Corrupt/stub ZIP (same detection as the launch
                                    // path): delete it and clear in_library so the
                                    // 1 Hz poll stops re-extracting the same broken
                                    // bytes forever and the user can re-download.
                                    if e.contains("EOCD") || e.contains("invalid Zip") || e.contains("Invalid archive") {
                                        log::warn!(
                                            "ZIP for {} is corrupt/incomplete - removing it so a re-download starts clean",
                                            title
                                        );
                                        let _ = std::fs::remove_file(&zip_path);
                                        if let Ok(conn) = db::open(&db_path) {
                                            let _ = queries::clear_in_library(&conn, game_id);
                                        }
                                    }
                                }
                                Err(e) => log::error!("Extraction task panicked for {}: {}", title, e),
                            }
                            let _ = std::fs::remove_file(&lock_path);
                        });
                    }
                } else {
                    // ZIP not on disk despite torrent reporting 100%.
                    // Common cause: pieces covering this file were received as a side effect of
                    // downloading a neighboring file, but librqbit never assembled the pieces
                    // into the output file. A plain re-select is a no-op when librqbit's view
                    // of the file is "already complete", so we toggle the selection - deselect,
                    // briefly yield, then re-add - to nudge librqbit into re-evaluating the file.
                    // Throttled to one attempt every 5 seconds to avoid spamming the session.
                    log::warn!(
                        "Download reports 100% but ZIP missing: {}. Re-requesting file assembly.",
                        zip_path.display()
                    );
                    let retry_key = id;
                    let now = std::time::Instant::now();
                    // After this many failed retries (~5 min at 5 s intervals), give up
                    // and surface an error so the UI stops polling forever and the user
                    // can take action (cancel + re-download).
                    const MAX_ATTEMPTS: u32 = 60;
                    // Returns (attempts_so_far, did_increment_this_poll). The counter only
                    // ticks every 5 s; in-between polls observe the same value with
                    // did_increment=false, so error/recovery decisions stay stable across
                    // every poll instead of flickering with the throttle window.
                    let (attempts, ticked) = retry_state().lock()
                        .map(|mut map| {
                            // Prune stale entries (>2 minutes idle) to bound memory.
                            map.retain(|_, (t, _)| now.duration_since(*t).as_secs() < 120);
                            // checked_sub: `Instant - Duration` panics when the process
                            // hasn't been alive that long (observed shortly after boot).
                            let seed = now
                                .checked_sub(std::time::Duration::from_secs(60))
                                .unwrap_or(now);
                            let entry = map.entry(retry_key).or_insert((seed, 0));
                            if now.duration_since(entry.0).as_secs() >= 5 {
                                entry.0 = now;
                                entry.1 = entry.1.saturating_add(1);
                                (entry.1, true)
                            } else {
                                (entry.1, false)
                            }
                        })
                        .unwrap_or((0, false));
                    if ticked {
                        if attempts <= MAX_ATTEMPTS {
                            let mgr = manager.clone();
                            tauri::async_runtime::spawn(async move {
                                // Toggle: deselect → tiny pause → re-add. The pause lets librqbit
                                // settle the deselect bookkeeping before the next selection update.
                                mgr.deselect_file(game_idx).await;
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                let _ = mgr.download_files(vec![game_idx]).await;
                            });
                        } else if attempts == MAX_ATTEMPTS + 1 {
                            log::error!(
                                "Giving up on stuck download for game {} ({}) after {} retries; \
                                 surfacing error to UI",
                                id, title, MAX_ATTEMPTS
                            );
                        }
                    }
                    // Show as still in-progress so the frontend keeps polling until the ZIP
                    // appears and extraction can proceed normally - unless we've exhausted retries,
                    // in which case surface an error so the UI can prompt the user to cancel.
                    if let Some(ref mut p) = progress {
                        p.finished = false;
                        if attempts > MAX_ATTEMPTS {
                            p.error = Some(
                                "Download stuck - librqbit reports 100% but the file isn't on disk. \
                                 Cancel and re-download to recover.".to_string()
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(progress)
}

/// Cancel an in-progress download: deselects the file from the torrent, then clears in_library.
/// Deselect happens first so the DB and torrent state stay consistent even if one step fails.
#[tauri::command]
pub async fn cancel_download(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<(), String> {
    let (game_idx, gamedata_idx, source) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;

        // The GameData ZIP is shared with this game's other language
        // variants (LP installs auto-download the EN GameData). Only
        // deselect it when no other in-flight download still needs it.
        let gamedata_idx = match game.gamedata_torrent_index {
            Some(gd) => {
                // in_library alone (not installed=0): a variant whose game
                // ZIP already extracted may still be fetching this GameData.
                // Over-retention for long-installed variants is harmless -
                // their GameData is complete anyway.
                let still_needed: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM games \
                         WHERE gamedata_torrent_index = ?1 AND id != ?2 \
                           AND in_library = 1",
                        rusqlite::params![gd, id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                if still_needed > 0 {
                    log::info!(
                        "cancel_download: keeping shared GameData index {} ({} other download(s) need it)",
                        gd, still_needed
                    );
                    None
                } else {
                    Some(gd as usize)
                }
            }
            None => None,
        };

        (
            game.game_torrent_index.map(|i| i as usize),
            gamedata_idx,
            game.torrent_source.unwrap_or_else(|| "eXoDOS".to_string()),
        )
    };

    // Serialize against a concurrently-running extraction/launch/uninstall of
    // the same game - cancel mid-extraction otherwise clears in_library while
    // the extractor later sets installed=1 (installed-but-not-in-library).
    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;

    // Deselect from torrent first - if this fails silently, we still want to clear the DB flag.
    // Clone Arc before dropping the guard so we don't hold the read lock across awaits.
    {
        let manager_arc = {
            let guard = torrent_state.0.read().await;
            guard.get(&source).cloned()
        };
        if let Some(manager) = manager_arc {
            if let Some(idx) = game_idx {
                manager.deselect_file(idx).await;
            }
            if let Some(idx) = gamedata_idx {
                manager.deselect_file(idx).await;
            }
        }
    }

    // Clear DB flag after torrent deselection.
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::clear_in_library(&conn, id).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Uninstall a game: back up saves, delete game files, free disk space.
#[tauri::command]
pub async fn uninstall_game(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<String, String> {
    let (game, data_dir) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured")?;
        (game, data_dir)
    };

    if !game.installed && !game.in_library {
        // Idempotent cleanup: the UI legitimately offers Uninstall for
        // half-states (incomplete download, failed extraction) where the
        // flags are already clear but files may exist on disk. Proceed and
        // clean whatever is there instead of erroring.
        log::info!(
            "uninstall_game: {} not marked installed - cleaning up leftovers anyway",
            game.title
        );
    }

    if running_games()
        .lock()
        .map(|s| s.contains(&running_game_key(&game)))
        .unwrap_or(false)
    {
        return Err(format!(
            "'{}' is currently running - quit DOSBox before uninstalling.",
            game.title
        ));
    }

    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;

    let shortcode = game.shortcode.as_deref()
        .ok_or("Game has no shortcode")?
        .to_string();

    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let torrent_root = crate::commands::setup::game_root(&data_dir);

    // Get game name from bat filename for ZIP deletion
    let game_name = game.application_path.as_deref()
        .and_then(crate::commands::setup::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());

    // Determine THIS variant's game directory:
    // EN: <game_prefix>/<shortcode>/   LP: <game_prefix>/<lang_dir>/<shortcode>/
    // eXoWin9x: <game_prefix>/<year>/<title dir>/
    // Never probe other languages' dirs - the old first-existing probe over
    // all lang dirs made "uninstall the DE variant" back up and delete the
    // EN install when both were on disk.
    let rel_game_dir =
        collection_rel_game_dir(source, &shortcode, game.application_path.as_deref());
    // Save backup lives NEXT TO the game dir (`.../!save/<shortcode>`), which
    // keeps it lang-scoped for LP variants and year-scoped for eXoWin9x -
    // exactly where extract_game_zip's restore probe looks.
    let rel_save_dir = match rel_game_dir.rsplit_once('/') {
        Some((parent, _)) => format!("{}/!save/{}", parent, shortcode),
        None => format!("!save/{}", shortcode),
    };
    let game_dir: Option<PathBuf> = Some(torrent_root.join(&rel_game_dir)).filter(|d| d.exists());
    let rel_zip = collection_rel_zip(source, &game_name, game.application_path.as_deref());

    let db_path = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.path().map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    let deleted_rels: Vec<String> = tauri::async_runtime::spawn_blocking(move || {
        if let Some(ref dir) = game_dir {
            if dir.exists() {
                // Back up the entire game directory (preserves saves, configs,
                // etc.). LP backups live under the language dir so uninstalling
                // one variant can't clobber another's backup; extract_game_zip's
                // restore probes both the lang-scoped and the legacy shared
                // location. For eXoWin9x the whole dir includes the game's own
                // VHD, which is where its saves live.
                let save_dir = torrent_root.join(&rel_save_dir);
                if save_dir.exists() {
                    let _ = std::fs::remove_dir_all(&save_dir);
                }
                // Rename is the fastest way to "back up" - atomic move
                if let Err(e) = std::fs::rename(dir, &save_dir) {
                    // Rename failed (cross-device?), fall back to copy + delete
                    log::warn!("Rename to save dir failed ({}), falling back to copy", e);
                    if let Err(e) = copy_dir_recursive(dir, &save_dir) {
                        log::error!(
                            "Failed to back up game directory '{}': {} - keeping originals in place",
                            dir.display(), e
                        );
                        // Don't delete the source if backup failed
                    } else {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                }
                log::info!("Backed up saves to {}", save_dir.display());
            }
        }

        // Track which ZIPs actually got deleted (torrent-relative paths) so
        // the caller can reset piece bookkeeping in exactly the torrents
        // that tracked them. Only THIS variant's ZIP - the old all-languages
        // sweep deleted neighbor variants' downloads too.
        let zip_rels = vec![rel_zip];
        let mut deleted_rels: Vec<String> = Vec::new();
        for rel in &zip_rels {
            let zip = torrent_root.join(rel);
            if zip.exists() && std::fs::remove_file(&zip).is_ok() {
                deleted_rels.push(rel.clone());
            }
        }

        if let Ok(conn) = db::open(&db_path) {
            if let Err(e) = queries::set_game_installed(&conn, id, false) {
                log::error!("Failed to update uninstall status: {}", e);
            }
            // Also clear in_library
            let _ = conn.execute(
                "UPDATE games SET in_library = 0 WHERE id = ?1",
                rusqlite::params![id],
            );
        } else {
            log::error!("Failed to open DB for uninstall update");
        }

        deleted_rels
    })
    .await
    .map_err(|e| e.to_string())?;

    // Deleted ZIPs' pieces are still marked "had" in librqbit's fastresume
    // state; a later re-download would report 100% instantly with no file on
    // disk (stuck-download loop). All collections overlay one root, so a
    // deleted ZIP may be tracked by a torrent OTHER than this game's source
    // (e.g. a GLP uninstall also removes the EN ZIP). Reset exactly the
    // torrents that tracked a deleted path.
    let managers: Vec<(String, std::sync::Arc<crate::torrent::manager::DownloadManager>)> = {
        let guard = torrent_state.0.read().await;
        guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };

    // Only deselect this game's shared GameData when no other variant still
    // wants it (mirrors cancel_download).
    let gamedata_drop: Option<usize> = match game.gamedata_torrent_index {
        Some(gd) => {
            let conn = db_state.0.lock().map_err(|e| e.to_string())?;
            let still_needed: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM games \
                     WHERE gamedata_torrent_index = ?1 AND id != ?2 AND in_library = 1",
                    rusqlite::params![gd, id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if still_needed > 0 { None } else { Some(gd as usize) }
        }
        None => None,
    };

    for (col_id, mgr) in managers {
        // Files of this torrent that were genuinely deleted from disk -
        // only these get their ledger pieces cleared.
        let deleted_indices: Vec<usize> = deleted_rels
            .iter()
            .filter_map(|rel| mgr.index().find_by_path(rel).map(|f| f.index))
            .collect();
        let mut drop_indices = deleted_indices.clone();
        let is_source = col_id == source;
        if is_source {
            if let Some(gi) = game.game_torrent_index {
                let gi = gi as usize;
                if !drop_indices.contains(&gi) {
                    drop_indices.push(gi);
                }
            }
            // The shared GameData ZIP stays ON DISK - deselect it (when no
            // sibling needs it) but never clear its ledger pieces, or the
            // next install re-downloads gigabytes it already has.
            if let Some(gd) = gamedata_drop {
                if !drop_indices.contains(&gd) {
                    drop_indices.push(gd);
                }
            }
        }

        let tracked_deleted = !deleted_indices.is_empty();
        if tracked_deleted {
            // Disk state changed under this torrent - full invalidation.
            if let Err(e) = mgr
                .invalidate_after_file_delete(&drop_indices, &deleted_indices)
                .await
            {
                log::warn!("Failed to reset {} torrent state after uninstall: {}", col_id, e);
            }
        } else if is_source {
            // Nothing deleted from this torrent's files; just drop the
            // selection so the re-add doesn't fetch the uninstalled game.
            for idx in drop_indices {
                mgr.deselect_file(idx).await;
            }
        }
    }

    Ok(format!("Uninstalled: {}", game.title))
}

/// Put a game back into the state it had right after installing: drop the
/// extracted directory AND its save backup, then unpack the ZIP again.
///
/// Uninstall deliberately KEEPS user data (§5) - it renames the game dir to
/// `!save/<shortcode>` and `extract_game_zip` restores it on the next
/// install - so "uninstall, then reinstall" cannot produce a clean slate.
/// This is the operation that can.
///
/// It matters beyond savegames for eXoWin9x, where the game's own VHD is
/// mounted as D: and holds the guest's filesystem: Windows clears that
/// volume's FAT "clean shutdown" bit on the first write and only restores it
/// on a proper shutdown, so a session ended by closing the emulator window
/// leaves it dirty and the next boot runs ScanDisk. Replacing the VHD with
/// the one from the ZIP is the only way back to a pristine volume.
///
/// The ZIP is validated BEFORE anything is deleted - a torrent placeholder
/// or a half-downloaded archive must not cost the user their install.
#[tauri::command]
pub async fn reset_game_data(db_state: State<'_, DbState>, id: i64) -> Result<String, String> {
    let (game, data_dir) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured")?;
        (game, data_dir)
    };

    if running_games()
        .lock()
        .map(|s| s.contains(&running_game_key(&game)))
        .unwrap_or(false)
    {
        return Err(format!(
            "'{}' is currently running - quit the emulator before resetting it.",
            game.title
        ));
    }

    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;

    let shortcode = game.shortcode.as_deref().ok_or("Game has no shortcode")?.to_string();
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let torrent_root = crate::commands::setup::game_root(&data_dir);
    let game_name = game
        .application_path
        .as_deref()
        .and_then(crate::commands::setup::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());

    let rel_game_dir =
        collection_rel_game_dir(source, &shortcode, game.application_path.as_deref());
    let rel_save_dir = match rel_game_dir.rsplit_once('/') {
        Some((parent, _)) => format!("{}/!save/{}", parent, shortcode),
        None => format!("!save/{}", shortcode),
    };
    let zip = torrent_root.join(collection_rel_zip(
        source,
        &game_name,
        game.application_path.as_deref(),
    ));
    let game_dir = torrent_root.join(&rel_game_dir);
    let save_dir = torrent_root.join(&rel_save_dir);
    let title = game.title.clone();

    tauri::async_runtime::spawn_blocking(move || {
        // Validate first: opening the archive reads its central directory,
        // which is exactly what distinguishes a real ZIP from librqbit's
        // 0-byte placeholder or a piece-sized fragment.
        let file = std::fs::File::open(&zip).map_err(|_| {
            format!(
                "The ZIP for '{}' is not on disk, so there is nothing to restore from. \
                 Re-download the game instead.",
                title
            )
        })?;
        zip::ZipArchive::new(file).map_err(|_| {
            format!(
                "The ZIP for '{}' is incomplete or corrupted (torrent placeholder), \
                 so there is nothing to restore from. Re-download the game instead.",
                title
            )
        })?;

        if game_dir.exists() {
            std::fs::remove_dir_all(&game_dir)
                .map_err(|e| format!("Failed to remove {}: {e}", game_dir.display()))?;
        }
        // Must go too, or extract_game_zip restores the very data this is
        // meant to discard.
        if save_dir.exists() {
            let _ = std::fs::remove_dir_all(&save_dir);
        }

        let dest = game_dir.parent().map(PathBuf::from).unwrap_or_else(|| torrent_root.clone());
        extract_game_zip(&zip, &dest)
    })
    .await
    .map_err(|e| format!("reset task failed: {e}"))??;

    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let _ = queries::set_game_installed(&conn, id, true);
    }

    log::info!("Reset game data: {}", game.title);
    Ok(format!("Reset {} to its original state", game.title))
}

/// In-flight + failure-backoff guard for the !DOSmetadata.zip extraction.
/// Without it, every 1 Hz progress poll during the (long) extraction spawned
/// another overlapping full extraction - and a corrupt ZIP retried forever.
/// State: (in_flight, last_failure).
static DOSMETA_EXTRACT: OnceLock<Mutex<(bool, Option<std::time::Instant>)>> = OnceLock::new();

fn dosmeta_state() -> &'static Mutex<(bool, Option<std::time::Instant>)> {
    DOSMETA_EXTRACT.get_or_init(|| Mutex::new((false, None)))
}

/// Returns true (and marks in-flight) if an extraction attempt may start now.
fn dosmeta_extract_try_begin() -> bool {
    let Ok(mut state) = dosmeta_state().lock() else { return false };
    if state.0 {
        return false;
    }
    // 5-minute cooldown after a failure so a corrupt ZIP doesn't hot-loop.
    if let Some(failed_at) = state.1 {
        if failed_at.elapsed().as_secs() < 300 {
            return false;
        }
    }
    state.0 = true;
    true
}

fn dosmeta_extract_finish(success: bool) {
    if let Ok(mut state) = dosmeta_state().lock() {
        state.0 = false;
        state.1 = if success { None } else { Some(std::time::Instant::now()) };
    }
}

/// Currently-running games (keyed by shortcode:language, or id when no
/// shortcode exists). Inserted at spawn, removed by the reaper task when
/// DOSBox exits. Lets uninstall refuse while the game's files are open.
fn running_games() -> &'static Mutex<std::collections::HashSet<String>> {
    static RUNNING: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn running_game_key(game: &Game) -> String {
    match game.shortcode.as_deref() {
        Some(sc) => format!("{}:{}", sc, game.language),
        None => format!("id:{}", game.id.unwrap_or(-1)),
    }
}

/// Per-game mutual exclusion for launch / uninstall / download. The old
/// sync-command design serialized these ACCIDENTALLY by freezing the main
/// thread; with async commands the UI stays responsive, so e.g. Uninstall
/// mid-launch-extraction became clickable - and would rename the game dir
/// out from under the extractor, polluting the !save backup. Real locks
/// replace the accidental ones.
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

/// Does this game's DOSBox config ask for MIDI music (MT-32 or General
/// MIDI)? Reads the bundled per-game conf; permissive on read failure so a
/// missing conf never blocks a download decision.
fn game_requests_midi(torrent_root: &std::path::Path, dosbox_conf: Option<&str>) -> bool {
    let Some(rel) = dosbox_conf else { return false };
    // DB paths mix separators ("eXo\eXoDOS\!dos\SQ1VGA/dosbox.conf").
    let rel = rel.replace('\\', "/");
    let Ok(text) = std::fs::read_to_string(torrent_root.join(rel)) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    lower.contains("mididevice")
        || lower.contains("mt32.")
        || lower.contains("fluid.")
        || lower.contains("[mt32]")
        || lower.contains("[fluidsynth]")
}

/// True when the config enables eXo's virtual printer (`printer=true` +
/// `parallel1=printer`, ECE/DOSBox-X keys). Comment lines are skipped: one
/// eXoWin3x config carries the entire option documentation as `#` comments
/// while actually setting `parallel1=disabled`.
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

/// Locate a game's dosbox.conf on disk - the canonical resolution, shared by
/// `launch_game` and `game_printing_unavailable` so they cannot drift. Probes,
/// in order: the game's own collection root, the main eXoDOS root (LP rows
/// inherit the EN conf), and the lang-scoped alternate locations. Returns the
/// conf path plus the torrent root it was found under (`launch_game` derives
/// the working dir from that root).
fn resolve_game_conf(
    data_dir: &str,
    source: &str,
    dosbox_conf: &str,
) -> Option<(PathBuf, PathBuf)> {
    // Normalize Windows backslashes - DB paths mix separators.
    let rel = dosbox_conf.replace('\\', "/");
    let main_root =
        crate::commands::setup::game_root(data_dir);
    let torrent_root = main_root.clone();

    let direct = torrent_root.join(&rel);
    if direct.exists() {
        return Some((direct, torrent_root));
    }

    // For LP games, the dosbox_conf was inherited from the EN game. The config
    // lives in the main eXoDOS data dir, but game files are in the LP dir - the
    // returned root stays the LP one so mounts resolve there.
    if source != "eXoDOS" {
        let main_conf = main_root.join(&rel);
        if main_conf.exists() {
            return Some((main_conf, torrent_root));
        }
    }

    // The config might be under a language-specific subdirectory.
    let main_game_prefix = collection_game_prefix("eXoDOS");
    let main_segment = crate::commands::setup::collection_def("eXoDOS")
        .map(|c| c.shortcode_segment)
        .unwrap_or("!dos");
    if let Some(shortcode) = rel
        .strip_suffix("/dosbox.conf")
        .and_then(|p| p.rsplit('/').next())
        .filter(|s| !s.is_empty())
    {
        let roots = if source != "eXoDOS" {
            vec![torrent_root, main_root]
        } else {
            vec![torrent_root]
        };
        for root in roots {
            for lang_dir in LANG_DIRS {
                let alt = root.join(format!(
                    "{}/{}/{}/{}/dosbox.conf",
                    main_game_prefix, main_segment, lang_dir, shortcode
                ));
                if alt.exists() {
                    return Some((alt, root));
                }
            }
        }
    }
    None
}

/// Which engine a DOS game actually launches under: `Some(path)` is eXo's own
/// DOSBox ECE build, `None` means DOSBox Staging.
///
/// THE one place that answers this. `launch_game` picks the binary here, the
/// detail panel labels the engine from it, and the printing and shader notes
/// state their limitation from it - a second copy of the rule would drift, and
/// a panel promising ECE while Staging runs is worse than no note at all.
///
/// `engine = staging` in a game's own config forces the fallback: ECE has no
/// shader pipeline, so users who want the CRT look on Windows trade eXo's
/// tuned engine for it. Per game and off by default, because eXo picked ECE
/// for a reason - printing among them.
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
    /// Could eXo's DOSBox ECE build run this game here at all? Drives whether
    /// the emulator choice is offered. Ignores the user's override on purpose:
    /// asking `uses_ece` instead made the control vanish the moment someone
    /// picked Staging, with no way back to eXo's choice.
    pub ece_available: bool,
    /// What will ACTUALLY run it, override included. Drives the engine label,
    /// the shader note and the printing note.
    pub uses_ece: bool,
}

/// Which engine this game gets, and whether there is a choice to make.
///
/// ECE has no shader pipeline: `glshader` is a DOSBox Staging feature, and eXo
/// sets it in 750 of its own configs - 675 of them on Staging variants, 16 on
/// ECE. So a CRT setting on an ECE game is not overridden at launch, it is
/// dropped, and the UI has to say so instead of offering a control that does
/// nothing.
///
/// Answered with `launch_game`'s own engine selection rather than the variant
/// alone: ECE exists on Windows and only once extracted from util.zip, so the
/// same game answers false until that build is on disk.
#[tauri::command]
pub async fn game_engine_info(
    db_state: State<'_, DbState>,
    id: i64,
) -> Result<GameEngineInfo, String> {
    let (dosbox_variant, data_dir, per_game_config) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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
    let main_root = crate::commands::setup::game_root(&data_dir);
    Ok(GameEngineInfo {
        ece_available: resolve_engine(dosbox_variant.as_deref(), &main_root, &Default::default())
            .is_some(),
        uses_ece: resolve_engine(dosbox_variant.as_deref(), &main_root, &per_game_config)
            .is_some(),
    })
}

/// Whether this game's printing features will be missing at launch. 13 eXoDOS
/// titles enable eXo's virtual printer (for most of them printing IS the
/// product), and DOSBox Staging has no printer support yet - so the answer is
/// "the conf requests a printer AND the engine that would run is Staging",
/// decided by `resolve_engine`, the same answer `launch_game` acts on. So it
/// flips to false by itself once the ECE build lands on disk - and back to
/// true if the user forces Staging for that game.
#[tauri::command]
pub async fn game_printing_unavailable(
    db_state: State<'_, DbState>,
    id: i64,
) -> Result<bool, String> {
    let (dosbox_conf, dosbox_variant, source, data_dir, per_game_config) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
        let cfg = queries::get_all_game_config(&conn, id).unwrap_or_default();
        (game.dosbox_conf, game.dosbox_variant, game.torrent_source, data_dir, cfg)
    };
    let (Some(conf), Some(data_dir)) = (dosbox_conf, data_dir) else {
        return Ok(false);
    };
    let source = source.unwrap_or_else(|| "eXoDOS".to_string());
    let Some((conf_path, _)) = resolve_game_conf(&data_dir, &source, &conf) else {
        return Ok(false);
    };
    let Ok(text) = std::fs::read_to_string(conf_path) else {
        return Ok(false);
    };
    if !conf_requests_printer(&text) {
        return Ok(false);
    }
    let main_root =
        crate::commands::setup::game_root(&data_dir);
    Ok(resolve_engine(dosbox_variant.as_deref(), &main_root, &per_game_config).is_none())
}

/// Serializes support-file extraction process-wide. A plain lock FILE was
/// racy: the startup rearm and a download-click watcher could both pass the
/// exists() check before either wrote it.
static EXTRACTION_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Extract the mt32 subtree (MT-32/CM32L ROMs incl. rev0, SoundCanvas +
/// AWE64 soundfonts, ~54 MB) from util.zip into `<torrent_root>/eXo/mt32/`.
///
/// util.zip is a matryoshka: the payload sits in a nested EXTDOS.zip whose
/// top-level `mt32/` dir is what the game configs reference as `.\mt32\`.
/// The inner zip (467 MB uncompressed) is staged to a temp file rather than
/// RAM; the rest of it (Windows emulator builds) is never extracted.
///
/// Everything is written to a staging dir first and moved into place with
/// atomic renames - the completion gates test directory EXISTENCE, so a
/// half-written eXo/mt32 from a mid-extraction kill would otherwise read as
/// "done" forever (silent no-music).
fn extract_mt32_from_util_zip(
    util_zip: &std::path::Path,
    torrent_root: &std::path::Path,
) -> Result<usize, String> {
    if EXTRACTION_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("extraction already running".to_string());
    }
    let result = do_extract_support_files(util_zip, torrent_root);
    EXTRACTION_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    result
}

fn do_extract_support_files(
    util_zip: &std::path::Path,
    torrent_root: &std::path::Path,
) -> Result<usize, String> {
    // Unique temp names so a leftover from a killed run can't collide.
    let pid = std::process::id();
    let tmp_path = util_zip.with_extension(format!("extdos_tmp_{pid}"));
    let staging_root = torrent_root.join("eXo").join(format!(".support_staging_{pid}"));

    let result = (|| {
        let file = std::fs::File::open(util_zip).map_err(|e| e.to_string())?;
        let mut outer = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        {
            let mut inner_entry = outer
                .by_name("EXTDOS.zip")
                .map_err(|e| format!("EXTDOS.zip not found inside util.zip: {}", e))?;
            let mut tmp = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut inner_entry, &mut tmp).map_err(|e| e.to_string())?;
        }

        let tmp = std::fs::File::open(&tmp_path).map_err(|e| e.to_string())?;
        let mut inner = zip::ZipArchive::new(tmp).map_err(|e| e.to_string())?;
        // mt32/ everywhere; on Windows also eXo's DOSBox ECE builds so
        // ECE-variant games run their intended emulator.
        let mut prefixes: Vec<&str> = vec!["mt32/"];
        if cfg!(windows) {
            prefixes.push("emulators/dosbox/ece4230/");
            prefixes.push("emulators/dosbox/ece4460/");
        }
        let mut extracted = 0usize;
        for i in 0..inner.len() {
            let mut entry = inner.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            let lower = name.to_ascii_lowercase();
            if !prefixes.iter().any(|p| lower.starts_with(p))
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
            return Err("no mt32/ entries found in EXTDOS.zip".to_string());
        }

        // Move each fully-staged subtree into place. rename is atomic on the
        // same filesystem; a pre-existing (possibly partial, from an older
        // build) destination is replaced.
        let dest_root = torrent_root.join("eXo");
        let mut targets = vec!["mt32".to_string()];
        if cfg!(windows) {
            for v in ["ece4230", "ece4460"] {
                if staging_root.join("emulators/dosbox").join(v).exists() {
                    targets.push(format!("emulators/dosbox/{v}"));
                }
            }
        }
        for rel in targets {
            let src = staging_root.join(&rel);
            if !src.exists() {
                continue;
            }
            let dst = dest_root.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if dst.exists() {
                std::fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
            }
            std::fs::rename(&src, &dst)
                .map_err(|e| format!("moving {} into place: {}", rel, e))?;
        }
        Ok(extracted)
    })();

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_dir_all(&staging_root);
    result
}

/// Re-arm the util.zip extraction watcher after an app restart. A watcher
/// armed by a download click dies with the app; util.zip finishing in a
/// later session would otherwise never extract (observed on Windows: 736 MB
/// downloaded, ROMs never landed). Called from init_download_manager once
/// the eXoDOS manager is hydrated.
pub(crate) async fn rearm_support_extraction(
    mgr: &std::sync::Arc<crate::torrent::manager::DownloadManager>,
) {
    let root = mgr.torrent_root();
    let mt32_missing = !root.join("eXo/mt32").exists();
    let ece_missing = cfg!(windows) && !root.join("eXo/emulators/dosbox/ece4230").exists();
    if !mt32_missing && !ece_missing {
        return;
    }
    let Some(util) = mgr.index().find_by_suffix("util/util.zip") else {
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
        "Re-arming support-file extraction watcher (util.zip {})",
        if selected { "still selected" } else { "present on disk" }
    );
    spawn_mt32_extraction_watcher(std::sync::Arc::clone(mgr), util_index);
}

/// Watch util.zip until it finishes downloading, then extract the mt32
/// payload. Runs as its own task because the frontend only polls progress
/// while a GAME download is active - util.zip (~630 MB) routinely finishes
/// long after the 8 MB game that triggered it, with nobody left polling.
fn spawn_mt32_extraction_watcher(mgr: std::sync::Arc<crate::torrent::manager::DownloadManager>, util_index: usize) {
    tauri::async_runtime::spawn(async move {
        let torrent_root = mgr.torrent_root();
        let mt32_dir = torrent_root.join("eXo/mt32");
        let ece_dir = torrent_root.join("eXo/emulators/dosbox/ece4230");
        let expected_size = mgr.index().files.get(util_index).map(|f| f.size).unwrap_or(0);
        let mut failures = 0u32;
        // Generous ceiling: 6 h at 10 s per check for slow swarms.
        for _ in 0..2160 {
            if mt32_dir.exists() && (!cfg!(windows) || ece_dir.exists()) {
                return; // someone else finished the job
            }
            let Some(zip_path) = mgr.file_output_path(util_index) else {
                return;
            };
            // Stats-based completion PLUS a disk-size fallback: librqbit's
            // per-file stat is known to stall short of total for files fully
            // on disk (see the identical fallback in get_download_progress),
            // and after a restart without session state the handle is None
            // and stats-based completion would never fire at all.
            let stats_complete = mgr.is_file_complete(util_index).await;
            let disk_complete = expected_size > 0
                && std::fs::metadata(&zip_path).is_ok_and(|m| m.len() >= expected_size);
            if stats_complete || disk_complete {
                let root = torrent_root.clone();
                let zp = zip_path.clone();
                let outcome = tauri::async_runtime::spawn_blocking(move || {
                    extract_mt32_from_util_zip(&zp, &root)
                })
                .await;
                match outcome {
                    Ok(Ok(n)) => {
                        log::info!("Extracted {} MT-32/soundfont files from util.zip", n);
                        return;
                    }
                    Ok(Err(e)) if e == "extraction already running" => return,
                    Ok(Err(e)) => {
                        failures += 1;
                        log::error!(
                            "Failed to extract support files from util.zip (attempt {}): {}",
                            failures, e
                        );
                        if failures >= 3 {
                            return;
                        }
                    }
                    Err(e) => {
                        log::error!("Extraction task panicked: {}", e);
                        return;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
        log::warn!("mt32 extraction watcher timed out waiting for util.zip");
    });
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

/// Build the per-launch overlay root for an LP game: a staging dir whose
/// `<shortcode>` entry links to the LP game dir, so the EN config's autoexec
/// ("mount c .\eXoDOS\", "cd <shortcode>", launch) runs unmodified against
/// the LP files. Any other eXoDOS-root entries the autoexec references
/// (shared CD-image folders etc.) get pass-through links to the real tree.
/// Rebuilt from scratch on every launch; contains only links, no data.
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

/// Can the EN autoexec run against the LP dir via the overlay? Simulates the
/// cd chain (a root-level `cd <shortcode>` lands in the LP game dir) and
/// requires the launch command's program to exist at the resulting location.
/// LP variants occasionally restructure the game (renamed executable,
/// different subdirs) - those fall back to the generated-autoexec strategy.
fn lp_autoexec_compatible(
    en_conf: &str,
    shortcode: &str,
    lp_game_dir: &std::path::Path,
    real_root: &std::path::Path,
) -> bool {
    let Some(autoexec) = en_conf.split("[autoexec]").nth(1) else {
        return false;
    };
    // cwd: None = mount root (the overlay staging dir).
    let mut cwd: Option<PathBuf> = None;
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
            let next = match &cwd {
                None => {
                    if target.eq_ignore_ascii_case(shortcode) {
                        lp_game_dir.to_path_buf()
                    } else {
                        // Root-level cd into a non-game entry resolves
                        // through a pass-through link to the real tree.
                        real_root.join(target)
                    }
                }
                Some(dir) => dir.join(target),
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

        // First real command = the launch line. Verify its program exists at
        // the simulated cwd; unrecognizable forms (boot images, drive-letter
        // paths) are trusted - the EN config knows better than any heuristic.
        // Booter games launch via `boot disk.img` - the image path was
        // already handled by the path-rewriting, trust the line as-is.
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
        let dir = match &cwd {
            Some(d) => d.clone(),
            None => return true, // command at mount root - rare, trust it
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

/// Rewrite eXo's `.\`-relative HOST paths, leaving everything else alone.
///
/// `resolve` receives the token after `.\` (e.g. `eXoDOS\SQ5`) and returns the
/// absolute path to substitute. Quoted tokens run to the closing quote so paths
/// with spaces survive; unquoted ones end at the first whitespace.
///
/// The narrow scope is the point: a blanket backslash swap also rewrites text
/// meant for the GUEST. `path=C:\;z:\;c:\windows\` became `path=C:/;...`,
/// which DOS does not resolve, so Windows 3.x could not find `RUNEXIT.EXE` and
/// 1,122 of 1,138 eXoWin3x games died at Program Manager. Every host path in
/// both packs is `.\`-relative (2,149 + 9,691, no exceptions), so this is also
/// complete.
pub(crate) fn rewrite_host_paths(text: &str, resolve: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    while let Some(idx) = rest.find(".\\") {
        out.push_str(&rest[..idx]);
        let quoted = out.ends_with('"');
        let tail = &rest[idx + 2..];
        let end = tail
            .find(|c: char| if quoted { c == '"' } else { c.is_whitespace() })
            .unwrap_or(tail.len());
        out.push_str(&resolve(&tail[..end]));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Patch a DOSBox config file: convert Windows-style relative paths to absolute Linux paths.
/// The eXoDOS configs use `.\eXoDOS\game\` which doesn't work on Linux.
///
/// For LP games, `lp_info` provides the shortcode, language dir, game_folder (the second
/// component of game_prefix, e.g. "eXoDOS"), and the resolved LP game directory path.
/// The EN config runs VERBATIM against an overlay mount whose `<shortcode>` entry links
/// to the LP game dir - preserving eXo's authored launch commands, imgmounts, and
/// utilities. Only when the LP variant's layout is incompatible with the EN autoexec
/// does it fall back to a generated autoexec.
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
    // A `.\` token is only a host path when the target actually exists - eXo
    // also writes `.\` GUEST paths resolved on the mounted drive (11th Hour:
    // `imgmount d ".\cd\11HDISK1.cue"` after `c:` means C:\cd\..., the game
    // dir's own cd folder; there is no eXo/cd on the host). Rewriting those
    // produced dead absolute paths, the imgmounts failed silently, and the
    // game booted without its CDs.
    let to_working_dir = |body: &str| {
        // Bare `.\` is guest text for "current directory" (OxydGold passes it
        // as a program argument) - it would resolve to the working dir, which
        // always exists, so the existence gate alone can't catch it.
        if body.is_empty() {
            return ".\\".to_string();
        }
        let resolved = format!("{}{}", abs_prefix, body.replace('\\', "/"));
        if std::path::Path::new(&resolved).exists() {
            resolved
        } else {
            format!(".\\{}", body)
        }
    };

    let patched = if let Some((shortcode, lang_dir, game_folder, game_dir)) = lp_info {
        // Strategy 1: overlay mount. The EN autoexec is ground truth authored
        // by eXo; the only real difference for an LP install is WHERE the
        // game files live. Point every eXoDOS-root reference at a staging dir
        // whose <shortcode> entry links to the LP game dir and run the config
        // as written. This also shadows an installed EN variant of the same
        // game - the link always wins.
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
                        return staged;
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

/// Translate DOSBox-ECE MIDI settings to DOSBox Staging equivalents.
///
/// ~1,500 eXoDOS configs carry ECE-style dotted keys in [midi]
/// (`mt32.romdir`, `fluid.soundfont`, `fluid.*`) that Staging silently
/// ignores - MT-32 and General-MIDI games then play with wrong or no music.
/// Staging expects the same settings in dedicated [mt32] / [fluidsynth]
/// sections, so: capture the ECE values, drop the dotted keys, and append
/// the Staging sections (unless the config already has them - the ~750
/// Staging-authored eXoDOS configs pass through unchanged). Also maps
/// `mididevice = default` (ECE) to Staging's `auto`.
///
/// Runs after the path rewriting in patch_dosbox_conf, so captured values
/// like `.\mt32` are already absolute forward-slash paths.
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

/// Translate DOSBox-X style IDE controller requests for DOSBox Staging.
///
/// 55 eXoWin3x configs enable `[ide, primary]` / `[ide, secondary]` so the
/// guest OS booted from an HDD image reaches the CD through its own ATAPI
/// driver (VIDE-CDD.SYS + MSCDEX live inside the image - after `boot` DOSBox's
/// DOS-level MSCDEX shim is gone). Staging has no `[ide]` section but provides
/// the same thing as an `-ide` flag on `imgmount ... -t cdrom|iso`, so: when
/// the section is present, add the flag to CD imgmounts and normalize
/// DOSBox-X's slot argument (`-ide 2m`) to Staging's bare flag. Measured
/// (issue #15): without this the guest boots but never sees the CD; with it
/// the ATAPI drive attaches and CD playback works.
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
        // Standalone `-ide` flag only: the line holds an absolute REWRITTEN
        // host path at this point, and a data dir like `/mnt/games-ide/` must
        // not read as "flag already present" (that would silently disable the
        // whole translation). `to_ascii_lowercase` never changes byte offsets,
        // so positions found in `lower` index into `line` directly.
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

/// Find the launch command for an LP game by inspecting its directory.
/// Prefers the launch command named by the EN config's autoexec (when given),
/// then parses run.bat to extract the actual game executable, since run.bat
/// itself is a LaunchBox-specific menu script not suitable for DOSBox autoexec.
/// Returns (subdir, command) if found.
fn find_lp_launch(game_dir: &std::path::Path, en_conf: Option<&str>) -> Option<(String, String)> {
    // Strategy 0: the EN autoexec names the real launcher ("cd cobmiss" then
    // "@cm") - by far the strongest signal, and the only one that works for
    // games with a bare root-level EXE and no .bat (e.g. Cobra Mission ES:
    // CM.EXE + INSTALL.EXE, nothing else runnable). Use the first
    // non-housekeeping command if the referenced program exists in the LP dir.
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

    // Strategy 4: Look for a .exe in subdirectories, then the game dir root
    // (skip utilities and installers). Subdirs first to keep the historical
    // preference; the root pass catches games like Cobra Mission ES whose
    // only executable sits at the top level.
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

/// Copy a directory tree. Returns Err when ANY entry failed - callers that
/// delete the source after a "successful" backup must see partial copies as
/// failures, otherwise locked/unreadable files are silently lost.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {}", dst.display(), e))?;
    let mut failures = 0usize;
    let mut first_err = String::new();
    let mut fail = |msg: String| {
        log::warn!("{}", msg);
        if first_err.is_empty() {
            first_err = msg;
        }
        failures += 1;
    };
    for entry in walkdir::WalkDir::new(src) {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                fail(format!("Walk error under {}: {}", src.display(), err));
                continue;
            }
        };
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.path().is_dir() {
            if let Err(e) = std::fs::create_dir_all(&target) {
                fail(format!("Failed to create dir {}: {}", target.display(), e));
            }
        } else if let Err(e) = std::fs::copy(entry.path(), &target) {
            fail(format!("Failed to copy {} -> {}: {}", entry.path().display(), target.display(), e));
        }
    }
    if failures > 0 {
        return Err(format!(
            "{} item(s) failed while copying {} (first: {})",
            failures, src.display(), first_err
        ));
    }
    Ok(())
}

/// Extract a game ZIP in place, then restore saves from !save/ if available.
pub(crate) fn extract_game_zip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    // Get the top-level directory name from the ZIP (the shortcode). Scan
    // for the first entry that actually has a directory component - entry 0
    // being a root-level file would otherwise silently skip save restore.
    let shortcode = (0..archive.len()).find_map(|i| {
        let entry = archive.by_index(i).ok()?;
        let name = entry.name();
        let (first, rest) = name.split_once('/')?;
        let _ = rest;
        Some(first.to_string())
    });

    archive.extract(dest).map_err(|e| e.to_string())?;
    log::info!("Extracted: {} -> {}", zip_path.display(), dest.display());

    // Restore saves if available
    // Saves are at !save/<shortcode>/ which could be:
    // - In dest itself (e.g., dest = .../eXo/eXoDOS/, saves at .../eXo/eXoDOS/!save/SQ5/)
    // - Or relative to the game dir's grandparent for LP games
    if let Some(sc) = shortcode {
        let game_dir = dest.join(&sc);
        // Search for !save in dest and parent directories
        let save_candidates = [
            dest.join(format!("!save/{}", sc)),
            dest.parent().map(|p| p.join(format!("!save/{}", sc))).unwrap_or_default(),
        ];
        for save_dir in &save_candidates {
            if save_dir.exists() && game_dir.exists() {
                log::info!("Restoring saves from {}", save_dir.display());
                if let Err(e) = copy_dir_recursive(save_dir, &game_dir) {
                    log::warn!("Save restore incomplete for {}: {}", game_dir.display(), e);
                }
                break;
            }
        }
    }

    Ok(())
}

/// Resolve the DOSBox Staging binary path.
/// Tauri's `externalBin` places sidecars at different locations per platform:
///  - macOS: Exodium.app/Contents/MacOS/dosbox-staging (next to the main binary)
///  - Windows: <install_dir>/dosbox-staging.exe (next to the main .exe)
///  - Linux (AppImage/deb): resources/dosbox-staging (inside the resource dir)
///
/// So we check `current_exe().parent()` AND `resource_dir()`, then fall back to PATH.
fn resolve_dosbox(app: &AppHandle) -> PathBuf {
    use tauri::Manager;
    let bin = if cfg!(windows) { "dosbox-staging.exe" } else { "dosbox-staging" };

    // 1. resource_dir/dosbox-bin/ - the canonical location since v0.6.6 on
    //    Windows, where the .exe MUST live alongside its bundled DLLs
    //    (SDL2.dll, vcruntime140.dll, …) plus DOSBox's `resources/` codepage
    //    folder for Windows DLL search to find them. On macOS/Linux this
    //    directory only contains a `.placeholder`, so the lookup falls
    //    through to the externalBin location below.
    //
    //    In dev mode resource_dir is src-tauri/, and the staged bundle lives
    //    one level deeper at src-tauri/resources/dosbox-bin/ (only flattened
    //    to <resource_dir>/dosbox-bin/ at bundle time). Check both so dev on
    //    Windows finds the DLL-adjacent .exe instead of falling through to
    //    the bare externalBin in binaries/, which would fail with missing-DLL
    //    errors when DOSBox tries to start.
    // Subdirs to search under resource_dir, in priority order. Bundled layout
    // (production) flattens to `dosbox-bin/`; dev layout keeps the staged
    // `resources/dosbox-bin/` source path. Update both if the bundle config
    // moves the binary directory.
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

/// Install DOSBox Staging glshaders into the user config dir if missing.
///
/// DOSBox aborts at startup with "Fallback shader 'interpolation/bilinear'
/// not found" unless it finds glshaders in one of its search paths. The
/// shader pack is bundled as a Tauri resource (`bundle.resources` maps
/// `resources/dosbox-glshaders` → `glshaders` inside resource_dir). On
/// macOS that alone is enough: the sidecar searches
/// `Contents/MacOS/../Resources/glshaders` natively. The copy into the
/// user config dir here covers Linux/Windows and acts as a fallback.
///
/// The presence check targets the mandatory fallback shader file, NOT the
/// directory: an empty `glshaders/` dir (seen in the wild on macOS, cause
/// unknown) used to short-circuit the install forever and DOSBox then
/// aborted on every launch with CRT enabled.
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
    let conn = state.0.lock().map_err(|e| e.to_string())?;
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
    let conn = state.0.lock().map_err(|e| e.to_string())?;
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
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_recently_played(&conn, limit.unwrap_or(12)).map_err(|e| e.to_string())
}

/// Launch a downloaded game via DOSBox Staging.
/// Where the per-launch DOSBox config fragments live.
///
/// NOT the game data dir: that is a location the user picked for their games,
/// and these files accumulated there one per game ever launched (15 of them in
/// one report) with nothing ever cleaning them up. They are derived from
/// settings and rewritten on every launch, so the app's own directory is the
/// right home - and `sweep_legacy_launch_confs` removes the old ones.
pub(crate) fn launch_conf_dir(app: &AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("launch");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Remove `*.conf` files from `dir`, optionally only those with `prefix`.
/// Non-recursive and never touches subdirectories - both callers clean a flat
/// directory that also holds files belonging to someone else.
fn remove_conf_files(dir: impl AsRef<Path>, prefix: Option<&str>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".conf") {
            continue;
        }
        if prefix.is_some_and(|p| !name.starts_with(p)) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Delete launch fragments left in the game data dir by earlier versions.
/// They are regenerated on demand, so removing them loses nothing.
pub fn sweep_legacy_launch_confs(data_dir: &str) {
    let removed = remove_conf_files(data_dir, Some("exodium_"));
    if removed > 0 {
        log::info!("Removed {} stray launch config(s) from the game folder", removed);
    }
}

/// Empty the launch-config dir at startup.
///
/// One fragment is written per game ever launched and it is only read while
/// DOSBox starts up, so without this they pile up forever - the same unbounded
/// growth that made them a problem in the game folder. Startup is the safe
/// moment: no game this instance launched is running yet.
pub fn prune_launch_confs(app: &AppHandle) {
    let Ok(dir) = launch_conf_dir(app) else { return };
    let removed = remove_conf_files(&dir, None);
    if removed > 0 {
        log::debug!("Cleared {} stale launch config(s)", removed);
    }
}

#[tauri::command]
pub async fn launch_game(app: AppHandle, db_state: State<'_, DbState>, id: i64) -> Result<String, String> {
    // Serialize against uninstall/download of the same game (see game_op_lock).
    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;
    // Read everything we need from the DB and drop the lock before the heavy
    // DOSBox path resolution + process spawning below.
    let (game, data_dir, crt_auto_enabled, fullscreen_enabled, per_game_config) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured. Run setup first.")?;
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

    // Refuse a second launch while the game is still running. Two emulator
    // instances on the same VHDs corrupt them (86Box recreates the shared
    // child mid-flight, DOSBox-X double-mounts the save drive) - and for the
    // rest of the catalogue a double launch is never what the user meant.
    if running_games()
        .lock()
        .map(|s| s.contains(&running_game_key(&game)))
        .unwrap_or(false)
    {
        return Err(format!("'{}' is already running.", game.title));
    }

    // Win9x games boot Windows 95/98 from VHDs inside DOSBox-X or 86Box -
    // they have their own engine pipeline and none of the Staging conf
    // machinery below applies (their confs run verbatim, §10a).
    if crate::commands::setup::collection_def(game.torrent_source.as_deref().unwrap_or("eXoDOS"))
        .is_some_and(|c| c.year_subdirs)
    {
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
    let main_torrent_root = crate::commands::setup::game_root(&data_dir);
    let torrent_root = main_torrent_root.clone();
    // working_dir is the first path component of game_prefix (e.g. "eXo")
    let working_dir_name = src_game_prefix.split('/').next().unwrap_or("eXo");
    let options_conf = main_torrent_root.join("eXo/emulators/dosbox/options.conf");

    let Some((game_conf, conf_root)) = resolve_game_conf(&data_dir, source, dosbox_conf) else {
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

    // Auto-extract ZIP on first launch if the game directory doesn't exist yet.
    // This mirrors LaunchBox's on-demand extraction behavior and handles games that were
    // imported from an existing installation where ZIPs haven't been extracted.
    if !shortcode.is_empty() {
        let game_dir = torrent_root.join(collection_rel_game_dir(
            source,
            shortcode,
            game.application_path.as_deref(),
        ));
        if !game_dir.exists() {
            let game_name = game.application_path.as_deref()
                .and_then(crate::commands::setup::game_name_from_app_path)
                .unwrap_or_else(|| game.title.clone());
            // LP ZIPs live under the collection's language dir
            // ("eXo/eXoDOS/<lang>/<name>.zip"), eXoWin9x's under the year dir;
            // EN under the prefix root as a fallback.
            let mut zip_candidates: Vec<PathBuf> = vec![torrent_root.join(collection_rel_zip(
                source,
                &game_name,
                game.application_path.as_deref(),
            ))];
            zip_candidates.push(torrent_root.join(format!("{}/{}.zip", src_game_prefix, game_name)));

            if let Some(zip_path) = zip_candidates.iter().find(|z| z.exists()) {
                log::info!("Auto-extracting {} before launch", zip_path.display());
                // Extract next to the ZIP so the game dir lands where the
                // game_dir probe above expects it (lang dir for LP, prefix
                // root for EN).
                let dest = zip_path
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| torrent_root.join(src_game_prefix));
                // spawn_blocking: a multi-GB unzip must not pin a tokio
                // worker (matches the extraction pattern in
                // get_download_progress / uninstall).
                let extract_result = {
                    let (z, d) = (zip_path.clone(), dest.clone());
                    tauri::async_runtime::spawn_blocking(move || extract_game_zip(&z, &d))
                        .await
                        .map_err(|e| format!("extraction task failed: {e}"))?
                };
                if let Err(e) = extract_result {
                    let msg = e.to_string();
                    if msg.contains("EOCD") || msg.contains("invalid Zip") || msg.contains("Invalid archive") {
                        // ZIP is a torrent stub or corrupted file - reset installed flag so the
                        // user can re-download rather than hitting this error on every launch.
                        if let Ok(conn) = db_state.0.lock() {
                            let _ = queries::set_game_installed(&conn, id, false);
                        }
                        return Err(format!(
                            "Game ZIP for '{}' is incomplete or corrupted (torrent placeholder). \
                             Please re-download the game.",
                            game.title
                        ));
                    }
                    return Err(format!("Failed to extract game before launch: {}", msg));
                }
            } else {
                return Err(format!(
                    "Game files not found for '{}'. The game may need to be re-downloaded.",
                    game.title
                ));
            }
        }
    }
    let lp_info = collection_lang_dir(source).map(|ld| {
        let dir = torrent_root.join(format!("{}/{}/{}", src_game_prefix, ld, shortcode));
        (shortcode, ld, game_folder, dir)
    });

    // Engine selection: on Windows, ECE-variant games run eXo's actual
    // DOSBox ECE build (extracted from util.zip's EXTDOS.zip into
    // eXo/emulators/dosbox/<variant>/). Everywhere else - and until the
    // build is on disk - DOSBox Staging is the best-effort fallback.
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

    // DOSBox Staging aborts at startup when it can't find glshaders and CRT
    // is enabled (glshader defaults to crt-auto, which is also OUR default).
    // Run unconditionally on every platform: the check is a single stat once
    // installed, and it self-repairs the empty-glshaders-dir state that
    // bricked launches on fresh macOS installs (v0.8.3).
    ensure_dosbox_shaders(&app);

    let dosbox_bin = ece_bin.unwrap_or_else(|| resolve_dosbox(&app));
    let mut cmd = Command::new(&dosbox_bin);
    cmd.current_dir(&working_dir)
        .arg("-conf")
        .arg(&patched_conf);

    if options_conf.exists() {
        cmd.arg("-conf").arg(&options_conf);
    }

    // macOS with CRT off: force `output = texture` (SDL hardware renderer, no
    // shader pipeline) via a last-wins conf fragment. Shaders are bundled at
    // Contents/Resources/glshaders since 0.8.4 so this is no longer required
    // to avoid the missing-shader abort, but texture output is the
    // long-proven macOS path for the non-CRT look, so keep it.
    #[cfg(target_os = "macos")]
    {
        if !crt_auto_enabled {
            let conf_path = launch_conf_dir(&app)?.join(format!("macos_dosbox_{}.conf", id));
            std::fs::write(&conf_path, "[sdl]\noutput = texture\n")
                .map_err(|e| format!("Failed to write macOS override conf: {e}"))?;
            cmd.arg("-conf").arg(&conf_path);
        }
    }

    // Global user-preference overrides (all platforms, applied LAST so they win
    // against per-game and options.conf settings). Always written and always
    // authoritative - for BOTH the on and off states. Reason: in DOSBox Staging
    // 0.82+ the default `glshader` is `crt-auto`, and ~90% of eXoDOS per-game
    // configs don't explicitly set glshader, so without an active "off" override
    // the user's unchecked CRT toggle would still get crt-auto from Staging's
    // default. Same logic applies to fullscreen - write the explicit value so
    // the user's UI state always wins, regardless of what eXoDOS configs or
    // DOSBox defaults say.
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
                // Nothing to say for this game - drop a fragment an earlier
                // configuration left behind. Keyed on the fragment, not on the
                // config being empty: `engine` is stored in the same table but
                // never written here, so a game left with only that setting
                // would keep a stale file forever.
                let _ = std::fs::remove_file(&game_conf_path);
            } else {
                std::fs::write(&game_conf_path, &frag)
                    .map_err(|e| format!("Failed to write per-game conf: {e}"))?;
                cmd.arg("-conf").arg(&game_conf_path);
            }
        }
    }

    spawn_emulator_and_track(cmd, &dosbox_bin, &game, id)
}

/// Variables the AppImage runtime and linuxdeploy's GTK/GStreamer hooks export
/// that point ONLY into `$APPDIR`. Dropped wholesale for emulator children;
/// unset, each falls back to its spec default, which is what a packaged install
/// (.deb/.rpm) gives them anyway.
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

/// Strip the AppImage's environment from an emulator child.
///
/// Our AppImage's AppRun exports `LD_LIBRARY_PATH` plus the GTK/GStreamer/GIO
/// overrides from linuxdeploy's hooks, and every child inherits them. An
/// emulator started that way loads a MIX of the AppImage's bundled libraries
/// (built against Ubuntu 22.04 / glib 2.72) and the host's current ones, which
/// hangs in library teardown when the emulator window is closed - the process
/// never exits and the desktop offers to kill it.
///
/// Gated on `APPIMAGE` being set, so .deb/.rpm installs (which run against the
/// host's libraries to begin with) are untouched. The Win9x emulator packs are
/// themselves sharun-based AppImages that rebuild their own environment on
/// start, so a clean env is correct for them too; DOSBox Staging, a plain
/// binary, is the main beneficiary.
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

/// Platform-correct stdio setup, spawn and child-reaping for an emulator
/// process. Shared by the Staging/ECE path above and the Win9x engines
/// (DOSBox-X / 86Box) - the macOS EBADF workarounds and the per-game log
/// capture must not fork per engine.
pub(crate) fn spawn_emulator_and_track(
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

    // Stdio handling differs by platform:
    //
    // macOS: Tauri 2 GUI builds were observed returning EBADF from posix_spawn
    // when stdout/stderr used Stdio::from(File) (dup2-based file_actions). We
    // null all three streams there. DOSBox Staging on macOS writes its own
    // logs into ~/Library/Preferences/DOSBox/, so the diagnostic surface is
    // preserved.
    //
    // Linux/Windows: keep the per-game log file capture introduced for Issue
    // #4 ("started then closed" crashes). On Windows in particular, DOSBox
    // doesn't write a user-accessible log otherwise, so dropping this would
    // be a diagnostic regression.
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
        if let Some(log_dir) = crate::commands::setup::LOG_DIR.get() {
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

    // macOS-only: force fork+exec instead of posix_spawn via a no-op pre_exec.
    // posix_spawn was the EBADF source on Tauri 2 GUI builds; fork+exec is more
    // permissive about parent fd state. Linux doesn't have the bug and would
    // pay a perf cost from skipping posix_spawn, so we don't apply it there.
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

    // Reap the child (dropped Child handles become zombies on Unix) and track
    // the running game so uninstall can refuse while the emulator holds its
    // files open - deleting/renaming a live game dir on Windows fails
    // per-file and used to silently lose saves through the copy fallback.
    let run_key = running_game_key(game);
    running_games().lock().map(|mut s| s.insert(run_key.clone())).ok();
    tauri::async_runtime::spawn_blocking(move || {
        match child.wait() {
            Ok(status) => log::info!("Emulator exited ({}) for {}", status, run_key),
            Err(e) => log::warn!("Emulator wait failed for {}: {}", run_key, e),
        }
        running_games().lock().map(|mut s| s.remove(&run_key)).ok();
    });

    Ok(format!("Launched: {}", game.title))
}

#[cfg(test)]
mod tests {
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

    // Both cleanups run against directories that also hold the user's own
    // files, so "only ours, only .conf, never recurse" is the contract.
    #[test]
    fn remove_conf_files_respects_prefix_and_extension() {
        let dir = std::env::temp_dir().join(format!("exodium_conf_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["exodium_a.conf", "global_overrides_7.conf", "dosbox.conf", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("sub").join("exodium_nested.conf"), b"x").unwrap();

        assert_eq!(super::remove_conf_files(&dir, Some("exodium_")), 1);
        assert!(!dir.join("exodium_a.conf").exists());
        // The game's own dosbox.conf must survive a prefixed sweep.
        assert!(dir.join("dosbox.conf").exists());
        assert!(dir.join("sub").join("exodium_nested.conf").exists());

        assert_eq!(super::remove_conf_files(&dir, None), 2);
        assert!(dir.join("notes.txt").exists());
        assert_eq!(super::remove_conf_files(&dir, None), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rel_paths_nest_win9x_games_under_their_year_dir() {
        let app = Some(r"eXo\eXoWin9x\!win9x\1995\Connect4 (1995)\Connect4 (1995).bat");
        assert_eq!(
            super::collection_rel_game_dir("eXoWin9x", "Connect4 (1995)", app),
            "eXo/eXoWin9x/1995/Connect4 (1995)"
        );
        assert_eq!(
            super::collection_rel_zip("eXoWin9x", "Connect4 (1995)", app),
            "eXo/eXoWin9x/1995/Connect4 (1995).zip"
        );
        // A malformed path falls back to the flat layout instead of panicking.
        assert_eq!(
            super::collection_rel_game_dir("eXoWin9x", "Connect4 (1995)", None),
            "eXo/eXoWin9x/Connect4 (1995)"
        );
    }

    #[test]
    fn rel_paths_keep_flat_and_lang_layouts_for_other_packs() {
        let app = Some(r"eXo\eXoDOS\!dos\SQ5\Space Quest V.bat");
        assert_eq!(
            super::collection_rel_game_dir("eXoDOS", "SQ5", app),
            "eXo/eXoDOS/SQ5"
        );
        assert_eq!(
            super::collection_rel_game_dir("eXoDOS_GLP", "SQ5", app),
            "eXo/eXoDOS/!german/SQ5"
        );
        assert_eq!(
            super::collection_rel_zip("eXoDOS_GLP", "Space Quest V", app),
            "eXo/eXoDOS/!german/Space Quest V.zip"
        );
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

    // ── collection_data_dir ──────────────────────────────────────────────────

    #[test]
    fn collection_data_dir_exodos_is_root() {
        let dir = collection_data_dir("/data", "eXoDOS");
        assert_eq!(dir, std::path::PathBuf::from("/data"));
    }

    #[test]
    fn collection_data_dir_glp_is_root() {
        let dir = collection_data_dir("/data", "eXoDOS_GLP");
        assert_eq!(dir, std::path::PathBuf::from("/data"));
    }

    #[test]
    fn collection_data_dir_slp_is_root() {
        let dir = collection_data_dir("/data", "eXoDOS_SLP");
        assert_eq!(dir, std::path::PathBuf::from("/data"));
    }

    // ── patch_dosbox_conf ────────────────────────────────────────────────────

    fn write_conf(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    /// eXoWin3x boots Windows 3.x and hands Program Manager `runexit <prog>`,
    /// which it resolves over the DOS PATH the autoexec just set. That PATH is
    /// guest-side text: rewriting its backslashes turned it into
    /// `path=C:/;z:/;c:/windows/`, which DOS does not resolve, and every such
    /// game (1,122 of 1,138) died at "Cannot find file 'runexit'".
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

    /// 11th Hour (DE) shape: eXo's imgmount lines can be GUEST paths - after
    /// `c:`, `imgmount d ".\cd\11HDISK1.cue"` means C:\cd\... on the mounted
    /// drive, and no eXo/cd exists on the host. Rewriting them to absolute
    /// host paths made every imgmount fail and the game booted without CDs.
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

    /// The three-step conf probe launch_game and game_printing_unavailable
    /// share: own collection root, main eXoDOS root, lang-scoped alternates -
    /// in that order. eXoWin3x is the collection whose root actually differs
    /// from the main one (inner_folder "eXoWin3x"); the eXoDOS-family packs
    /// Every collection resolves inside the SINGLE root - eXo's merged
    /// layout, where `eXo/eXoDOS`, `eXo/eXoWin3x` and `eXo/eXoWin9x` are
    /// siblings. The old per-torrent roots (and the cross-root fallback that
    /// went with them) are gone.
    #[test]
    fn resolve_game_conf_probe_order() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_string_lossy().into_owned();
        let rel = "eXo/eXoWin3x/!win3x/GeoGeo/dosbox.conf";
        let root = tmp.path().join(crate::commands::setup::DEFAULT_ROOT_FOLDER);

        // Nothing on disk: no result.
        assert!(resolve_game_conf(&data_dir, "eXoWin3x", rel).is_none());

        // A Win3x conf is found in the one root, not in a tree of its own.
        let conf_path = root.join(rel);
        fs::create_dir_all(conf_path.parent().unwrap()).unwrap();
        fs::write(&conf_path, "[autoexec]\n").unwrap();
        let (conf, found_root) = resolve_game_conf(&data_dir, "eXoWin3x", rel).unwrap();
        assert_eq!(conf, conf_path);
        assert_eq!(found_root, root);

        // Lang-scoped alternate (LP rows): conf only under a language subdir.
        let lang_conf = root.join("eXo/eXoDOS/!dos/!german/DasAmt/dosbox.conf");
        fs::create_dir_all(lang_conf.parent().unwrap()).unwrap();
        fs::write(&lang_conf, "[autoexec]\n").unwrap();
        let (conf, found_root) =
            resolve_game_conf(&data_dir, "eXoDOS_GLP", "eXo/eXoDOS/!dos/DasAmt/dosbox.conf")
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
