use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tokio::sync::RwLock;

use crate::db;
use crate::db::queries;
use crate::import;
use crate::torrent::manager::{DownloadManager, DownloadProgress};
use crate::torrent::TorrentIndex;

use super::DbState;

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

    for (col_id, _, torrent_file, configs_zip) in COLLECTION_MAP {
        if !collections.contains(col_id) {
            continue;
        }
        if let Ok(torrent_path) = bundled_torrent_path(torrent_file) {
            // Each collection gets its own subdirectory to avoid file path conflicts
            // (all eXoDOS torrents share the same torrent name "eXoDOS")
            let col_data_path = if *col_id == "eXoDOS" {
                data_path.clone()
            } else {
                data_path.join(col_id)
            };
            match DownloadManager::new(&torrent_path, &col_data_path).await {
                Ok(mgr) => {
                    // Extract bundled DOSBox configs if available
                    if let Some(cfg_zip) = configs_zip {
                        if let Some(ref md) = metadata_dir {
                            let cfg_path = md.join(cfg_zip);
                            let torrent_root = mgr.torrent_root();
                            if cfg_path.exists() {
                                let lock = torrent_root.join(format!(".{}_configs_extracted", col_id));
                                if !lock.exists() {
                                    log::info!("Extracting {} configs to {}", col_id, torrent_root.display());
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
                    log::info!("Initialized download manager: {}", col_id);
                    managers.insert(col_id.to_string(), Arc::new(mgr));
                }
                Err(e) => {
                    log::warn!("Failed to init {} download manager: {}", col_id, e);
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
) -> Result<(), String> {
    // Drop all download managers
    torrent_state.0.write().await.clear();

    // Clear all tables
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
    conn.execute_batch(
        "DELETE FROM games; DELETE FROM downloads; DELETE FROM images;
         DELETE FROM playlists; DELETE FROM playlist_games; DELETE FROM config;",
    )
    .map_err(|e| e.to_string())?;

    log::info!("Factory reset completed");
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

/// Return a default data directory path ($HOME/eXoDOS).
#[tauri::command]
pub fn get_default_data_dir() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "Cannot determine home directory".to_string())?;
    let default = std::path::Path::new(&home).join("eXoDOS");
    Ok(default.to_string_lossy().to_string())
}

/// Collection ID → (metadata_file, torrent_file, optional_configs_zip)
/// Language packs listed BEFORE eXoDOS so their games get matched to the correct
/// torrent before eXoDOS matching can claim them (same-title translations).
const COLLECTION_MAP: &[(&str, &str, &str, Option<&str>)] = &[
    ("eXoDOS_GLP", "GLP.xml.gz", "eXoDOS_GLP.torrent", Some("GLP_configs.zip")),
    ("eXoDOS_PLP", "PLP.xml.gz", "eXoDOS_PLP.torrent", None),
    ("eXoDOS_SLP", "SLP.xml.gz", "eXoDOS_SLP.torrent", None),
    ("eXoDOS", "MS-DOS.xml.gz", "eXoDOS.torrent", Some("eXoDOS_configs.zip")),
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
            // Ready if data_dir is configured (games come from bundled DB)
            let (has_data_dir, count) = {
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                let dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
                let count = queries::count_games(&conn, "", "").map_err(|e| e.to_string())?;
                (dir.is_some(), count)
            };
            return Ok(SetupStatus {
                phase: if has_data_dir {
                    "ready".to_string()
                } else {
                    "not_started".to_string()
                },
                metadata_progress: None,
                dosbox_metadata_progress: None,
                games_imported: count,
                ready: has_data_dir,
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
        queries::count_games(&conn, "", "").map_err(|e| e.to_string())?
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
            import::import_from_zip(&zip, &conn).map_err(|e| e.to_string())?;

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
/// `shared_gamedata_index` is the eXoDOS torrent index for looking up shared GameData for LP games.
fn match_torrent_indices(
    conn: &rusqlite::Connection,
    index: &TorrentIndex,
    torrent_source: &str,
) -> Result<(), String> {
    match_torrent_indices_with_shared(conn, index, torrent_source, None)
}

fn match_torrent_indices_with_shared(
    conn: &rusqlite::Connection,
    index: &TorrentIndex,
    torrent_source: &str,
    shared_gamedata_index: Option<&TorrentIndex>,
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
                let mut size = game.size as i64
                    + gamedata_entry.map(|g| g.size as i64).unwrap_or(0);

                // For LP games, add shared EN GameData size from eXoDOS torrent
                if let Some(shared_idx) = shared_gamedata_index {
                    let (_, shared_gd) = shared_idx.find_game_files(&search_name);
                    if let Some(gd) = shared_gd {
                        size += gd.size as i64;
                    }
                }

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
/// Searches for XODOSMetadata.zip or MS-DOS.xml in the given path.
#[tauri::command]
pub async fn setup_from_local(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    exodos_path: String,
    data_dir: String,
) -> Result<usize, String> {
    // Save data_dir to config
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
    }

    let root = PathBuf::from(&exodos_path);

    // Search for the metadata ZIP in common locations
    let zip_candidates = [
        root.join("Content/XODOSMetadata.zip"),
        root.join("eXo/Content/XODOSMetadata.zip"),
        root.join("XODOSMetadata.zip"),
    ];

    let zip_path = zip_candidates
        .iter()
        .find(|p| p.exists())
        .cloned();

    // Or look for already-extracted XML
    let xml_candidates = [
        root.join("xml/all/MS-DOS.xml"),
        root.join("eXo/xml/all/MS-DOS.xml"),
    ];

    let xml_path = xml_candidates
        .iter()
        .find(|p| p.exists())
        .cloned();

    if zip_path.is_none() && xml_path.is_none() {
        return Err(format!(
            "No eXoDOS metadata found in {}. Expected Content/XODOSMetadata.zip or xml/all/MS-DOS.xml",
            exodos_path
        ));
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

    // Get DB path and torrent index for the blocking task
    let db_path = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.path()
            .map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    let torrent_path = bundled_torrent_path("eXoDOS.torrent")?;
    let torrent_index = TorrentIndex::from_file(&torrent_path)
        .map_err(|e| format!("Failed to parse torrent: {}", e))?;

    let count = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        db::init(&conn).map_err(|e| e.to_string())?;

        // Clear existing games to prevent duplicates on re-import
        queries::clear_games(&conn).map_err(|e| e.to_string())?;

        let count = if let Some(zip) = zip_path {
            log::info!("Importing from ZIP: {}", zip.display());
            import::import_from_zip(&zip, &conn).map_err(|e| e.to_string())?
        } else if let Some(xml) = xml_path {
            log::info!("Importing from XML: {}", xml.display());
            let file = std::fs::File::open(&xml).map_err(|e| e.to_string())?;
            let reader = std::io::BufReader::new(file);
            let games = import::xml::parse_games_xml(reader).map_err(|e| e.to_string())?;
            let count = games.len();
            db::queries::insert_games(&conn, &games).map_err(|e| e.to_string())?;
            count
        } else {
            0
        };

        match_torrent_indices(&conn, &torrent_index, "eXoDOS").map_err(|e| e.to_string())?;

        Ok::<usize, String>(count)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Init the download manager for future game downloads
    let data_path = PathBuf::from(&data_dir);
    let manager = DownloadManager::new(&torrent_path, &data_path)
        .await
        .map_err(|e| format!("Failed to init download manager: {}", e))?;

    // Extract !DOSmetadata.zip (DOSBox configs) into the torrent root
    if let Some(dosbox_zip) = dosbox_zip_path {
        let torrent_root = manager.torrent_root();
        tauri::async_runtime::spawn_blocking(move || {
            log::info!("Extracting DOSBox configs to {}", torrent_root.display());
            let file = std::fs::File::open(&dosbox_zip).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            archive.extract(&torrent_root).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
    }

    torrent_state.0.write().await.insert("eXoDOS".to_string(), Arc::new(manager));

    Ok(count)
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
