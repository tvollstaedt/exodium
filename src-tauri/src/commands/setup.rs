use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;

/// Cached Tauri resource_dir(), set once during app setup. Needed because
/// sync helpers (bundled_metadata_dir, bundled_torrent_path) are called from
/// contexts that don't carry an AppHandle — and without this cache they'd
/// have to be plumbed everywhere.
pub(crate) static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Called once from lib.rs' setup closure with the app's resource directory.
pub fn init_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

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

    let metadata_dir = bundled_metadata_dir().ok();

    // All collections share one librqbit session and the same data directory.
    // Session state (.librqbit/) is stored in the app config dir, not the game data dir.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let session = DownloadManager::create_session(&config_dir)
        .await
        .map_err(|e| e.to_string())?;

    // Build all managers and do slow work (infohash, config extraction) WITHOUT holding
    // the torrent_state write lock — archive.extract() on 7 000+ files blocks for seconds.
    let mut new_managers: Vec<(String, Arc<DownloadManager>)> = Vec::new();

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
                    new_managers.push((col.id.to_string(), Arc::new(mgr)));
                }
                Err(e) => {
                    log::warn!("Failed to init {} download manager: {}", col.id, e);
                }
            }
        }
    }

    // Acquire write lock only for the insert — no blocking work inside.
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

/// Reset all data: clear DB, remove config. Returns to setup state.
#[tauri::command]
pub async fn factory_reset(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    delete_game_data: bool,
) -> Result<(), String> {
    log::info!("factory_reset called (delete_game_data={})", delete_game_data);
    // Read data_dir before clearing config (Mutex must not be held across await)
    let data_dir = if delete_game_data {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    } else {
        None
    };

    // Drop all download managers. Use a timeout so a stuck reader doesn't hang forever.
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        torrent_state.0.write(),
    ).await {
        Ok(mut managers) => managers.clear(),
        Err(_) => {
            log::error!("factory_reset: timed out waiting for torrent write lock");
            return Err("Could not stop downloads in time. Cancel any active downloads and try again.".to_string());
        }
    }

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

    // Optionally delete the eXoDOS game folder + content packs + stale downloads.
    // data_dir is the PARENT of the eXoDOS folder, so we delete <data_dir>/eXoDOS/,
    // never data_dir itself (which could be the home directory).
    if let Some(dir) = data_dir {
        if !dir.is_empty() {
            let base = std::path::Path::new(&dir);
            let exodos_path = base.join("eXoDOS");
            if exodos_path.exists() {
                log::info!("Deleting game data folder: {}", exodos_path.display());
                if let Err(e) = std::fs::remove_dir_all(&exodos_path) {
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
        }
    }

    log::info!("Factory reset completed (delete_game_data={})", delete_game_data);
    Ok(())
}

/// Convert a PathBuf to a forward-slash string. Tauri's convertFileSrc on
/// the frontend expects consistent separators when we later join `${dir}/${file}`;
/// mixed Windows backslash + frontend forward slash produces broken asset URLs.
fn path_to_fwd_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
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
        return Ok(path_to_fwd_slash(&dev_path));
    }

    // Production: data_dir/thumbnails/<collection>
    if let Ok(conn) = db_state.0.lock() {
        if let Ok(Some(data_dir)) = queries::get_config(&conn, "data_dir") {
            let prod_path = PathBuf::from(&data_dir).join("thumbnails").join(&collection);
            if prod_path.exists() {
                return Ok(path_to_fwd_slash(&prod_path));
            }
        }
    }

    Err("Thumbnail directory not found".to_string())
}

/// Get the Tier 0 preview directory for a collection.
/// Checks multiple platform-specific layouts because Tauri's bundle.resources
/// placement varies: macOS uses Contents/Resources/, Linux deb uses
/// /usr/lib/<pkg>/, AppImage uses <mount>/usr/lib/<pkg>/, Windows flat-installs
/// into the install directory.
#[tauri::command]
pub fn get_preview_dir(collection: String) -> Result<String, String> {
    // Dev mode: direct from repo tree.
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("previews")
        .join(&collection);
    if dev_path.exists() {
        return Ok(path_to_fwd_slash(&dev_path));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Tauri's reported resource_dir (canonical)
    if let Some(res_dir) = RESOURCE_DIR.get() {
        candidates.push(res_dir.join("previews").join(&collection));
    }

    // 2. Next to the executable (Windows flat install, macOS Contents/MacOS)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("previews").join(&collection));
            candidates.push(exe_dir.join("resources").join("previews").join(&collection));
            // Linux /usr layout: /usr/bin/exodium → /usr/lib/exodium/previews/
            if let Some(usr_dir) = exe_dir.parent() {
                candidates.push(
                    usr_dir
                        .join("lib")
                        .join("exodium")
                        .join("previews")
                        .join(&collection),
                );
                candidates.push(
                    usr_dir
                        .join("share")
                        .join("exodium")
                        .join("previews")
                        .join(&collection),
                );
            }
        }
    }

    for candidate in &candidates {
        if candidate.exists() {
            log::info!("get_preview_dir: found at {}", candidate.display());
            return Ok(path_to_fwd_slash(candidate));
        }
    }

    let checked: Vec<String> = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    log::warn!(
        "get_preview_dir({}): not found. Checked: {}",
        collection,
        checked.join(", ")
    );
    Err(format!(
        "Preview directory not found. Checked: {}",
        checked.join(", ")
    ))
}

/// Get the Tier 1 poster content-pack directory for a collection.
/// Returns <data_dir>/content/posters/<collection> if it exists.
#[tauri::command]
pub fn get_poster_dir(
    db_state: State<DbState>,
    collection: String,
) -> Result<String, String> {
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
    let data_dir = queries::get_config(&conn, "data_dir")
        .map_err(|e| e.to_string())?
        .ok_or("Data directory not configured")?;
    let base = PathBuf::from(&data_dir).join("content").join("posters");
    // Check collection-specific dir first, fall back to eXoDOS.
    // All poster thumbnails live in the eXoDOS pack; LP collections share them.
    let poster_path = base.join(&collection);
    if poster_path.exists() {
        return Ok(path_to_fwd_slash(&poster_path));
    }
    if collection != "eXoDOS" {
        let fallback = base.join("eXoDOS");
        if fallback.exists() {
            return Ok(path_to_fwd_slash(&fallback));
        }
    }
    Err("Poster directory not found".to_string())
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
///
/// Dev mode reads straight from the repo tree via CARGO_MANIFEST_DIR. Prod
/// mode looks inside the Tauri resource_dir cached by `init_resource_dir`
/// at app startup. current_exe().parent() is NOT used because on macOS
/// that's Contents/MacOS/ while bundled resources live in Contents/Resources/.
pub fn bundled_metadata_dir() -> Result<PathBuf, String> {
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("metadata"))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(dev_path);
    }

    if let Some(res_dir) = RESOURCE_DIR.get() {
        let prod_path = res_dir.join("metadata");
        if prod_path.exists() {
            return Ok(prod_path);
        }
        return Err(format!(
            "Bundled metadata not found in resource dir {} (dev path also missing: {})",
            res_dir.display(),
            dev_path.display()
        ));
    }

    Err(format!(
        "Bundled metadata not found: resource_dir uninitialized and dev path {} missing",
        dev_path.display()
    ))
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
    // Build managers WITHOUT holding the write lock — create_session is async and can be slow.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let data_path = PathBuf::from(&data_dir);

    let session = DownloadManager::create_session(&config_dir)
        .await
        .map_err(|e| format!("Failed to init session: {}", e))?;

    let mut new_managers = Vec::new();
    for col in COLLECTION_MAP {
        if let Ok(col_torrent_path) = bundled_torrent_path(col.torrent_file) {
            match DownloadManager::new_with_session(Arc::clone(&session), &col_torrent_path, &data_path) {
                Ok(mgr) => new_managers.push((col.id.to_string(), Arc::new(mgr))),
                Err(e) => log::warn!("Failed to init {} download manager: {}", col.id, e),
            }
        }
    }

    // Backfill any LP collections that are absent from the DB.
    // This happens when the DB was originally built from XODOSMetadata.zip (EN only) by the
    // old setup path.  The bundled .xml.gz files always include all LP catalogs, so we import
    // whichever collections are missing and then run match_torrent_indices to wire up
    // torrent_source / game_torrent_index.
    if let Ok(metadata_dir) = bundled_metadata_dir() {
        for (col_id, manager) in &new_managers {
            let col = match collection_def(col_id) {
                Some(c) => c,
                None => continue,
            };
            if col.lang_dir.is_none() {
                continue; // base eXoDOS — skip
            }

            let already_in_db: i64 = {
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                import::import_from_gz(&xml_gz, &conn, col.shortcode_segment)
                    .unwrap_or_else(|e| {
                        log::warn!("Failed to import {} XML: {}", col_id, e);
                        0
                    })
            };
            log::info!("Backfilled {} {} games from bundled XML", imported, col_id);

            if imported > 0 {
                // Wire up game_torrent_index and torrent_source for the newly imported rows.
                let torrent_index = manager.index().clone();
                let col_id_owned = col_id.clone();
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                if let Err(e) = match_torrent_indices(&conn, &torrent_index, &col_id_owned) {
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
            let conn = db_state.0.lock().map_err(|e| e.to_string())?;
            db::populate_thumbnail_keys(&conn).map_err(|e| e.to_string())?;
        }

        // Backfill shortcodes, dosbox_conf and has_thumbnail for LP games that lack them.
        // PLP and SLP XMLs use a path format without the "!dos/<shortcode>" segment, so their
        // shortcodes (and derived dosbox_conf) come out as NULL after import.  Mirror the same
        // two-step approach as generate_db.rs: exact EN title match first.
        // This is idempotent — rows already having values are unaffected.
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let _ = conn.execute_batch(
            "-- Inherit shortcode from the matching EN game by exact title
             UPDATE games
             SET shortcode = (
                 SELECT en.shortcode FROM games en
                 WHERE en.language = 'EN'
                   AND en.shortcode IS NOT NULL
                   AND en.title = games.title
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
                 WHERE en.shortcode = games.shortcode
                   AND en.language = 'EN'
                   AND en.dosbox_conf IS NOT NULL
                 LIMIT 1
             )
             WHERE dosbox_conf IS NULL AND shortcode IS NOT NULL;

             -- Propagate has_thumbnail flag from EN variant (same shortcode = same cover art)
             UPDATE games
             SET has_thumbnail = 1
             WHERE shortcode IN (
                 SELECT shortcode FROM games WHERE language = 'EN' AND has_thumbnail = 1
             )
             AND has_thumbnail = 0;

             -- Overwrite LP variants' thumbnail_key with EN's hash so German/
             -- Polish/Spanish versions share the EN primary's cover art. This
             -- runs after populate_thumbnail_keys set every row to its own-title
             -- hash, so LP rows here lose their per-language hash in favor of
             -- the shared EN hash. Rows without an EN match keep their own.
             UPDATE games
             SET thumbnail_key = (
                 SELECT en.thumbnail_key FROM games en
                 WHERE en.language = 'EN'
                   AND en.shortcode = games.shortcode
                   AND en.thumbnail_key IS NOT NULL
                 LIMIT 1
             )
             WHERE shortcode IS NOT NULL
               AND language != 'EN'
               AND EXISTS (
                   SELECT 1 FROM games en
                   WHERE en.language = 'EN'
                     AND en.shortcode = games.shortcode
                     AND en.thumbnail_key IS NOT NULL
               );",
        );
        log::info!("LP shortcode/dosbox_conf/has_thumbnail/thumbnail_key backfill complete");

        // Pass 3: pull shortcodes for LP-exclusive games from the bundled static DB.
        // metadata/exodium.db (built by generate_db.rs) contains 100% shortcode coverage
        // including LP-exclusive games with no EN equivalent (via generate_shortcode()).
        // Passes 1 & 2 only matched titles present in the EN catalog; this covers the rest.
        //
        // ATTACH and DETACH are issued as separate calls so DETACH always runs even when an
        // UPDATE fails — execute_batch stops at the first error, which would leave lp_static
        // attached for the lifetime of the connection if DETACH were part of the same batch.
        let static_db = metadata_dir.join("exodium.db");
        if static_db.exists() {
            let path_esc = static_db.to_string_lossy().replace('\'', "''");
            let attach_ok = conn
                .execute_batch(&format!("ATTACH DATABASE '{path}' AS lp_static;", path = path_esc))
                .is_ok();
            if attach_ok {
                let result = conn.execute_batch(
                    "UPDATE games
                     SET shortcode = (
                         SELECT s.shortcode FROM lp_static.games s
                         WHERE s.title = games.title AND s.shortcode IS NOT NULL
                         LIMIT 1
                     )
                     WHERE shortcode IS NULL AND language != 'EN';
                     UPDATE games
                     SET has_thumbnail = COALESCE((
                         SELECT s.has_thumbnail FROM lp_static.games s
                         WHERE s.title = games.title
                         LIMIT 1
                     ), has_thumbnail)
                     WHERE language != 'EN' AND shortcode IS NOT NULL;
                     UPDATE games
                     SET thumbnail_key = COALESCE((
                         SELECT s.thumbnail_key FROM lp_static.games s
                         WHERE s.title = games.title AND s.thumbnail_key IS NOT NULL
                         LIMIT 1
                     ), thumbnail_key)
                     WHERE thumbnail_key IS NULL AND language != 'EN';
                     UPDATE games
                     SET dosbox_conf = (
                         SELECT en.dosbox_conf FROM games en
                         WHERE en.shortcode = games.shortcode
                           AND en.language = 'EN'
                           AND en.dosbox_conf IS NOT NULL
                         LIMIT 1
                     )
                     WHERE dosbox_conf IS NULL AND shortcode IS NOT NULL;",
                );
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

    // Briefly acquire write lock just to insert — no awaits inside this block.
    {
        let mut managers = torrent_state.0.write().await;
        for (id, mgr) in new_managers {
            managers.insert(id, mgr);
        }
    }

    // The user's existing eXoDOS tree already has all DOSBox configs in place.
    // No need to extract !DOSmetadata.zip — that's only required when downloading from scratch.
    // (init_download_manager handles the bundled configs zip for fresh installs.)

    // Scan the existing eXoDOS tree to mark games that are already on disk as installed.
    let installed_count = scan_installed_games_with_db(&db_state.0, &data_dir)
        .unwrap_or_else(|e| { log::warn!("scan_installed_games failed: {}", e); 0 });
    log::info!("Import from local complete: {} games, {} installed, data_dir={}", count, installed_count, data_dir);

    Ok(count)
}

/// Scan the eXoDOS directory tree and mark games whose directories exist on disk
/// as `installed = 1, in_library = 1`.  Returns the number of rows updated.
///
/// This is called automatically at the end of `setup_from_local` and is also
/// exposed as the `scan_installed_games` Tauri command so the user can re-run it
/// from the Settings panel after manually adding game files.
fn scan_installed_games_with_db(
    db: &std::sync::Mutex<rusqlite::Connection>,
    data_dir: &str,
) -> Result<usize, String> {
    // The eXoDOS torrent always creates a folder called "eXoDOS" inside data_dir.
    // Extracted game data lives at:
    //   eXo/eXoDOS/<shortcode>/           — English (eXoDOS)
    //   eXo/eXoDOS/!german/<shortcode>/   — German LP (GLP)
    //   eXo/eXoDOS/!polish/<shortcode>/   — Polish LP (PLP)
    //   eXo/eXoDOS/!spanish/<shortcode>/  — Spanish LP (SLP)
    //
    // Note: eXo/eXoDOS/!dos/<shortcode>/ contains only config/script files and is
    // ALWAYS present in any eXoDOS installation — it is NOT an indicator of game installation.
    let game_base = PathBuf::from(data_dir)
        .join("eXoDOS")
        .join("eXo")
        .join("eXoDOS");

    // Reset all installed flags before the scan so that games whose extracted directory
    // was removed are correctly flipped back to "not installed".
    // in_library is also cleared — the scan is the authoritative source for local installs.
    {
        let conn = db.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("UPDATE games SET installed = 0, in_library = 0")
            .map_err(|e| e.to_string())?;
    }

    let mut total = 0usize;

    for col in COLLECTION_MAP {
        let shortcodes: Vec<String> = if let Some(lang_dir) = col.lang_dir {
            // LP collection: extracted game data is at game_base/<lang_dir>/<shortcode>/
            let seg_dir = game_base.join(lang_dir);
            if !seg_dir.is_dir() {
                continue;
            }
            match std::fs::read_dir(&seg_dir) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect(),
                Err(e) => {
                    log::warn!("scan_installed_games: cannot read {}: {}", seg_dir.display(), e);
                    continue;
                }
            }
        } else {
            // Base EN collection: extracted game data is directly at game_base/<shortcode>/
            // Filter out system/language dirs (starting with '!' or '.') which are always present.
            if !game_base.is_dir() {
                continue;
            }
            match std::fs::read_dir(&game_base) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|name| !name.starts_with('!') && !name.starts_with('.'))
                    .collect(),
                Err(e) => {
                    log::warn!("scan_installed_games: cannot read {}: {}", game_base.display(), e);
                    continue;
                }
            }
        };

        if shortcodes.is_empty() {
            continue;
        }

        // Build "UPDATE … WHERE shortcode IN (?, ?, …) AND torrent_source = ?"
        let placeholders = shortcodes.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE games SET installed = 1, in_library = 1 WHERE shortcode IN ({}) AND torrent_source = ?",
            placeholders
        );

        // Append torrent_source as the last bind value so we can use params_from_iter.
        let mut all_params: Vec<String> = shortcodes.clone();
        all_params.push(col.id.to_string());

        let conn = db.lock().map_err(|e| e.to_string())?;
        let rows = conn
            .execute(&sql, rusqlite::params_from_iter(all_params.iter()))
            .map_err(|e| e.to_string())?;

        log::info!(
            "scan_installed_games: {} of {} dirs matched in DB for {}",
            rows,
            shortcodes.len(),
            col.id
        );
        total += rows;
    }

    // Pass 2: detect downloaded-but-not-extracted game ZIPs → mark as installed + in_library.
    // All eXoDOS game ZIPs live at game_base/<title with year>.zip regardless of collection.
    // This mirrors LaunchBox behavior where games stay as ZIPs until first launch.
    //
    // IMPORTANT: Only match ZIPs to non-LP (base) collections.  LP collections (GLP, PLP, SLP)
    // may share English titles with eXoDOS games; including them in the HashMap would cause
    // title collisions where an EN ZIP incorrectly marks an LP game as installed.
    // LP games are only considered installed when their extracted directory exists (Pass 1).
    if game_base.is_dir() {
        // Build lookup: zip_stem → game_id, restricted to non-LP collections.
        let lp_sources: Vec<String> = COLLECTION_MAP
            .iter()
            .filter(|c| c.lang_dir.is_some())
            .map(|c| c.id.to_string())
            .collect();
        let lp_placeholders = lp_sources.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let zip_query = format!(
            "SELECT id, title, application_path FROM games \
             WHERE installed = 0 AND in_library = 0 \
             AND torrent_source NOT IN ({})",
            lp_placeholders
        );

        let name_to_id: std::collections::HashMap<String, i64> = {
            let conn = db.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare(&zip_query)
                .map_err(|e| e.to_string())?;
            // Collect eagerly so stmt and conn can be dropped before the HashMap build.
            let rows: Vec<(i64, String, Option<String>)> = stmt
                .query_map(rusqlite::params_from_iter(lp_sources.iter()), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            rows.into_iter()
                .map(|(id, title, app_path)| {
                    let name = app_path
                        .as_deref()
                        .and_then(game_name_from_app_path)
                        .unwrap_or(title);
                    (name, id)
                })
                .collect()
        };

        let zip_ids: Vec<i64> = match std::fs::read_dir(&game_base) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map_or(false, |ext| ext.eq_ignore_ascii_case("zip"))
                        // Skip zero-byte stubs and tiny torrent placeholders (<1 KB)
                        && e.metadata().map(|m| m.len() >= 1024).unwrap_or(false)
                })
                .filter_map(|e| {
                    let stem = e.path().file_stem()?.to_string_lossy().into_owned();
                    name_to_id.get(&stem).copied()
                })
                .collect(),
            Err(e) => {
                log::warn!(
                    "scan_installed_games: cannot scan {} for ZIPs: {}",
                    game_base.display(),
                    e
                );
                vec![]
            }
        };

        if !zip_ids.is_empty() {
            let placeholders = zip_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!(
                "UPDATE games SET installed = 1, in_library = 1 WHERE id IN ({})",
                placeholders
            );
            let conn = db.lock().map_err(|e| e.to_string())?;
            let rows = conn
                .execute(&sql, rusqlite::params_from_iter(zip_ids.iter()))
                .map_err(|e| e.to_string())?;
            log::info!(
                "scan_installed_games: {} games marked installed from ZIP scan ({} ZIPs found)",
                rows,
                zip_ids.len()
            );
            total += rows;
        }
    }

    Ok(total)
}

/// Re-scan the eXoDOS directory tree to detect games already downloaded to disk,
/// marking them as `installed` and `in_library`.  Returns the count of games updated.
#[tauri::command]
pub async fn scan_installed_games(
    db_state: State<'_, DbState>,
) -> Result<usize, String> {
    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    };
    let data_dir = data_dir.ok_or("data_dir not configured")?;
    scan_installed_games_with_db(&db_state.0, &data_dir)
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
    // Dev mode reads from the repo tree.
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("torrents").join(filename))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(dev_path);
    }

    // Production reads from the Tauri resource_dir (cached at startup).
    if let Some(res_dir) = RESOURCE_DIR.get() {
        let prod_path = res_dir.join("torrents").join(filename);
        if prod_path.exists() {
            return Ok(prod_path);
        }
    }

    Err(format!(
        "Bundled torrent '{}' not found (dev path: {})",
        filename,
        dev_path.display()
    ))
}
