use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use tauri::AppHandle;

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::db;
use crate::db::queries;
use crate::models::Game;
use crate::torrent::manager::DownloadProgress;

use super::TorrentState;

/// Resolve the data directory for a collection.
/// All collections share the same data directory (overlay model — no collection subdirectories).
pub fn collection_data_dir(data_dir: &str, _source: &str) -> PathBuf {
    std::path::Path::new(data_dir).to_path_buf()
}

/// Get the inner folder name for a collection (the folder the torrent creates).
fn collection_inner_folder(source: &str) -> &'static str {
    crate::commands::setup::collection_def(source)
        .map(|c| c.inner_folder)
        .unwrap_or("eXoDOS")
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

/// Language subdirectories used in the eXoDOS file structure.
const LANG_DIRS: &[&str] = &["!german", "!polish", "!czech", "!slovak", "!spanish"];

pub struct DbState(pub Mutex<Connection>);

#[derive(Debug, Clone, Serialize)]
pub struct GameList {
    pub games: Vec<Game>,
    pub total: usize,
}

#[tauri::command]
pub fn get_games(
    state: State<DbState>,
    page: Option<usize>,
    per_page: Option<usize>,
    query: Option<String>,
    genre: Option<String>,
    sort_by: Option<String>,
    collection: Option<String>,
    favorites_only: Option<bool>,
) -> Result<GameList, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let page = page.unwrap_or(1);
    let per_page = per_page.unwrap_or(50).min(500);
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
    };

    let total = queries::count_games_filtered(&conn, &f).map_err(|e| e.to_string())?;
    let games = queries::fetch_games_filtered(&conn, page, per_page, &f).map_err(|e| e.to_string())?;

    Ok(GameList { games, total })
}

#[tauri::command]
pub fn get_genres(state: State<DbState>, collection: Option<String>) -> Result<Vec<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let collection = collection.unwrap_or_default();
    queries::get_genres(&conn, &collection).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_section_keys(
    state: State<DbState>,
    sort_by: Option<String>,
    query: Option<String>,
    genre: Option<String>,
    collection: Option<String>,
    favorites_only: Option<bool>,
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
    };
    queries::get_section_keys(&conn, &f).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_game_variants(state: State<'_, DbState>, shortcode: String) -> Result<Vec<Game>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_game_variants(&conn, &shortcode).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_installed_games(state: State<DbState>) -> Result<Vec<Game>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_installed_games(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_favorite(state: State<DbState>, id: i64) -> Result<bool, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::toggle_favorite(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_game(state: State<DbState>, id: i64) -> Result<Option<Game>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_game_by_id(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn import_games(state: State<'_, DbState>, zip_path: String) -> Result<usize, String> {
    let db_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.path()
            .map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    let zip = zip_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        let path = std::path::Path::new(&zip);
        crate::import::import_from_zip(path, &conn, "!dos").map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_config(state: State<DbState>, key: String) -> Result<Option<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::get_config(&conn, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_config(state: State<DbState>, key: String, value: String) -> Result<(), String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::set_config(&conn, &key, &value).map_err(|e| e.to_string())
}

/// Queue a game for download via torrent.
#[tauri::command]
pub async fn download_game(
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

    // Mark as in library immediately
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_in_library(&conn, id).map_err(|e| e.to_string())?;
    }

    let game_idx = game
        .game_torrent_index
        .ok_or_else(|| format!("{} has no torrent index — cannot download", game.title))?
        as usize;

    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");

    // Clone Arc references and immediately drop the guard so we don't hold it across awaits.
    let (manager, main_mgr_opt) = {
        let guard = torrent_state.0.read().await;
        let manager = guard
            .get(source)
            .cloned()
            .ok_or_else(|| format!("Download manager for '{}' not initialized.", source))?;
        let main_mgr = guard.get("eXoDOS").cloned();
        (manager, main_mgr)
    };

    let mut files = vec![game_idx];
    if let Some(gd_idx) = game.gamedata_torrent_index {
        files.push(gd_idx as usize);
    }

    // Also queue !DOSmetadata.zip (DOSBox configs) if not already extracted
    if let Some(ref main_mgr) = main_mgr_opt {
        let main_prefix = collection_game_prefix("eXoDOS");
        let main_segment = crate::commands::setup::collection_def("eXoDOS")
            .map(|c| c.shortcode_segment)
            .unwrap_or("!dos");
        let dosbox_dir = main_mgr.torrent_root().join(format!("{}/{}", main_prefix, main_segment));
        if !dosbox_dir.exists() {
            if let Some(dm) = main_mgr.index().find_dosbox_metadata_zip() {
                let _ = main_mgr.download_files(vec![dm.index]).await;
                log::info!("Also downloading !DOSmetadata.zip (DOSBox configs)");
            }
        }
    }

    manager
        .download_files(files)
        .await
        .map_err(|e| format!("Failed to queue download: {}", e))?;

    Ok(format!("Downloading: {}", game.title))
}

/// Get download progress for a game. If complete, extract and mark installed.
#[tauri::command]
pub async fn get_download_progress(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<Option<DownloadProgress>, String> {
    let (game_idx, title, already_installed, source) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        match game.game_torrent_index {
            Some(idx) => (
                idx as usize,
                game.title,
                game.installed,
                game.torrent_source.unwrap_or_else(|| "eXoDOS".to_string()),
            ),
            None => return Ok(None),
        }
    };

    // Clone Arc references and drop the guard immediately — the guard must not be held
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

    // Extract !DOSmetadata.zip if it just finished downloading (check main eXoDOS manager)
    if let Some(ref main_mgr) = main_mgr_opt {
        if let Some(dosbox_meta) = main_mgr.index().find_dosbox_metadata_zip() {
            if main_mgr.is_file_complete(dosbox_meta.index).await {
                if let Some(zip_path) = main_mgr.file_output_path(dosbox_meta.index) {
                    let lock = zip_path.with_extension("extracted");
                    if zip_path.exists() && !lock.exists() {
                        let torrent_root = main_mgr.torrent_root();
                        tauri::async_runtime::spawn_blocking(move || {
                            let result = (|| -> Result<(), String> {
                                let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
                                let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
                                archive.extract(&torrent_root).map_err(|e| e.to_string())?;
                                std::fs::write(&lock, "").map_err(|e| e.to_string())?;
                                Ok(())
                            })();
                            match result {
                                Ok(()) => log::info!("Extracted DOSBox configs to {}", torrent_root.display()),
                                Err(e) => log::error!("Failed to extract DOSBox configs: {}", e),
                            }
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
                    let lock_path = zip_path.with_extension("extracting");

                    // Clean up stale lock files (e.g., from crashed/interrupted extraction)
                    if lock_path.exists() {
                        if let Ok(age) = std::fs::metadata(&lock_path)
                            .and_then(|m| m.modified())
                            .and_then(|t| t.elapsed().map_err(|e| std::io::Error::other(e)))
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

                        tauri::async_runtime::spawn_blocking(move || {
                            log::info!("Extracting {} from {}", title, zip_path.display());
                            if let Err(e) = extract_game_zip(&zip_path, &extract_dir) {
                                log::error!("Failed to extract {}: {}", title, e);
                                let _ = std::fs::remove_file(&lock_path);
                                return;
                            }
                            match db::open(&db_path) {
                                Ok(conn) => {
                                    if let Err(e) = queries::set_game_installed(&conn, game_id, true) {
                                        log::error!("Failed to mark {} installed: {}", title, e);
                                    } else {
                                        log::info!("Installed: {}", title);
                                    }
                                }
                                Err(e) => log::error!("Failed to open DB for install update: {}", e),
                            }
                            let _ = std::fs::remove_file(&lock_path);
                        });
                    }
                } else {
                    // ZIP not on disk despite torrent reporting 100%.
                    // Common cause: pieces covering this file were received as a side effect of
                    // downloading a neighboring file, but the file was never selected so librqbit
                    // never assembled/wrote it. Re-selecting the file forces assembly.
                    log::warn!(
                        "Download reports 100% but ZIP missing: {}. Re-requesting file assembly.",
                        zip_path.display()
                    );
                    // Only spawn the re-trigger once — if the file is already selected (i.e., a
                    // previous poll already spawned a re-request), skip spawning again to avoid
                    // a new task every second while librqbit assembles the file.
                    if !manager.is_file_selected(game_idx).await {
                        let mgr = manager.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = mgr.download_files(vec![game_idx]).await;
                        });
                    }
                    // Show as still in-progress so the frontend keeps polling until the ZIP
                    // appears and extraction can proceed normally.
                    if let Some(ref mut p) = progress {
                        p.finished = false;
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
        (
            game.game_torrent_index.map(|i| i as usize),
            game.gamedata_torrent_index.map(|i| i as usize),
            game.torrent_source.unwrap_or_else(|| "eXoDOS".to_string()),
        )
    };

    // Deselect from torrent first — if this fails silently, we still want to clear the DB flag.
    {
        let guard = torrent_state.0.read().await;
        if let Some(manager) = guard.get(&source) {
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
        return Err(format!("{} is not installed", game.title));
    }

    let shortcode = game.shortcode.as_deref()
        .ok_or("Game has no shortcode")?
        .to_string();

    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let inner_folder = collection_inner_folder(source);
    let game_prefix = collection_game_prefix(source);
    let torrent_root = collection_data_dir(&data_dir, source).join(inner_folder);

    // Get game name from bat filename for ZIP deletion
    let game_name = game.application_path.as_deref()
        .and_then(crate::commands::setup::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());

    // Determine game directory
    // For EN:  <game_prefix>/<shortcode>/
    // For LP:  <game_prefix>/<lang_dir>/<shortcode>/
    let mut game_dir_candidates = vec![torrent_root.join(format!("{}/{}", game_prefix, shortcode))];
    for ld in LANG_DIRS {
        game_dir_candidates.push(torrent_root.join(format!("{}/{}/{}", game_prefix, ld, shortcode)));
    }

    let game_dir: Option<PathBuf> = game_dir_candidates.into_iter().find(|d| d.exists());

    let db_path = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.path().map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    tauri::async_runtime::spawn_blocking(move || {
        if let Some(ref dir) = game_dir {
            if dir.exists() {
                // Back up the entire game directory (preserves saves, configs, etc.)
                let save_dir = torrent_root.join(format!("{}/!save/{}", game_prefix, shortcode));
                if save_dir.exists() {
                    let _ = std::fs::remove_dir_all(&save_dir);
                }
                // Rename is the fastest way to "back up" — atomic move
                if let Err(e) = std::fs::rename(dir, &save_dir) {
                    // Rename failed (cross-device?), fall back to copy + delete
                    log::warn!("Rename to save dir failed ({}), falling back to copy", e);
                    if let Err(e) = copy_dir_recursive(dir, &save_dir) {
                        log::error!("Failed to back up game directory '{}': {}", dir.display(), e);
                        // Don't delete the source if backup failed
                    } else {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                }
                log::info!("Backed up saves to {}", save_dir.display());
            }
        }

        let mut zip_paths = vec![torrent_root.join(format!("{}/{}.zip", game_prefix, game_name))];
        for ld in LANG_DIRS {
            zip_paths.push(torrent_root.join(format!("{}/{}/{}.zip", game_prefix, ld, game_name)));
        }
        for zip in &zip_paths {
            let _ = std::fs::remove_file(zip);
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
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(format!("Uninstalled: {}", game.title))
}

/// Patch a DOSBox config file: convert Windows-style relative paths to absolute Linux paths.
/// The eXoDOS configs use `.\eXoDOS\game\` which doesn't work on Linux.
///
/// For LP games, `lp_info` provides the shortcode, language dir, game_folder (the second
/// component of game_prefix, e.g. "eXoDOS"), and the resolved LP game directory path.
/// The EN autoexec's mount paths are redirected to the LP location.  If the redirected
/// path doesn't exist (different directory structure), falls back to a generated autoexec.
fn patch_dosbox_conf(
    conf_path: &std::path::Path,
    working_dir: &std::path::Path,
    lp_info: Option<(&str, &str, &str, &std::path::Path)>, // (shortcode, lang_dir, game_folder, lp_game_dir)
) -> Result<PathBuf, String> {
    let content = std::fs::read_to_string(conf_path)
        .map_err(|e| format!("Failed to read {}: {}", conf_path.display(), e))?;

    let abs_prefix = format!("{}/", working_dir.to_string_lossy());

    let patched = if let Some((shortcode, lang_dir, game_folder, game_dir)) = lp_info {
        // Strategy 1: Redirect EN mount paths to LP location (preserves CD mounts, etc.)
        let en_path = format!("{}\\{}", game_folder, shortcode);
        let lp_path = format!("{}\\{}\\{}", game_folder, lang_dir, shortcode);
        let redirected = content.replace(&en_path, &lp_path);

        // Check if the redirected mount path exists AND internal dirs match
        let redirected_dir = working_dir.join(format!("{}/{}/{}", game_folder, lang_dir, shortcode));
        let autoexec_compatible = redirected_dir.exists() && {
            // Verify that any `cd` targets in the autoexec exist in the redirected dir
            let autoexec = content.split("[autoexec]").nth(1).unwrap_or("");
            autoexec.lines().all(|line| {
                let trimmed = line.trim().to_lowercase();
                if let Some(dir) = trimmed.strip_prefix("cd ").or_else(|| trimmed.strip_prefix("cd\\")) {
                    let dir = dir.trim();
                    dir.is_empty() || dir == "\\" || dir == "/" || redirected_dir.join(dir).exists()
                } else {
                    true
                }
            })
        };
        if autoexec_compatible {
            log::info!("LP launch: using redirected EN config for {}", shortcode);
            let mut result = redirected
                .replace(".\\", &abs_prefix)
                .replace('\\', "/");

            // If autoexec has no actual launch command (e.g., all commented out with #),
            // append one found by inspecting the LP game directory.
            if !autoexec_has_launch_cmd(&result) {
                log::info!("LP launch: autoexec has no launch cmd, appending find_lp_launch for {}", shortcode);
                if let Some((subdir, cmd)) = find_lp_launch(game_dir) {
                    // Strip any trailing `exit` so our appended commands aren't skipped.
                    let trimmed = result.trim_end();
                    if trimmed.to_ascii_lowercase().ends_with("exit") {
                        result.truncate(trimmed.len() - "exit".len());
                        result.push('\n');
                    }
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
            // Strategy 2: Different directory structure — generate custom autoexec
            log::info!("LP launch: generating custom autoexec for {} (redirected path not found)", shortcode);
            let settings = content
                .split("[autoexec]")
                .next()
                .unwrap_or(&content);

            let mut patched = settings
                .replace(".\\", &abs_prefix)
                .replace('\\', "/");

            let game_dir_abs = game_dir.to_string_lossy();
            patched.push_str("[autoexec]\n");
            patched.push_str(&format!("@mount c \"{}\"\n", game_dir_abs));
            patched.push_str("c:\n");

            // Find the game subdirectory and launch command
            if let Some((subdir, cmd)) = find_lp_launch(game_dir) {
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
        // EN game: simple path replacement
        content
            .replace(".\\", &abs_prefix)
            .replace('\\', "/")
    };

    let patched_path = working_dir.join(".exodium_launch.conf");
    std::fs::write(&patched_path, &patched)
        .map_err(|e| format!("Failed to write patched config: {}", e))?;

    log::debug!("Patched config written to {}", patched_path.display());

    Ok(patched_path)
}

/// Find the launch command for an LP game by inspecting its directory.
/// Parses run.bat to extract the actual game executable, since run.bat itself
/// is a LaunchBox-specific menu script not suitable for DOSBox autoexec.
/// Returns (subdir, command) if found.
fn find_lp_launch(game_dir: &std::path::Path) -> Option<(String, String)> {
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

    // Strategy 4: Look for a .exe in subdirectories (skip utilities and installers)
    const SKIP_EXE_STEMS: &[&str] = &[
        "install", "setup", "uninst", "config", "cdtest", "showtext",
        // DOS/4GW and protected-mode extenders — not the game itself
        "rtm", "dos4gw", "dpmi", "cwsdpmi",
    ];
    for (subdir, dir) in search_dirs.iter().filter(|(s, _)| !s.is_empty()) {
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

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {}", dst.display(), e))?;
    for entry in walkdir::WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.path().is_dir() {
            if let Err(e) = std::fs::create_dir_all(&target) {
                log::warn!("Failed to create dir {}: {}", target.display(), e);
            }
        } else if let Err(e) = std::fs::copy(entry.path(), &target) {
            log::warn!("Failed to copy {} -> {}: {}", entry.path().display(), target.display(), e);
        }
    }
    Ok(())
}

/// Extract a game ZIP in place, then restore saves from !save/ if available.
fn extract_game_zip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    // Get the top-level directory name from the ZIP (the shortcode)
    let shortcode = archive.by_index(0).ok()
        .and_then(|e| e.name().split('/').next().map(|s| s.to_string()));

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
                let _ = copy_dir_recursive(save_dir, &game_dir);
                break;
            }
        }
    }

    Ok(())
}

/// Resolve the DOSBox Staging binary path.
/// Checks the bundled sidecar binary first, then falls back to the system PATH.
fn resolve_dosbox(app: &AppHandle) -> PathBuf {
    use tauri::Manager;
    let bin = if cfg!(windows) { "dosbox-staging.exe" } else { "dosbox-staging" };

    if let Ok(res_dir) = app.path().resource_dir() {
        // Production bundle: Tauri strips the triple suffix and places the binary here.
        let prod = res_dir.join(bin);
        if prod.exists() {
            return prod;
        }

        // Dev mode (pnpm tauri dev): resource_dir is src-tauri/; binary is in binaries/
        // named with the Rust target triple, e.g. dosbox-staging-aarch64-apple-darwin.
        let binaries_dir = res_dir.join("binaries");
        if let Ok(entries) = std::fs::read_dir(&binaries_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("dosbox-staging") {
                    log::info!("Using bundled DOSBox: {}", entry.path().display());
                    return entry.path();
                }
            }
        }
    }

    log::warn!("Bundled DOSBox not found, falling back to system PATH");
    PathBuf::from(bin)
}

/// Launch a downloaded game via DOSBox Staging.
#[tauri::command]
pub fn launch_game(app: AppHandle, db_state: State<DbState>, id: i64) -> Result<String, String> {
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;

    let game = queries::fetch_game_by_id(&conn, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Game with id {} not found", id))?;

    let data_dir = queries::get_config(&conn, "data_dir")
        .map_err(|e| e.to_string())?
        .ok_or("Data directory not configured. Run setup first.")?;

    if !game.installed {
        return Err(format!("{} is not installed. Download it first.", game.title));
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

    // Normalize Windows backslashes
    let dosbox_conf = dosbox_conf.replace('\\', "/");

    // Each collection has its own subdirectory (except eXoDOS which is at the root).
    // Layout:  <data_dir>/<inner_folder>/           — for eXoDOS
    //          <data_dir>/<col_id>/<inner_folder>/  — for sub-collections
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let main_inner = collection_inner_folder("eXoDOS");
    let src_inner = collection_inner_folder(source);
    let src_game_prefix = collection_game_prefix(source);
    let main_torrent_root = collection_data_dir(&data_dir, "eXoDOS").join(main_inner);
    let torrent_root = collection_data_dir(&data_dir, source).join(src_inner);
    // working_dir is the first path component of game_prefix (e.g. "eXo")
    let working_dir_name = src_game_prefix.split('/').next().unwrap_or("eXo");
    let mut working_dir = torrent_root.join(working_dir_name);
    let mut game_conf = torrent_root.join(&dosbox_conf);
    let options_conf = main_torrent_root.join("eXo/emulators/dosbox/options.conf");

    // For LP games, the dosbox_conf was inherited from the EN game.
    // The config lives in the main eXoDOS data dir, but game files are in the LP dir.
    // We use the EN config but redirect mount paths to the LP location via lp_redirect.
    if !game_conf.exists() && source != "eXoDOS" {
        let main_conf = main_torrent_root.join(&dosbox_conf);
        if main_conf.exists() {
            game_conf = main_conf;
            // Keep working_dir as LP torrent root — lp_redirect will fix mount paths
        }
    }

    // The config might be under a language-specific subdirectory
    if !game_conf.exists() {
        let main_game_prefix = collection_game_prefix("eXoDOS");
        let main_segment = crate::commands::setup::collection_def("eXoDOS")
            .map(|c| c.shortcode_segment)
            .unwrap_or("!dos");
        if let Some(shortcode) = dosbox_conf
            .strip_suffix("/dosbox.conf")
            .and_then(|p| p.rsplit('/').next())
            .filter(|s| !s.is_empty())
        {
            let roots = if source != "eXoDOS" {
                vec![&torrent_root, &main_torrent_root]
            } else {
                vec![&torrent_root]
            };
            'outer: for root in &roots {
                for lang_dir in LANG_DIRS {
                    let alt = root.join(format!(
                        "{}/{}/{}/{}/dosbox.conf",
                        main_game_prefix, main_segment, lang_dir, shortcode
                    ));
                    if alt.exists() {
                        game_conf = alt;
                        working_dir = root.join(working_dir_name);
                        break 'outer;
                    }
                }
            }
        }
    }

    if !game_conf.exists() {
        let msg = format!(
            "Game config not found: {}\nMake sure the game is fully downloaded and extracted.",
            game_conf.display()
        );
        log::error!("launch_game({}): {}", game.title, msg);
        return Err(msg);
    }

    if !working_dir.exists() {
        return Err(format!("Working directory not found: {}", working_dir.display()));
    }

    // For LP games, determine the language dir and game path for config patching.
    // The game_folder is the second component of game_prefix (e.g. "eXoDOS" from "eXo/eXoDOS").
    let shortcode = game.shortcode.as_deref().unwrap_or("");
    let game_folder = src_game_prefix.split('/').nth(1).unwrap_or("eXoDOS");
    let lp_info = collection_lang_dir(source).map(|ld| {
        let dir = torrent_root.join(format!("{}/{}/{}", src_game_prefix, ld, shortcode));
        (shortcode, ld, game_folder, dir)
    });

    let patched_conf = patch_dosbox_conf(
        &game_conf,
        &working_dir,
        lp_info.as_ref().map(|(sc, ld, gf, dir)| (*sc, *ld, *gf, dir.as_path())),
    )?;

    log::info!(
        "Launching: {} with config {} (patched: {})",
        game.title,
        game_conf.display(),
        patched_conf.display()
    );

    // Log variant for diagnostics; all variants currently map to DOSBox Staging.
    // ECE builds have no cross-platform release — Staging is used as best-effort.
    if let Some(ref variant) = game.dosbox_variant {
        if variant.starts_with("ece") {
            log::warn!(
                "Game '{}' uses ECE variant '{}' which has no cross-platform build. \
                 Using DOSBox Staging — gameplay accuracy may differ.",
                game.title, variant
            );
        }
    }

    let dosbox_bin = resolve_dosbox(&app);
    let mut cmd = Command::new(&dosbox_bin);
    cmd.current_dir(&working_dir)
        .arg("-conf")
        .arg(&patched_conf);

    if options_conf.exists() {
        cmd.arg("-conf").arg(&options_conf);
    }

    // macOS: the standalone binary extracted from the .app DMG lacks the bundle's
    // Contents/Resources/glshaders/, so DOSBox aborts when it can't find the mandatory
    // 'interpolation/bilinear' fallback shader. Override output to 'texture' (SDL
    // hardware renderer, no shaders required) via a last-wins conf fragment.
    // This conf is written once per data_dir and reused on subsequent launches.
    #[cfg(target_os = "macos")]
    let macos_override_conf = {
        let conf_path = std::path::Path::new(&data_dir).join("exodium_macos_dosbox.conf");
        std::fs::write(&conf_path, "[sdl]\noutput = texture\n")
            .map_err(|e| format!("Failed to write macOS override conf: {e}"))?;
        conf_path
    };
    #[cfg(target_os = "macos")]
    cmd.arg("-conf").arg(&macos_override_conf);

    // macOS dev builds: the binary extracted from the .app DMG has a bundle-anchored
    // code signature that becomes invalid without the surrounding bundle. Re-sign
    // ad-hoc if the signature is broken so macOS doesn't SIGKILL the process.
    #[cfg(all(target_os = "macos", debug_assertions))]
    {
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(&dosbox_bin)
            .output();
        let sig_ok = std::process::Command::new("codesign")
            .arg("-v")
            .arg(&dosbox_bin)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !sig_ok {
            log::warn!("DOSBox binary has invalid signature, re-signing ad-hoc: {}", dosbox_bin.display());
            let _ = std::process::Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(&dosbox_bin)
                .output();
        }
    }

    cmd.spawn().map_err(|e| {
        format!(
            "Failed to launch DOSBox Staging ({}): {}",
            dosbox_bin.display(), e
        )
    })?;

    Ok(format!("Launched: {}", game.title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn patch_dosbox_conf_converts_windows_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();

        let conf_content = "[sdl]\nfullscreen=false\n[autoexec]\n@mount c .\\eXoDOS\\SQ5\nc:\nSQ5.bat\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(&conf_path, working_dir, None).unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        // Backslash replaced with forward slash
        assert!(!patched.contains('\\'), "no backslashes should remain: {}", patched);
        // Relative .\ prefix replaced with absolute working dir
        let abs_prefix = format!("{}/", working_dir.to_string_lossy());
        assert!(patched.contains(&abs_prefix), "absolute path prefix expected: {}", patched);
    }

    #[test]
    fn patch_dosbox_conf_lp_mount_redirect() {
        let tmp = tempfile::tempdir().unwrap();
        let working_dir = tmp.path();

        // Create the LP redirect dir so the compatibility check passes
        let lp_dir = working_dir.join("eXoDOS/!german/SQ5");
        fs::create_dir_all(&lp_dir).unwrap();

        let conf_content = "[autoexec]\n@mount c eXoDOS\\SQ5\nc:\nSQ5.bat\nexit\n";
        let conf_path = write_conf(working_dir, "dosbox.conf", conf_content);

        let patched_path = patch_dosbox_conf(
            &conf_path,
            working_dir,
            Some(("SQ5", "!german", "eXoDOS", &lp_dir)),
        )
        .unwrap();
        let patched = fs::read_to_string(&patched_path).unwrap();

        assert!(
            patched.contains("eXoDOS/!german/SQ5"),
            "LP path redirect expected in patched config: {}",
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

        let result = find_lp_launch(game_dir);
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

        let result = find_lp_launch(game_dir);
        assert!(result.is_some(), ".com file should be found as fallback");
        let (_, cmd) = result.unwrap();
        assert!(cmd.to_lowercase().ends_with(".com"));
    }

    #[test]
    fn find_lp_launch_returns_none_for_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_lp_launch(tmp.path()).is_none());
    }
}
