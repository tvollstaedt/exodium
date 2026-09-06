use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;


use crate::db;
use crate::db::queries;
use crate::import;
use crate::torrent::manager::{fastresume_dir, DownloadManager, DownloadProgress};
use crate::torrent::TorrentIndex;

use super::DbState;
use super::library::{match_torrent_indices, scan_installed_games_with_db};
use super::paths::{bundled_metadata_dir, bundled_torrent_path, game_root, is_os_metadata, load_root_folder, set_root_folder, DEFAULT_ROOT_FOLDER, LOG_DIR};
use super::collections::{collection_def, CollectionDef, COLLECTION_MAP};


/// Config value of `network_mode` that keeps the torrent engine shut down.
pub(crate) const OFFLINE_MODE: &str = "offline";

/// True when the user picked "offline" during setup (or later in Settings):
/// no librqbit session is created, nothing is downloaded, nothing is shared.
/// A missing key means "live" so installs from before this setting keep
/// behaving as they did.
pub(crate) fn is_offline(db: &std::sync::Mutex<rusqlite::Connection>) -> bool {
    db.lock()
        .ok()
        .and_then(|conn| queries::get_config(&conn, "network_mode").ok().flatten())
        .as_deref()
        == Some(OFFLINE_MODE)
}

/// Sharing is opt-in: only an explicit "1" counts. Split out from
/// `apply_transfer_preferences` so the rule can be tested without a session.
pub(crate) fn seeding_enabled(db: &std::sync::Mutex<rusqlite::Connection>) -> bool {
    db.lock()
        .ok()
        .and_then(|conn| queries::get_config(&conn, "seeding_enabled").ok().flatten())
        .as_deref()
        == Some("1")
}

/// The user's transfer caps in KB/s as `(upload, download)`, `None` meaning
/// unlimited. Anything unparseable or zero reads as unlimited: a stored "0"
/// would otherwise mean "throttle to nothing", which is not a setting the UI
/// can offer back out again.
pub(crate) fn rate_limits(db: &std::sync::Mutex<rusqlite::Connection>) -> (Option<u32>, Option<u32>) {
    let read = |key: &str| -> Option<u32> {
        let conn = db.lock().ok()?;
        let raw = queries::get_config(&conn, key).ok().flatten()?;
        raw.parse::<u32>().ok().filter(|v| *v > 0)
    };
    (read("rate_limit_up_kbps"), read("rate_limit_down_kbps"))
}

/// Extract the bundled emulator configs for every enabled collection. This is
/// all `init_download_manager` does in offline mode, so it is the whole offline
/// path in one testable place.
fn extract_all_bundled_configs(
    collections: &[&str],
    metadata_dir: Option<&PathBuf>,
    data_path: &Path,
) {
    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        extract_bundled_configs(col, metadata_dir, &game_root(&data_path.to_string_lossy()));
    }
}

/// Serialisable summary returned by the `get_available_collections` command.
#[derive(Debug, Serialize)]
pub struct CollectionInfo {
    pub id: String,
    pub display_name: String,
    pub torrent_file: String,
    /// Catalogue rows in this collection (variant rows, not merged groups) -
    /// rendered on the collection shelf cards.
    pub game_count: i64,
}

/// Return the list of all known collections (for the frontend to render
/// collection pickers / labels without hardcoding IDs).
#[tauri::command]
pub async fn get_available_collections(
    db_state: State<'_, DbState>,
) -> Result<Vec<CollectionInfo>, String> {
    // NULL torrent_source rows (a handful of unmatched variants and the pack
    // sentinel) are deliberately NOT attributed to eXoDOS: the grid's
    // collection filter can't reach them, so counting them would make the
    // shelf number disagree with the grid it opens.
    let counts: std::collections::HashMap<String, i64> = {
        let conn = db_state.lock()?;
        let mut stmt = conn
            .prepare("SELECT torrent_source, COUNT(*) FROM games GROUP BY torrent_source")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok())
            .filter_map(|(k, v)| k.map(|k| (k, v)))
            .collect()
    };
    Ok(COLLECTION_MAP
        .iter()
        .map(|c| CollectionInfo {
            id: c.id.to_string(),
            display_name: c.display_name.to_string(),
            torrent_file: c.torrent_file.to_string(),
            game_count: counts.get(c.id).copied().unwrap_or(0),
        })
        .collect())
}

/// Return the directory where Exodium writes its log file. Served from the
/// `LOG_DIR` cache populated in `lib.rs::run` to avoid re-resolving via
/// `app.path().app_log_dir()` at command time - that round-trip was observed
/// failing silently on shipped Windows builds while the setup-time call
/// succeeded.
#[tauri::command]
pub async fn get_log_dir(app: AppHandle) -> Result<String, String> {
    if let Some(dir) = LOG_DIR.get() {
        return Ok(dir.to_string_lossy().into_owned());
    }
    // Fallback: try the live resolver. If the cache isn't populated (init
    // bug or test harness), this still works in dev mode.
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("app_log_dir failed: {e}"))?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Open the log folder in the user's file explorer. Bypasses the frontend's
/// `getLogDir() + openPath()` two-step which was observed leaving the UI
/// "Resolving…" forever in shipped Windows builds - by doing both lookup
/// and open server-side, the UI just calls one command and either succeeds
/// or sees the error.
#[tauri::command]
pub async fn open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = match LOG_DIR.get() {
        Some(d) => d.clone(),
        None => app
            .path()
            .app_log_dir()
            .map_err(|e| format!("app_log_dir failed: {e}"))?,
    };
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create log folder: {e}"))?;
    }
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(&dir).spawn();
    result
        .map(|_| ())
        .map_err(|e| format!("Failed to open log folder {}: {e}", dir.display()))
}

/// Managed state for the download system - supports multiple torrents.
pub struct TorrentState(pub RwLock<std::collections::HashMap<String, Arc<DownloadManager>>>);

#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub phase: String,
    pub metadata_progress: Option<DownloadProgress>,
    pub dosbox_metadata_progress: Option<DownloadProgress>,
    pub games_imported: usize,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentInfo {
    pub name: String,
    pub file_count: usize,
    pub total_size: u64,
    pub metadata_size: u64,
}

/// Decide if a torrent_root looks like a fresh install - i.e. either missing
/// or contains nothing that librqbit needs to validate against. We use this
/// to gate the empty-bitfield pre-seed: if the user pointed Exodium at a
/// directory full of existing game data, we must let librqbit do its real
/// validation pass rather than tell it "everything is missing" (which would
/// trigger a re-download of complete files).
///
/// Dotfiles (e.g. `.eXoDOS_configs_extracted` markers we drop into the
/// torrent_root after extracting bundled DOSBox configs) are NOT torrent
/// content and must be ignored - otherwise the second app launch would see
/// our own markers and refuse to seed, defeating the optimisation for users
/// who haven't actually downloaded any games yet.
fn torrent_root_looks_empty(torrent_root: &Path) -> bool {
    let iter = match std::fs::read_dir(torrent_root) {
        Err(_) => return true, // doesn't exist or unreadable → safe to seed
        Ok(it) => it,
    };
    for entry in iter.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if !s.starts_with('.') {
            return false;
        }
    }
    true
}

/// Pre-seed `<persistence_dir>/<info_hash>.bitv` files with `ceil(piece_count/8)`
/// zero bytes for each enabled collection where (a) the .bitv doesn't already
/// exist and (b) the data directory looks fresh.
///
/// librqbit's `JsonSessionPersistenceStore::BitVFactory::load(info_hash)` reads
/// this file directly by info_hash - it does not require an entry in
/// `session.json`. With a present, all-zero bitfield, `validate_fastresume`
/// accepts "I have 0 pieces" without iterating any pieces, and the slow
/// `initial_check` pass is skipped entirely. Net effect: first download on a
/// fresh install starts in seconds instead of 5–10 minutes on Windows.
fn seed_fastresume_bitvs(
    persistence_dir: &Path,
    collections: &[&str],
    data_path: &Path,
) {
    if let Err(e) = std::fs::create_dir_all(persistence_dir) {
        log::warn!("fastresume: could not create {}: {}", persistence_dir.display(), e);
        return;
    }
    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        let torrent_path = match bundled_torrent_path(col.torrent_file) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Parse torrent for info_hash + piece_count. We use lava_torrent
        // directly here rather than TorrentIndex because TorrentIndex doesn't
        // expose piece_count.
        let torrent = match lava_torrent::torrent::v1::Torrent::read_from_file(&torrent_path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("fastresume: failed to parse {}: {}", col.torrent_file, e);
                continue;
            }
        };
        let info_hash = torrent.info_hash();
        let bitv_path = persistence_dir.join(format!("{}.bitv", info_hash));

        if bitv_path.exists() {
            log::debug!("fastresume: existing bitv for {} ({})", col.id, info_hash);
            continue;
        }

        // One root for every collection. Only seed if it looks empty -
        // otherwise librqbit must verify what's actually on disk.
        let torrent_root = game_root(&data_path.to_string_lossy());
        if !torrent_root_looks_empty(&torrent_root) {
            log::info!(
                "fastresume: skipping seed for {} - torrent_root {} non-empty, real validation needed",
                col.id,
                torrent_root.display()
            );
            continue;
        }

        let piece_count = torrent.pieces.len();
        let byte_count = piece_count.div_ceil(8);
        match std::fs::write(&bitv_path, vec![0u8; byte_count]) {
            Ok(_) => log::info!(
                "fastresume: seeded empty bitv for {} ({} pieces, {} bytes)",
                col.id, piece_count, byte_count
            ),
            Err(e) => log::warn!(
                "fastresume: failed to write {}: {}",
                bitv_path.display(), e
            ),
        }
    }
}

/// Initialize download managers for all available torrents.
/// Returns true if initialized, false if no config found.
#[tauri::command]
pub async fn init_download_manager(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<bool, String> {
    // Clear existing managers
    torrent_state.0.write().await.clear();

    let data_dir = {
        let conn = db_state.lock()?;
        // Every path below (and in launch/uninstall/scan) is derived from the
        // root, so it has to be in the cache before anything asks.
        load_root_folder(&conn);
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    };

    let data_dir = match data_dir {
        Some(d) => d,
        None => return Ok(false),
    };

    let data_path = PathBuf::from(&data_dir);

    // Get selected collections from config
    let collections_str = {
        let conn = db_state.lock()?;
        queries::get_config(&conn, "collections")
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "eXoDOS".to_string())
    };
    let collections: Vec<&str> = collections_str.split(',').collect();

    let metadata_dir = bundled_metadata_dir().ok();

    super::assets::prune_gallery_cache_at_startup(&data_dir);
    // Earlier versions wrote per-launch DOSBox fragments straight into the
    // user's game folder; they are regenerated on demand, so sweep them up.
    crate::commands::paths::sweep_legacy_launch_confs(&data_dir);
    crate::commands::paths::prune_launch_confs(&app);
    crate::commands::media::prune_video_cache(&data_dir);
    crate::commands::media::prune_music_cache(&data_dir);

    // Offline mode: no session, no torrents, no swarm traffic - the app is a
    // launcher for whatever is already on disk. Bundled emulator configs are
    // still extracted; they are shipped with Exodium, not with the torrent.
    // Managers were cleared above, so a live -> offline switch at runtime
    // drops the last Arc to the session and shuts librqbit down.
    if is_offline(&db_state.0) {
        extract_all_bundled_configs(&collections, metadata_dir.as_ref(), &data_path);
        log::info!("Offline mode: torrent engine not started (data_dir: {})", data_dir);
        return Ok(false);
    }

    // All collections share one librqbit session and the same data directory.
    // Session state (.librqbit/) is stored in the app config dir, not the game data dir.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let persistence_dir = fastresume_dir(&config_dir);

    // Plant empty bitfields for fresh-install torrents BEFORE creating the
    // session so librqbit's `BitVFactory::load(info_hash)` finds them and
    // skips the initial_check pass. Existing data triggers real validation.
    seed_fastresume_bitvs(&persistence_dir, &collections, &data_path);

    let session = DownloadManager::create_session(&config_dir, &persistence_dir)
        .await
        .map_err(|e| e.to_string())?;
    evict_mismatched_session_torrents(&session, &persistence_dir, &data_path).await;
    apply_transfer_preferences(&session, &db_state);

    // Build all managers and do slow work (infohash, config extraction) WITHOUT holding
    // the torrent_state write lock - archive.extract() on 7 000+ files blocks for seconds.
    let mut new_managers: Vec<(String, Arc<DownloadManager>)> = Vec::new();

    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        if let Ok(torrent_path) = bundled_torrent_path(col.torrent_file) {
            match DownloadManager::new_with_session(Arc::clone(&session), &torrent_path, &data_path, &persistence_dir) {
                Ok(mgr) => {
                    // Record which torrent this install was built against.
                    // Nothing reads it today - the catalogue update check was
                    // removed until there is a migration that keeps a user's
                    // library (issue #18) - but it has to be captured NOW: once
                    // a newer bundled torrent ships, the old baseline is gone
                    // and the comparison can never be made retroactively.
                    // Write-if-absent for the same reason.
                    match TorrentIndex::infohash(&torrent_path) {
                        Ok(hash) => {
                            match db_state.0.lock() {
                                Ok(conn) => {
                                    let key = format!("{}_infohash", col.id);
                                    let existing = queries::get_config(&conn, &key).ok().flatten();
                                    if existing.is_none() {
                                        if let Err(e) = queries::set_config(&conn, &key, &hash) {
                                            log::warn!("Failed to save infohash for {}: {}", col.id, e);
                                        }
                                    }
                                }
                                Err(e) => log::warn!("Failed to lock DB for infohash write ({}): {}", col.id, e),
                            }
                        }
                        Err(e) => log::warn!("Failed to compute infohash for {}: {}", col.id, e),
                    }

                    extract_bundled_configs(col, metadata_dir.as_ref(), &mgr.torrent_root());
                    log::info!("Initialized download manager: {}", col.id);
                    new_managers.push((col.id.to_string(), Arc::new(mgr)));
                }
                Err(e) => {
                    log::warn!("Failed to init {} download manager: {}", col.id, e);
                }
            }
        }
    }

    // All enabled torrents overlay into the same root - placeholder cleanup
    // in any one manager must keep the union of every torrent's file list.
    set_union_cleanup_keep_paths(&new_managers);

    // Adopt torrents the session auto-resumed from persistence, so downloads
    // interrupted by an app restart report progress and finish extraction.
    for (id, mgr) in &new_managers {
        if mgr.hydrate_from_session().await {
            log::info!("{}: adopted persisted torrent from session", id);
        }
    }

    // A util zip in flight when the app last quit lost its watcher with it.
    for pack in crate::support_files::PACKS {
        if let Some((_, mgr)) = new_managers.iter().find(|(id, _)| id == pack.collection) {
            crate::support_files::rearm(pack, mgr).await;
        }
    }

    // Acquire write lock only for the insert - no blocking work inside.
    let count = new_managers.len();
    {
        let mut managers = torrent_state.0.write().await;
        for (id, mgr) in new_managers {
            managers.insert(id, mgr);
        }
    }

    log::info!("Download managers initialized: {} (data_dir: {})", count, data_dir);
    Ok(count > 0)
}

/// Extract a collection's bundled emulator-config ZIP into the torrent root,
/// once (a lock file marks success). Called for every enabled collection in
/// BOTH network modes - an offline install still needs the DOSBox configs
/// that would otherwise arrive with the torrent.
fn extract_bundled_configs(col: &CollectionDef, metadata_dir: Option<&PathBuf>, torrent_root: &Path) {
    let (Some(cfg_zip), Some(md)) = (col.configs_zip, metadata_dir) else {
        return;
    };
    let cfg_path = md.join(cfg_zip);
    if !cfg_path.exists() {
        return;
    }
    let lock = torrent_root.join(format!(".{}_configs_extracted", col.id));
    if lock.exists() {
        return;
    }
    log::info!("Extracting {} configs to {}", col.id, torrent_root.display());
    // Write the lock ONLY on success - latching a failed extract (disk full,
    // permissions) left the configs permanently missing with no retry.
    let extracted = std::fs::File::open(&cfg_path)
        .map_err(|e| e.to_string())
        .and_then(|f| zip::ZipArchive::new(f).map_err(|e| e.to_string()))
        .and_then(|mut a| a.extract(torrent_root).map_err(|e| e.to_string()));
    match extracted {
        Ok(()) => {
            if let Err(e) = std::fs::write(&lock, "") {
                log::warn!("Could not write configs lock for {}: {}", col.id, e);
            }
        }
        Err(e) => log::error!(
            "Failed to extract {} configs (will retry next startup): {}",
            col.id, e
        ),
    }
}

/// Apply the persisted seeding preference to a freshly created session.
/// Push the stored transfer preferences into a freshly created session.
///
/// Sharing is OPT-IN: only an explicit "1" lifts the upload cap, anything else
/// (including an unset key) caps upload at 1 KB/s. Uploading copyrighted
/// material is a legal risk in some jurisdictions, so it must never start
/// without consent. The user's own caps ride along, because a new session
/// starts unlimited in both directions and would otherwise ignore them until
/// the next time Settings was touched.
fn apply_transfer_preferences(session: &Arc<librqbit::Session>, db_state: &State<'_, DbState>) {
    let seeding = seeding_enabled(&db_state.0);
    let (up_kbps, down_kbps) = rate_limits(&db_state.0);
    crate::torrent::manager::apply_session_limits(session, seeding, up_kbps, down_kbps);
}

/// Drop session torrents whose persisted output folder is not the current game
/// root. Adopting them via hydrate_from_session would report progress against
/// files librqbit writes to the OLD location - extraction probes the new one
/// and loops on "100% but ZIP missing" forever.
///
/// The test is EXACT, not "somewhere under the data dir": every torrent is
/// added with `output_folder = game_root`, so any other value is stale by
/// definition. The looser check let the pre-single-root layout survive - a
/// torrent persisted with `<data>/eXoWin9x` sat inside the data dir, passed,
/// and librqbit re-created its whole file tree there seconds after the layout
/// migration had merged it away.
async fn evict_mismatched_session_torrents(
    session: &Arc<librqbit::Session>,
    persistence_dir: &Path,
    data_path: &Path,
) {
    let session_json = persistence_dir.join("session.json");
    let content = match std::fs::read_to_string(&session_json) {
        Ok(c) => c,
        Err(_) => return, // no persisted session yet
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Could not parse {}: {}", session_json.display(), e);
            return;
        }
    };
    let Some(torrents) = parsed.get("torrents").and_then(|t| t.as_object()) else {
        return;
    };

    // Normalize for comparison: strip Windows \\?\ long-path prefix, unify
    // slashes (output folders are written with to_long_path on Windows),
    // trim trailing separators, and case-fold on case-insensitive platforms
    // (C:\Games vs c:\games must not read as a mismatch).
    let normalize = |p: &str| {
        let s = p
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        if cfg!(any(windows, target_os = "macos")) {
            s.to_ascii_lowercase()
        } else {
            s
        }
    };
    let root_norm = normalize(&game_root(&data_path.to_string_lossy()).to_string_lossy());

    for entry in torrents.values() {
        let (Some(hash), Some(folder)) = (
            entry.get("info_hash").and_then(|h| h.as_str()),
            entry.get("output_folder").and_then(|f| f.as_str()),
        ) else {
            continue;
        };
        if normalize(folder) == root_norm {
            continue;
        }
        let stale_id = session.with_torrents(|iter| {
            for (tid, t) in iter {
                if t.info_hash().as_string().eq_ignore_ascii_case(hash) {
                    return Some(tid);
                }
            }
            None
        });
        if let Some(tid) = stale_id {
            log::warn!(
                "Evicting session torrent {} - persisted output folder {} is not the game root {}",
                hash, folder, root_norm
            );
            if let Err(e) = session
                .delete(librqbit::api::TorrentIdOrHash::Id(tid), false)
                .await
            {
                log::warn!("Failed to evict stale session torrent {}: {}", hash, e);
            }
        }
    }
}

/// Give every manager the union of all managers' torrent file lists as its
/// placeholder-cleanup keep-list. See DownloadManager::cleanup_keep_paths.
fn set_union_cleanup_keep_paths(managers: &[(String, Arc<DownloadManager>)]) {
    let union: Arc<Vec<String>> = Arc::new(
        managers
            .iter()
            .flat_map(|(_, m)| m.index().files.iter().map(|f| f.path.clone()))
            .collect(),
    );
    for (_, mgr) in managers {
        mgr.set_cleanup_keep_paths(Arc::clone(&union));
    }
}

/// Reset all data: clear DB, remove config. Returns to setup state.
#[tauri::command]
pub async fn factory_reset(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    delete_game_data: bool,
) -> Result<(), String> {
    log::info!("factory_reset called (delete_game_data={})", delete_game_data);
    // Read data_dir before clearing config (Mutex must not be held across await)
    let data_dir = if delete_game_data {
        let conn = db_state.lock()?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    } else {
        None
    };

    // Drop all download managers. Use a timeout so a stuck reader doesn't hang forever.
    let session_mgr = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        torrent_state.0.write(),
    ).await {
        Ok(mut managers) => {
            // Keep one Arc to reach the shared session after clearing the map.
            let any = managers.values().next().cloned();
            managers.clear();
            any
        }
        Err(_) => {
            log::error!("factory_reset: timed out waiting for torrent write lock");
            return Err("Could not stop downloads in time. Cancel any active downloads and try again.".to_string());
        }
    };

    // Quiesce librqbit BEFORE deleting files: spawned tasks hold Arcs that
    // keep the session's writers alive past the map clear, and a live writer
    // can re-create files after the wipe - leaving a piece ledger that
    // claims data which no longer exists ("100% but ZIP missing" on the
    // next setup).
    if let Some(mgr) = session_mgr {
        mgr.shutdown_session().await;
    }

    // Reset user state without touching the game catalog.
    // Games are catalog data (from the bundled DB) - clearing them would leave
    // the library empty until next restart. Only reset per-user flags and config.
    {
        let conn = db_state.lock()?;
        conn.execute_batch(
            "UPDATE games SET in_library = 0, installed = 0, favorited = 0, last_played = NULL;
             DELETE FROM game_config;
             DELETE FROM downloads;
             DELETE FROM images;
             -- Curated playlists are catalog data like the games rows above:
             -- deleting them here would leave the Playlists dropdown empty
             -- until the next launch re-runs the catalog refresh. Only user
             -- playlists are user state (their memberships cascade).
             DELETE FROM playlists WHERE kind = 'user';
             DELETE FROM config;",
        )
        .map_err(|e| e.to_string())?;
    }

    // Optionally delete the game folder + content packs + stale downloads.
    if let Some(dir) = data_dir {
        if !dir.is_empty() {
            let base = std::path::Path::new(&dir);
            let root = game_root(&dir);
            // Normally the root is a folder INSIDE the data dir, so it can go
            // whole. On a legacy install the two are the same directory - the
            // one the user picked - and removing it would take the folder
            // itself with it. Delete the two trees eXo puts there instead.
            let targets: Vec<PathBuf> = if root == base {
                vec![base.join("eXo"), base.join("Content")]
            } else {
                vec![root]
            };
            for target in targets {
                if !target.exists() {
                    continue;
                }
                log::info!("Deleting game data: {}", target.display());
                if let Err(e) = std::fs::remove_dir_all(&target) {
                    log::error!("Failed to delete game data folder: {}", e);
                    return Err(format!("Failed to delete game data: {}", e));
                }
            }
            // Also remove downloaded content packs and staging artifacts.
            let content_path = base.join("content");
            if content_path.exists() {
                log::info!("Deleting content packs: {}", content_path.display());
                let _ = std::fs::remove_dir_all(&content_path);
            }
            let downloads_path = base.join(".content-downloads");
            if downloads_path.exists() {
                let _ = std::fs::remove_dir_all(&downloads_path);
            }
            // Legacy residue: pre-0.8.4 setup sessions persisted fastresume
            // in the DATA dir (session split-brain bug). Clean it up here so
            // a later setup against the same dir can't load a stale ledger.
            let legacy_fastresume = base.join("librqbit-fastresume");
            if legacy_fastresume.exists() {
                let _ = std::fs::remove_dir_all(&legacy_fastresume);
            }
        }
    }

    // Fastresume cache: clear it ONLY when the data was deleted too. The
    // ledger describes the torrent bytes on disk - after a delete it's stale
    // and would mislead librqbit, but in keep-data mode it's still accurate,
    // and clearing it forced the next download into a full multi-minute
    // revalidation of ~14k files (the exact hang the seeding avoids).
    if delete_game_data {
        match app.path().app_data_dir() {
            Ok(config_dir) => {
                let persistence = fastresume_dir(&config_dir);
                if persistence.exists() {
                    log::info!("Clearing fastresume cache: {}", persistence.display());
                    if let Err(e) = std::fs::remove_dir_all(&persistence) {
                        log::warn!("Failed to clear fastresume cache: {}", e);
                    }
                }
            }
            Err(e) => log::warn!(
                "factory_reset: could not resolve app_data_dir to clear fastresume cache: {}", e
            ),
        }
    }

    log::info!("Factory reset completed (delete_game_data={})", delete_game_data);
    Ok(())
}


/// Return the default parent directory for game storage ($HOME).
/// The eXoDOS folder will be created inside this directory by the torrent engine.
#[tauri::command]
pub async fn get_default_data_dir() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "Cannot determine home directory".to_string())?;
    Ok(home)
}

/// Whether a directory holds nothing Exodium would recognise as game data.
///
/// "Change game folder" POINTS Exodium at a folder, it does not move anything
/// into one - so an empty target is the signature of the misunderstanding: the
/// user meant to relocate their library and is about to end up with an empty
/// view and their games still on the old disk. OS metadata does not count as
/// content; a Finder visit is not an install.
#[tauri::command]
pub async fn data_dir_is_empty(path: String) -> Result<bool, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Ok(true);
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    Ok(!entries
        .flatten()
        .any(|entry| !is_os_metadata(&entry.path())))
}


#[tauri::command]
pub async fn get_torrent_info() -> Result<TorrentInfo, String> {
    let torrent_path = bundled_torrent_path("eXoDOS.torrent")?;
    let index =
        TorrentIndex::from_file(&torrent_path).map_err(|e| format!("Failed to parse torrent: {}", e))?;

    let metadata_size = index
        .find_metadata_zip()
        .map(|f| f.size)
        .unwrap_or(0);

    Ok(TorrentInfo {
        name: index.name.clone(),
        file_count: index.files.len(),
        total_size: index.total_size,
        metadata_size,
    })
}

/// Initialize the download system and start downloading metadata.
#[tauri::command]
pub async fn setup_start(
    app: tauri::AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    data_dir: String,
) -> Result<String, String> {
    use tauri::Manager;

    // Save data_dir to config. A fresh install keeps the historical root
    // name, so nothing existing has to move.
    {
        let conn = db_state.lock()?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
        queries::set_config(&conn, "root_folder", DEFAULT_ROOT_FOLDER).map_err(|e| e.to_string())?;
    }
    set_root_folder(DEFAULT_ROOT_FOLDER);

    let torrent_path = bundled_torrent_path("eXoDOS.torrent")?;
    let data_path = PathBuf::from(&data_dir);

    // Same session root as init_download_manager (app config dir, NOT the
    // data dir). The old data-dir session split the piece ledger from the
    // main flow: fastresume earned during setup was invisible afterwards
    // (forcing a full ~14k-file revalidation on the first game download) and
    // its data-dir persistence was cleaned by neither factory_reset nor
    // eviction, leaving stale ledgers behind.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let persistence_dir = fastresume_dir(&config_dir);
    seed_fastresume_bitvs(&persistence_dir, &["eXoDOS"], &data_path);
    let session = DownloadManager::create_session(&config_dir, &persistence_dir)
        .await
        .map_err(|e| e.to_string())?;
    evict_mismatched_session_torrents(&session, &persistence_dir, &data_path).await;
    apply_transfer_preferences(&session, &db_state);
    let manager =
        DownloadManager::new_with_session(session, &torrent_path, &data_path, &persistence_dir)
            .map_err(|e| format!("Failed to init download manager: {}", e))?;

    // Find metadata files in the torrent
    let metadata_idx = manager
        .index()
        .find_metadata_zip()
        .map(|f| f.index)
        .ok_or("XODOSMetadata.zip not found in torrent")?;

    let dosbox_idx = manager
        .index()
        .find_dosbox_metadata_zip()
        .map(|f| f.index);

    // Queue metadata files for download
    let mut files_to_download = vec![metadata_idx];
    if let Some(idx) = dosbox_idx {
        files_to_download.push(idx);
    }

    manager
        .download_files(files_to_download)
        .await
        .map_err(|e| format!("Failed to start metadata download: {}", e))?;

    let manager = Arc::new(manager);
    torrent_state.0.write().await.insert("eXoDOS".to_string(), manager);

    Ok("Metadata download started".to_string())
}

/// Poll setup progress (metadata download + import status).
#[tauri::command]
pub async fn get_setup_status(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<SetupStatus, String> {
    // Clone the Arc so we can drop the read guard before any .await points.
    // Holding the guard across awaits blocks factory_reset's write lock indefinitely.
    let manager_arc = {
        let guard = torrent_state.0.read().await;
        guard.get("eXoDOS").cloned()
    };

    let manager = match manager_arc {
        Some(ref m) => m,
        None => {
            // Ready if data_dir is configured AND the game DB has content
            let (has_data_dir, count) = {
                let conn = db_state.lock()?;
                load_root_folder(&conn);
                let dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
                let count = queries::count_games(&conn, "").map_err(|e| e.to_string())?;
                (dir.is_some(), count)
            };
            let ready = has_data_dir && count > 0;
            return Ok(SetupStatus {
                phase: if ready {
                    "ready".to_string()
                } else {
                    "not_started".to_string()
                },
                metadata_progress: None,
                dosbox_metadata_progress: None,
                games_imported: count,
                ready,
            });
        }
    };

    let metadata_idx = manager.index().find_metadata_zip().map(|f| f.index);
    let dosbox_idx = manager.index().find_dosbox_metadata_zip().map(|f| f.index);

    let metadata_progress = if let Some(idx) = metadata_idx {
        manager.file_progress(idx).await
    } else {
        None
    };

    let dosbox_progress = if let Some(idx) = dosbox_idx {
        manager.file_progress(idx).await
    } else {
        None
    };

    let metadata_done = metadata_progress
        .as_ref()
        .map(|p| p.finished)
        .unwrap_or(false);

    // Check if games are already imported
    let games_imported = {
        let conn = db_state.lock()?;
        queries::count_games(&conn, "").map_err(|e| e.to_string())?
    };

    let phase = if games_imported > 0 {
        "ready"
    } else if metadata_done {
        "metadata_ready"
    } else if metadata_progress.is_some() {
        "downloading_metadata"
    } else {
        "starting"
    };

    Ok(SetupStatus {
        phase: phase.to_string(),
        metadata_progress,
        dosbox_metadata_progress: dosbox_progress,
        games_imported,
        ready: games_imported > 0,
    })
}

/// After metadata ZIP is downloaded, extract and import games.
#[tauri::command]
pub async fn setup_import(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<usize, String> {
    // Clone the Arc and drop the read guard immediately (same pattern as
    // get_setup_status) - holding it across the multi-second import blocks
    // factory_reset's write lock into its 5s timeout.
    let manager = {
        let guard = torrent_state.0.read().await;
        guard
            .get("eXoDOS")
            .cloned()
            .ok_or("Download manager not initialized")?
    };

    // Find the downloaded metadata ZIP path
    let metadata_idx = manager
        .index()
        .find_metadata_zip()
        .map(|f| f.index)
        .ok_or("Metadata ZIP not found in torrent")?;

    if !manager.is_file_complete(metadata_idx).await {
        return Err("Metadata ZIP is still downloading".to_string());
    }

    let zip_path = manager
        .file_output_path(metadata_idx)
        .ok_or("Cannot determine metadata ZIP path")?;

    if !zip_path.exists() {
        return Err(format!("Metadata ZIP not found at: {}", zip_path.display()));
    }

    // Get DB path for a separate connection
    let db_path = {
        let conn = db_state.lock()?;
        conn.path()
            .map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    // Clone what we need for the blocking task
    let torrent_index = manager.index().clone();
    let zip = zip_path.clone();

    // Also extract !DOSmetadata.zip (DOSBox configs) if downloaded
    let dosbox_zip_path = manager
        .index()
        .find_dosbox_metadata_zip()
        .and_then(|f| manager.file_output_path(f.index))
        .filter(|p| p.exists());

    let torrent_root = manager.torrent_root();

    let count = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        db::init(&conn).map_err(|e| e.to_string())?;

        // Clear existing games to prevent duplicates on re-import
        queries::clear_games(&conn).map_err(|e| e.to_string())?;

        let count =
            import::import_from_zip(&zip, &conn, "!dos").map_err(|e| e.to_string())?;

        // Extract !DOSmetadata.zip to torrent root so eXo/eXoDOS/!dos/ is available
        if let Some(dosbox_zip) = dosbox_zip_path {
            log::info!("Extracting DOSBox configs to {}", torrent_root.display());
            let file = std::fs::File::open(&dosbox_zip).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            archive.extract(&torrent_root).map_err(|e| e.to_string())?;
        }

        match_torrent_indices(&conn, &torrent_index, "eXoDOS").map_err(|e| e.to_string())?;

        Ok::<usize, String>(count)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(count)
}


/// Import from an existing eXoDOS directory on disk (skips metadata download).
/// The user selects the eXoDOS folder itself; the parent is stored as data_dir
/// so that new downloads land correctly inside the existing eXoDOS tree.
#[tauri::command]
pub async fn setup_from_local(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    exodos_path: String,
) -> Result<usize, String> {
    let root = PathBuf::from(&exodos_path);

    // The data_dir is the parent of the selected eXoDOS folder.
    // librqbit will write new downloads to <data_dir>/eXoDOS/ which equals the selected path.
    let data_dir = root
        .parent()
        .ok_or("Selected path has no parent directory")?
        .to_string_lossy()
        .to_string();

    // The imported folder IS the root - whatever the user called it. eXo's
    // setup does not dictate a name, and assuming "eXoDOS" made every path
    // miss for anyone who had named it differently or merged their packs.
    let root_folder = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| DEFAULT_ROOT_FOLDER.to_string());

    // Save data_dir, the root folder and all collections to config
    {
        let conn = db_state.lock()?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
        queries::set_config(&conn, "root_folder", &root_folder).map_err(|e| e.to_string())?;
        // Remembered, not just passed to this one scan: every later startup
        // scan has to know that on THIS install the archives on disk are the
        // library, or it treats them as somebody else's download debris.
        queries::set_config(&conn, "library_from_disk", "1").map_err(|e| e.to_string())?;
        let all_collections = COLLECTION_MAP.iter().map(|c| c.id).collect::<Vec<_>>().join(",");
        queries::set_config(&conn, "collections", &all_collections).map_err(|e| e.to_string())?;
    }
    set_root_folder(&root_folder);
    crate::allow_asset_dir(&app, &PathBuf::from(&data_dir));

    // The bundled DB already has the full game catalog - no need to re-parse the
    // eXoDOS XML (XODOSMetadata.zip is 5 GB and would block for minutes).
    // Just report how many games are in the current DB.
    let count = {
        let conn = db_state.lock()?;
        conn.query_row("SELECT COUNT(*) FROM games", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    };

    // Init download managers for all collections (for future game downloads).
    // Session state goes in the app config dir, game files in the existing eXoDOS tree.
    // Build managers WITHOUT holding the write lock - create_session is async and can be slow.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let data_path = PathBuf::from(&data_dir);
    let persistence_dir = fastresume_dir(&config_dir);

    // Same fastresume seed as init_download_manager - see notes there. This
    // path is hit by `setup_from_local`, where the data_dir often already
    // contains pre-extracted games, so the seed is a no-op except for
    // truly empty cases. That's the correct behavior.
    let collection_ids: Vec<&str> = COLLECTION_MAP.iter().map(|c| c.id).collect();
    seed_fastresume_bitvs(&persistence_dir, &collection_ids, &data_path);

    // The torrent file lists are needed in BOTH network modes: the LP backfill
    // below wires game_torrent_index from them, and an offline user who later
    // switches to live must find those indices already populated.
    let mut torrent_indices: Vec<(String, TorrentIndex)> = Vec::new();
    for col in COLLECTION_MAP {
        if let Ok(col_torrent_path) = bundled_torrent_path(col.torrent_file) {
            match TorrentIndex::from_file(&col_torrent_path) {
                Ok(idx) => torrent_indices.push((col.id.to_string(), idx)),
                Err(e) => log::warn!("Failed to parse {} torrent: {}", col.id, e),
            }
        }
    }

    let offline = is_offline(&db_state.0);
    let mut new_managers = Vec::new();
    if offline {
        log::info!("Offline mode: importing local collection without starting the torrent engine");
    } else {
        let session = DownloadManager::create_session(&config_dir, &persistence_dir)
            .await
            .map_err(|e| format!("Failed to init session: {}", e))?;
        evict_mismatched_session_torrents(&session, &persistence_dir, &data_path).await;
        apply_transfer_preferences(&session, &db_state);

        for col in COLLECTION_MAP {
            if let Ok(col_torrent_path) = bundled_torrent_path(col.torrent_file) {
                match DownloadManager::new_with_session(Arc::clone(&session), &col_torrent_path, &data_path, &persistence_dir) {
                    Ok(mgr) => new_managers.push((col.id.to_string(), Arc::new(mgr))),
                    Err(e) => log::warn!("Failed to init {} download manager: {}", col.id, e),
                }
            }
        }
        set_union_cleanup_keep_paths(&new_managers);
        for (id, mgr) in &new_managers {
            if mgr.hydrate_from_session().await {
                log::info!("{}: adopted persisted torrent from session", id);
            }
        }
    }

    // Backfill any LP collections that are absent from the DB.
    // This happens when the DB was originally built from XODOSMetadata.zip (EN only) by the
    // old setup path.  The bundled .xml.gz files always include all LP catalogs, so we import
    // whichever collections are missing and then run match_torrent_indices to wire up
    // torrent_source / game_torrent_index.
    if let Ok(metadata_dir) = bundled_metadata_dir() {
        for (col_id, torrent_index) in &torrent_indices {
            let col = match collection_def(col_id) {
                Some(c) => c,
                None => continue,
            };
            if col.lang_dir.is_none() {
                continue; // base eXoDOS - skip
            }

            let already_in_db: i64 = {
                let conn = db_state.lock()?;
                conn.query_row(
                    "SELECT COUNT(*) FROM games WHERE torrent_source = ?1",
                    rusqlite::params![col_id],
                    |r| r.get(0),
                )
                .unwrap_or(0)
            };
            if already_in_db > 0 {
                continue;
            }

            let xml_gz = metadata_dir.join(col.metadata_file);
            if !xml_gz.exists() {
                log::warn!("Bundled metadata not found for {}: {}", col_id, xml_gz.display());
                continue;
            }

            let imported = {
                let conn = db_state.lock()?;
                import::import_from_gz(&xml_gz, &conn, col.shortcode_segment)
                    .unwrap_or_else(|e| {
                        log::warn!("Failed to import {} XML: {}", col_id, e);
                        0
                    })
            };
            log::info!("Backfilled {} {} games from bundled XML", imported, col_id);

            if imported > 0 {
                // Wire up game_torrent_index and torrent_source for the newly imported rows.
                let col_id_owned = col_id.clone();
                let conn = db_state.lock()?;
                if let Err(e) = match_torrent_indices(&conn, torrent_index, &col_id_owned) {
                    log::warn!("match_torrent_indices failed for {}: {}", col_id_owned, e);
                }
            }
        }

        // Populate thumbnail_key for every game whose row got its hash wiped
        // by `import_bundled_metadata`'s clear_games() + XML re-import above.
        // Without this the library would show no covers after first-run setup
        // even though the bundled preview pack has everything. Shared helper
        // in db::populate_thumbnail_keys uses the same hash function as
        // gen_thumbnails.py and generate_db.rs.
        {
            let conn = db_state.lock()?;
            db::populate_thumbnail_keys(&conn).map_err(|e| e.to_string())?;
        }

        // Backfill shortcodes, dosbox_conf and has_thumbnail for LP games that lack them.
        // PLP and SLP XMLs use a path format without the "!dos/<shortcode>" segment, so their
        // shortcodes (and derived dosbox_conf) come out as NULL after import.  Mirror the same
        // two-step approach as generate_db.rs: exact EN title match first.
        // This is idempotent - rows already having values are unaffected.
        let conn = db_state.lock()?;
        // Every LP↔EN match is family-scoped (CLAUDE.md §1): titles and
        // shortcodes repeat across pack families, and an unscoped match hands
        // an LP row another family's shortcode - which orphans it from its
        // variant group. There is deliberately NO thumbnail_key copy here:
        // db::propagate_lp_thumbnail_keys below owns that rule, and a second
        // copy of it drifted from the first once already.
        let same = db::queries::same_group("en", "games");
        let fam_en = db::queries::family_expr("en");
        let fam_g = db::queries::family_expr("games");
        let _ = conn.execute_batch(&format!(
            "-- Inherit shortcode from the matching EN game by exact title
             UPDATE games
             SET shortcode = (
                 SELECT en.shortcode FROM games en
                 WHERE en.language = 'EN'
                   AND en.shortcode IS NOT NULL
                   AND en.title = games.title
                   AND {fam_en} = {fam_g}
                 LIMIT 1
             )
             WHERE shortcode IS NULL;

             -- Second pass: normalized title match (handles punctuation differences like
             -- 'Foo - Bar' vs 'Foo: Bar').  Only touches LP rows still without a shortcode.
             UPDATE games
             SET shortcode = (
                 SELECT en.shortcode FROM games en
                 WHERE en.language = 'EN'
                   AND en.shortcode IS NOT NULL
                   AND {fam_en} = {fam_g}
                   AND LOWER(TRIM(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(
                         en.title, ':', ' '), '-', ' '), ',', ''), '.', ''), '  ', ' ')))
                     = LOWER(TRIM(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(
                         games.title, ':', ' '), '-', ' '), ',', ''), '.', ''), '  ', ' ')))
                 LIMIT 1
             )
             WHERE shortcode IS NULL AND language != 'EN';

             -- LP games use the EN DOSBox config; backfill dosbox_conf from the EN variant
             UPDATE games
             SET dosbox_conf = (
                 SELECT en.dosbox_conf FROM games en
                 WHERE {same}
                   AND en.language = 'EN'
                   AND en.dosbox_conf IS NOT NULL
                 LIMIT 1
             )
             WHERE dosbox_conf IS NULL AND shortcode IS NOT NULL;

             -- Propagate has_thumbnail flag from EN variant (same group = same cover art)
             UPDATE games
             SET has_thumbnail = 1
             WHERE has_thumbnail = 0
               AND EXISTS (
                   SELECT 1 FROM games en
                   WHERE en.language = 'EN' AND en.has_thumbnail = 1 AND {same}
               );",
        ));
        log::info!("LP shortcode/dosbox_conf/has_thumbnail/thumbnail_key backfill complete");

        // Pass 3: pull shortcodes for LP-exclusive games from the bundled static DB.
        // metadata/exodium.db (built by generate_db.rs) contains 100% shortcode coverage
        // including LP-exclusive games with no EN equivalent (via generate_shortcode()).
        // Passes 1 & 2 only matched titles present in the EN catalog; this covers the rest.
        //
        // ATTACH and DETACH are issued as separate calls so DETACH always runs even when an
        // UPDATE fails - execute_batch stops at the first error, which would leave lp_static
        // attached for the lifetime of the connection if DETACH were part of the same batch.
        let static_db = metadata_dir.join("exodium.db");
        if static_db.exists() {
            let path_esc = static_db.to_string_lossy().replace('\'', "''");
            let attach_ok = conn
                .execute_batch(&format!("ATTACH DATABASE '{path}' AS lp_static;", path = path_esc))
                .is_ok();
            if attach_ok {
                // Title matches against the static DB are family-scoped too -
                // it catalogues every pack, and eXoWin9x shares titles with
                // the LP rows this pass is meant to serve.
                let fam_s = db::queries::family_expr("s");
                let result = conn.execute_batch(&format!(
                    "UPDATE games
                     SET shortcode = (
                         SELECT s.shortcode FROM lp_static.games s
                         WHERE s.title = games.title AND s.shortcode IS NOT NULL
                           AND {fam_s} = {fam_g}
                         LIMIT 1
                     )
                     WHERE shortcode IS NULL AND language != 'EN';
                     UPDATE games
                     SET has_thumbnail = COALESCE((
                         SELECT s.has_thumbnail FROM lp_static.games s
                         WHERE s.title = games.title AND {fam_s} = {fam_g}
                         LIMIT 1
                     ), has_thumbnail)
                     WHERE language != 'EN' AND shortcode IS NOT NULL;
                     UPDATE games
                     SET thumbnail_key = COALESCE((
                         SELECT s.thumbnail_key FROM lp_static.games s
                         WHERE s.title = games.title AND s.thumbnail_key IS NOT NULL
                           AND {fam_s} = {fam_g}
                         LIMIT 1
                     ), thumbnail_key)
                     WHERE thumbnail_key IS NULL AND language != 'EN';
                     UPDATE games
                     SET dosbox_conf = (
                         SELECT en.dosbox_conf FROM games en
                         WHERE {same}
                           AND en.language = 'EN'
                           AND en.dosbox_conf IS NOT NULL
                         LIMIT 1
                     )
                     WHERE dosbox_conf IS NULL AND shortcode IS NOT NULL;",
                ));
                let _ = conn.execute_batch("DETACH DATABASE lp_static;");
                match result {
                    Ok(_) => log::info!("Pass 3: LP-exclusive shortcode backfill from static DB complete"),
                    Err(e) => log::warn!("Pass 3: LP-exclusive shortcode backfill from static DB failed: {}", e),
                }
            } else {
                log::warn!("Pass 3: failed to attach {:?}, skipping LP backfill", static_db);
            }
        } else {
            log::debug!("Static exodium.db not found at {:?}, skipping Pass 3 LP backfill", static_db);
        }

        // Any rows still with NULL thumbnail_key (Pass 3 static-DB backfill
        // might have added new rows without keys) get their own-title hash.
        db::populate_thumbnail_keys(&conn).map_err(|e| e.to_string())?;

        // Final pass: match LP titles to EN via canonical form (article-
        // stripped, word-numbers-as-digits, etc.) and overwrite LP's
        // thumbnail_key with EN's. Catches the ~575 LP games whose auto-
        // generated shortcode diverged from EN but whose titles are clearly
        // the same game (e.g. PL "Legend of Kyrandia Book 2" ↔ EN
        // "The Legend of Kyrandia: Book Two").
        db::propagate_lp_thumbnail_keys(&conn).map_err(|e| e.to_string())?;
    }

    // Briefly acquire write lock just to insert - no awaits inside this block.
    {
        let mut managers = torrent_state.0.write().await;
        for (id, mgr) in new_managers {
            managers.insert(id, mgr);
        }
    }

    // The user's existing eXoDOS tree already has all DOSBox configs in place.
    // No need to extract !DOSmetadata.zip - that's only required when downloading from scratch.
    // (init_download_manager handles the bundled configs zip for fresh installs.)

    // Scan the existing eXoDOS tree to mark games that are already on disk as installed.
    let installed_count = scan_installed_games_with_db(&db_state.0, &data_dir, true)
        .unwrap_or_else(|e| { log::warn!("scan_installed_games failed: {}", e); 0 });
    log::info!("Import from local complete: {} games, {} installed, data_dir={}", count, installed_count, data_dir);

    Ok(count)
}


/// Result of validating a candidate eXoDOS installation directory.
#[derive(Debug, Serialize)]
pub struct ExodosValidation {
    pub valid: bool,
    pub hint: String,
}

/// Check whether a directory looks like a valid eXoDOS installation.
/// Expects the folder the user selected to BE the eXoDOS folder (e.g. ~/eXoDOS),
/// which should contain eXo/eXoDOS/ with at least one game language subdirectory.
#[tauri::command]
pub async fn validate_exodos_dir(path: String) -> Result<ExodosValidation, String> {
    let root = Path::new(&path);
    let game_root = root.join("eXo/eXoDOS");

    if !game_root.is_dir() {
        return Ok(ExodosValidation {
            valid: false,
            hint: "Not a valid eXoDOS folder (eXo/eXoDOS/ not found)".to_string(),
        });
    }

    let has_games = ["!dos", "!german", "!polish", "!spanish"]
        .iter()
        .any(|sub| game_root.join(sub).is_dir());

    if !has_games {
        return Ok(ExodosValidation {
            valid: false,
            hint: "No game directories found inside eXo/eXoDOS/".to_string(),
        });
    }

    // Count top-level game directories as a rough hint
    let count: usize = std::fs::read_dir(&game_root)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0);

    Ok(ExodosValidation {
        valid: true,
        hint: format!("Valid eXoDOS installation (~{} directories)", count),
    })
}


#[cfg(test)]
mod network_mode_tests {
    use super::*;
    use std::sync::Mutex;

    fn db_with(pairs: &[(&str, &str)]) -> Mutex<rusqlite::Connection> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        for (k, v) in pairs {
            queries::set_config(&conn, k, v).unwrap();
        }
        Mutex::new(conn)
    }

    #[test]
    fn missing_network_mode_means_live() {
        // Installs from before the setting exists must keep downloading.
        assert!(!is_offline(&db_with(&[])));
    }

    #[test]
    fn only_the_exact_offline_value_disables_the_engine() {
        assert!(is_offline(&db_with(&[("network_mode", "offline")])));
        assert!(!is_offline(&db_with(&[("network_mode", "live")])));
        assert!(!is_offline(&db_with(&[("network_mode", "")])));
        assert!(!is_offline(&db_with(&[("network_mode", "Offline")])));
    }

    /// Sharing uploads copyrighted data, so anything short of an explicit yes
    /// has to read as no - including the unset key of an older install.
    #[test]
    fn seeding_requires_an_explicit_yes() {
        assert!(seeding_enabled(&db_with(&[("seeding_enabled", "1")])));
        assert!(!seeding_enabled(&db_with(&[])));
        assert!(!seeding_enabled(&db_with(&[("seeding_enabled", "0")])));
        assert!(!seeding_enabled(&db_with(&[("seeding_enabled", "true")])));
    }

    /// A cap of 0 would throttle to nothing and the UI has no way to express
    /// it, so it has to read as "unlimited" like an absent or broken value.
    #[test]
    fn rate_limits_treat_zero_and_junk_as_unlimited() {
        assert_eq!(
            rate_limits(&db_with(&[("rate_limit_up_kbps", "500"), ("rate_limit_down_kbps", "2000")])),
            (Some(500), Some(2000))
        );
        assert_eq!(rate_limits(&db_with(&[])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "")])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "0")])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "-5")])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "abc")])), (None, None));
    }

    /// The offline branch of init_download_manager is exactly this call, and
    /// the import flow depends on it: an offline install still needs eXo's
    /// DOSBox configs, which normally arrive with the torrent.
    #[test]
    fn offline_still_extracts_bundled_configs() {
        let dir = std::env::temp_dir().join(format!("exodium_offline_cfg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = dir.join("metadata");
        std::fs::create_dir_all(&meta).unwrap();

        // A stand-in for eXoDOS_configs.zip carrying one config file.
        let zip_path = meta.join("eXoDOS_configs.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file::<_, ()>("eXo/eXoDOS/!dos/TEST/dosbox.conf", Default::default()).unwrap();
            use std::io::Write;
            zip.write_all(b"[sdl]\nfullscreen=false\n").unwrap();
            zip.finish().unwrap();
        }

        let data = dir.join("data");
        extract_all_bundled_configs(&["eXoDOS"], Some(&meta), &data);

        let extracted = data.join("eXoDOS").join("eXo/eXoDOS/!dos/TEST/dosbox.conf");
        assert!(extracted.is_file(), "configs must land even with no torrent session");
        assert!(data.join("eXoDOS").join(".eXoDOS_configs_extracted").is_file(),
                "lock file marks the extraction as done");

        // Second call is a no-op thanks to the lock: delete the payload and
        // confirm nothing restores it.
        std::fs::remove_file(&extracted).unwrap();
        extract_all_bundled_configs(&["eXoDOS"], Some(&meta), &data);
        assert!(!extracted.exists(), "lock file must prevent a re-extract");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Collections the user did not enable must be skipped entirely.
    #[test]
    fn disabled_collections_are_not_extracted() {
        let dir = std::env::temp_dir().join(format!("exodium_offline_skip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = dir.join("metadata");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(meta.join("eXoDOS_configs.zip"), b"not a zip").unwrap();

        let data = dir.join("data");
        extract_all_bundled_configs(&["eXoDOS_GLP"], Some(&meta), &data);

        assert!(!data.join("eXoDOS").join(".eXoDOS_configs_extracted").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}


#[cfg(test)]
mod data_dir_tests {
    use super::*;

    #[test]
    fn a_folder_with_only_os_metadata_counts_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".DS_Store"), b"x").unwrap();
        std::fs::write(tmp.path().join("._something"), b"x").unwrap();
        // A Finder visit is not an install - the warning has to fire here, or
        // it never fires for the macOS users who most need it.
        let empty = tauri::async_runtime::block_on(data_dir_is_empty(
            tmp.path().to_string_lossy().to_string(),
        ))
        .unwrap();
        assert!(empty);
    }

    #[test]
    fn a_folder_holding_game_data_is_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("eXo/eXoDOS")).unwrap();
        let empty = tauri::async_runtime::block_on(data_dir_is_empty(
            tmp.path().to_string_lossy().to_string(),
        ))
        .unwrap();
        assert!(!empty);
    }

    #[test]
    fn a_folder_that_does_not_exist_yet_counts_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let empty = tauri::async_runtime::block_on(data_dir_is_empty(
            missing.to_string_lossy().to_string(),
        ))
        .unwrap();
        assert!(empty);
    }
}
