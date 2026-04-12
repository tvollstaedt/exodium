use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::db;
use crate::db::queries;
use crate::import;
use crate::torrent::manager::{DownloadManager, DownloadProgress};
use crate::torrent::TorrentIndex;

use super::DbState;

/// Metadata describing a single eXo collection.
/// All path conventions for a collection are captured here so that game
/// launch / install / uninstall code does not need to hard-code any
/// collection-specific strings.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionDef {
    /// Internal collection ID (e.g. "eXoDOS", "eXoDOS_GLP").
    pub id: &'static str,
    /// Human-readable name shown in the UI.
    pub display_name: &'static str,
    /// Bundled metadata XML gz file (e.g. "MS-DOS.xml.gz").
    pub metadata_file: &'static str,
    /// Bundled .torrent filename (e.g. "eXoDOS.torrent").
    pub torrent_file: &'static str,
    /// Optional bundled DOSBox/emulator configs ZIP.
    pub configs_zip: Option<&'static str>,
    /// The folder name the torrent creates inside the data dir (always "eXoDOS").
    /// All four collections (eXoDOS, GLP, PLP, SLP) share the same output folder via
    /// the overlay model — their torrents all have the internal name "eXoDOS" and write
    /// to <data_dir>/eXoDOS/ without any per-collection subdirectory.
    pub inner_folder: &'static str,
    /// Path from <inner_folder> to the individual game directories.
    /// e.g. "eXo/eXoDOS" → games are at <inner_folder>/eXo/eXoDOS/<shortcode>/
    pub game_prefix: &'static str,
    /// Segment in the LaunchBox application_path used to extract the shortcode.
    /// e.g. "!dos" for eXoDOS (path looks like "eXo\eXoDOS\!dos\<shortcode>\…")
    pub shortcode_segment: &'static str,
    /// Language subdirectory inside game_prefix for LP variant games.
    /// None for the base English collection.
    pub lang_dir: Option<&'static str>,
}

/// Look up a collection definition by ID.  Returns None for unknown IDs.
pub fn collection_def(id: &str) -> Option<&'static CollectionDef> {
    COLLECTION_MAP.iter().find(|c| c.id == id)
}

/// Serialisable summary returned by the `get_available_collections` command.
#[derive(Debug, Serialize)]
pub struct CollectionInfo {
    pub id: String,
    pub display_name: String,
    pub torrent_file: String,
}

/// Return the list of all known collections (for the frontend to render
/// collection pickers / labels without hardcoding IDs).
#[tauri::command]
pub fn get_available_collections() -> Vec<CollectionInfo> {
    COLLECTION_MAP
        .iter()
        .map(|c| CollectionInfo {
            id: c.id.to_string(),
            display_name: c.display_name.to_string(),
            torrent_file: c.torrent_file.to_string(),
        })
        .collect()
}

/// Managed state for the download system — supports multiple torrents.
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
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    };

    let data_dir = match data_dir {
        Some(d) => d,
        None => return Ok(false),
    };

    let data_path = PathBuf::from(&data_dir);

    // Get selected collections from config
    let collections_str = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "collections")
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "eXoDOS".to_string())
    };
    let collections: Vec<&str> = collections_str.split(',').collect();

    let mut managers = torrent_state.0.write().await;
    let metadata_dir = bundled_metadata_dir().ok();

    // All collections share one librqbit session and the same data directory.
    // The torrent files have no overlapping file paths, so all four torrents
    // extract cleanly into <data_dir>/eXoDOS/ — matching the original eXoDOS layout.
    // Session state (.librqbit/) is stored in the app config dir, not the game data dir.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let session = DownloadManager::create_session(&config_dir)
        .await
        .map_err(|e| e.to_string())?;

    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        if let Ok(torrent_path) = bundled_torrent_path(col.torrent_file) {
            match DownloadManager::new_with_session(Arc::clone(&session), &torrent_path, &data_path) {
                Ok(mgr) => {
                    // Store the torrent infohash so the update-checker can compare later
                    match TorrentIndex::infohash(&torrent_path) {
                        Ok(hash) => {
                            match db_state.0.lock() {
                                Ok(conn) => {
                                    if let Err(e) = queries::set_config(&conn, &format!("{}_infohash", col.id), &hash) {
                                        log::warn!("Failed to save infohash for {}: {}", col.id, e);
                                    }
                                }
                                Err(e) => log::warn!("Failed to lock DB for infohash write ({}): {}", col.id, e),
                            }
                        }
                        Err(e) => log::warn!("Failed to compute infohash for {}: {}", col.id, e),
                    }

                    // Extract bundled emulator configs if available
                    if let Some(cfg_zip) = col.configs_zip {
                        if let Some(ref md) = metadata_dir {
                            let cfg_path = md.join(cfg_zip);
                            let torrent_root = mgr.torrent_root();
                            if cfg_path.exists() {
                                let lock = torrent_root.join(format!(".{}_configs_extracted", col.id));
                                if !lock.exists() {
                                    log::info!("Extracting {} configs to {}", col.id, torrent_root.display());
                                    if let Ok(file) = std::fs::File::open(&cfg_path) {
                                        if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                            let _ = archive.extract(&torrent_root);
                                            let _ = std::fs::write(&lock, "");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    log::info!("Initialized download manager: {}", col.id);
                    managers.insert(col.id.to_string(), Arc::new(mgr));
                }
                Err(e) => {
                    log::warn!("Failed to init {} download manager: {}", col.id, e);
                }
            }
        }
    }

    log::info!("Download managers initialized: {} (data_dir: {})", managers.len(), data_dir);
    Ok(!managers.is_empty())
}

/// Reset all data: clear DB, remove config. Returns to setup state.
#[tauri::command]
pub async fn factory_reset(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    delete_game_data: bool,
) -> Result<(), String> {
    // Read data_dir before clearing config (Mutex must not be held across await)
    let data_dir = if delete_game_data {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    } else {
        None
    };

    // Drop all download managers
    torrent_state.0.write().await.clear();

    // Reset user state without touching the game catalog.
    // Games are catalog data (from the bundled DB) — clearing them would leave
    // the library empty until next restart. Only reset per-user flags and config.
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "UPDATE games SET in_library = 0, installed = 0, favorited = 0;
             DELETE FROM downloads;
             DELETE FROM images;
             DELETE FROM playlists;
             DELETE FROM playlist_games;
             DELETE FROM config;",
        )
        .map_err(|e| e.to_string())?;
    }

    // Optionally delete the eXoDOS game folder.
    // data_dir is the PARENT of the eXoDOS folder, so we delete <data_dir>/eXoDOS/,
    // never data_dir itself (which could be the home directory).
    if let Some(dir) = data_dir {
        if !dir.is_empty() {
            let exodos_path = std::path::Path::new(&dir).join("eXoDOS");
            if exodos_path.exists() {
                log::info!("Deleting game data folder: {}", exodos_path.display());
                if let Err(e) = std::fs::remove_dir_all(&exodos_path) {
                    log::error!("Failed to delete game data folder: {}", e);
                    return Err(format!("Failed to delete game data: {}", e));
                }
            }
        }
    }

    log::info!("Factory reset completed (delete_game_data={})", delete_game_data);
    Ok(())
}

/// Get the thumbnail directory path.
/// Checks: dev project dir → data_dir/thumbnails → exe dir/thumbnails
#[tauri::command]
pub fn get_thumbnail_dir(
    db_state: State<DbState>,
    collection: String,
) -> Result<String, String> {
    // Dev: project directory
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("thumbnails").join(&collection))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(dev_path.to_string_lossy().to_string());
    }

    // Production: data_dir/thumbnails/<collection>
    if let Ok(conn) = db_state.0.lock() {
        if let Ok(Some(data_dir)) = queries::get_config(&conn, "data_dir") {
            let prod_path = PathBuf::from(&data_dir).join("thumbnails").join(&collection);
            if prod_path.exists() {
                return Ok(prod_path.to_string_lossy().to_string());
            }
        }
    }

    Err("Thumbnail directory not found".to_string())
}

/// Return the default parent directory for game storage ($HOME).
/// The eXoDOS folder will be created inside this directory by the torrent engine.
#[tauri::command]
pub fn get_default_data_dir() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "Cannot determine home directory".to_string())?;
    Ok(home)
}

/// All known eXo collections.
/// Language packs are listed BEFORE eXoDOS so their games are matched to the
/// correct torrent before eXoDOS can claim same-title translations.
/// To add a new collection, append a CollectionDef entry here — no other
/// Rust file needs to be changed for path/emulator dispatch.
pub const COLLECTION_MAP: &[CollectionDef] = &[
    CollectionDef {
        id: "eXoDOS_GLP",
        display_name: "German Language Pack",
        metadata_file: "GLP.xml.gz",
        torrent_file: "eXoDOS_GLP.torrent",
        configs_zip: Some("GLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!german"),
    },
    CollectionDef {
        id: "eXoDOS_PLP",
        display_name: "Polish Language Pack",
        metadata_file: "PLP.xml.gz",
        torrent_file: "eXoDOS_PLP.torrent",
        configs_zip: None,
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!polish"),
    },
    CollectionDef {
        id: "eXoDOS_SLP",
        display_name: "Spanish Language Pack",
        metadata_file: "SLP.xml.gz",
        torrent_file: "eXoDOS_SLP.torrent",
        configs_zip: None,
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!spanish"),
    },
    CollectionDef {
        id: "eXoDOS",
        display_name: "eXoDOS",
        metadata_file: "MS-DOS.xml.gz",
        torrent_file: "eXoDOS.torrent",
        configs_zip: Some("eXoDOS_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: None,
    },
];

/// Resolve bundled metadata directory.
pub fn bundled_metadata_dir() -> Result<PathBuf, String> {
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("metadata"))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(dev_path);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let prod_path = dir.join("metadata");
            if prod_path.exists() {
                return Ok(prod_path);
            }
        }
    }

    Err(format!("Bundled metadata not found (checked: {})", dev_path.display()))
}

/// Get info about the bundled torrent without starting anything.
#[tauri::command]
pub fn get_torrent_info() -> Result<TorrentInfo, String> {
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
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    data_dir: String,
) -> Result<String, String> {
    // Save data_dir to config
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
    }

    let torrent_path = bundled_torrent_path("eXoDOS.torrent")?;
    let data_path = PathBuf::from(&data_dir);

    let manager = DownloadManager::new(&torrent_path, &data_path)
        .await
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
    let guard = torrent_state.0.read().await;
    let manager = match guard.get("eXoDOS") {
        Some(m) => m,
        None => {
            // Ready if data_dir is configured AND the game DB has content
            let (has_data_dir, count) = {
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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
    let guard = torrent_state.0.read().await;
    let manager = guard
        .get("eXoDOS")
        .ok_or("Download manager not initialized")?;

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
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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

/// Extract the game name (with year) from the application_path.
/// e.g. "eXo\eXoDOS\!dos\captlsm\Capitalism (1995).bat" -> "Capitalism (1995)"
pub fn game_name_from_app_path(app_path: &str) -> Option<String> {
    let normalized = app_path.replace('\\', "/");
    let filename = normalized.rsplit('/').next()?;
    let name = filename.strip_suffix(".bat")?;
    Some(name.to_string())
}

/// Match imported games to their torrent file indices.
/// `torrent_source` identifies which torrent file this is.
fn match_torrent_indices(
    conn: &rusqlite::Connection,
    index: &TorrentIndex,
    torrent_source: &str,
) -> Result<(), String> {
    let mut matched = 0;
    let mut unmatched = 0;

    // Only match games that don't already have a torrent index
    let mut stmt = conn
        .prepare("SELECT id, title, application_path FROM games WHERE game_torrent_index IS NULL")
        .map_err(|e| e.to_string())?;
    let games: Vec<(i64, String, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    {
        let mut update_stmt = tx
            .prepare_cached(
                "UPDATE games SET game_torrent_index = ?1, gamedata_torrent_index = ?2,
                 download_size = ?3, torrent_source = ?4 WHERE id = ?5",
            )
            .map_err(|e| e.to_string())?;

        for (id, title, app_path) in &games {
            let search_name = app_path
                .as_deref()
                .and_then(game_name_from_app_path)
                .unwrap_or_else(|| title.clone());

            let (game_entry, gamedata_entry) = index.find_game_files(&search_name);

            if let Some(game) = game_entry {
                let gamedata_idx = gamedata_entry.map(|g| g.index as i64);
                let size = game.size as i64
                    + gamedata_entry.map(|g| g.size as i64).unwrap_or(0);

                update_stmt
                    .execute(rusqlite::params![
                        game.index as i64,
                        gamedata_idx,
                        size,
                        torrent_source,
                        id,
                    ])
                    .map_err(|e| e.to_string())?;
                matched += 1;
            } else {
                unmatched += 1;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;

    log::info!(
        "Torrent index matching: {} matched, {} unmatched out of {} games",
        matched,
        unmatched,
        games.len()
    );
    Ok(())
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

    // Save data_dir and all collections to config
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
        let all_collections = COLLECTION_MAP.iter().map(|c| c.id).collect::<Vec<_>>().join(",");
        queries::set_config(&conn, "collections", &all_collections).map_err(|e| e.to_string())?;
    }

    // Find !DOSmetadata.zip for DOSBox configs
    let dosbox_zip_candidates = [
        root.join("Content/!DOSmetadata.zip"),
        root.join("eXo/Content/!DOSmetadata.zip"),
        root.join("!DOSmetadata.zip"),
    ];
    let dosbox_zip_path = dosbox_zip_candidates
        .iter()
        .find(|p| p.exists())
        .cloned();

    // The bundled DB already has the full game catalog — no need to re-parse the
    // eXoDOS XML (XODOSMetadata.zip is 5 GB and would block for minutes).
    // Just report how many games are in the current DB.
    let count = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT COUNT(*) FROM games", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    };

    // Init download managers for all collections (for future game downloads).
    // Session state goes in the app config dir, game files in the existing eXoDOS tree.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let data_path = PathBuf::from(&data_dir);

    let session = DownloadManager::create_session(&config_dir)
        .await
        .map_err(|e| format!("Failed to init session: {}", e))?;

    let mut managers = torrent_state.0.write().await;
    for col in COLLECTION_MAP {
        if let Ok(col_torrent_path) = bundled_torrent_path(col.torrent_file) {
            match DownloadManager::new_with_session(Arc::clone(&session), &col_torrent_path, &data_path) {
                Ok(mgr) => {
                    managers.insert(col.id.to_string(), Arc::new(mgr));
                }
                Err(e) => log::warn!("Failed to init {} download manager: {}", col.id, e),
            }
        }
    }

    // Extract !DOSmetadata.zip (DOSBox configs) into the torrent root if found
    if let Some(dosbox_zip) = dosbox_zip_path {
        if let Some(main_mgr) = managers.get("eXoDOS") {
            let torrent_root = main_mgr.torrent_root();
            let dosbox_zip = dosbox_zip.clone();
            tauri::async_runtime::spawn_blocking(move || {
                log::info!("Extracting DOSBox configs to {}", torrent_root.display());
                let file = std::fs::File::open(&dosbox_zip).map_err(|e| e.to_string())?;
                let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
                archive.extract(&torrent_root).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
        }
    }

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
pub fn validate_exodos_dir(path: String) -> Result<ExodosValidation, String> {
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

/// Resolve bundled torrent file path.
fn bundled_torrent_path(filename: &str) -> Result<PathBuf, String> {
    // In development, look relative to Cargo manifest
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("torrents").join(filename))
        .unwrap_or_default();

    if dev_path.exists() {
        return Ok(dev_path);
    }

    // In production, look next to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let prod_path = dir.join("torrents").join(filename);
            if prod_path.exists() {
                return Ok(prod_path);
            }
        }
    }

    Err(format!(
        "Bundled torrent '{}' not found (checked: {})",
        filename,
        dev_path.display()
    ))
}
