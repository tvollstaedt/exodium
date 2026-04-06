use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::db;
use crate::db::queries;
use crate::models::Game;
use crate::torrent::manager::DownloadProgress;

use super::TorrentState;

/// Map torrent_source to the language subdirectory under eXo/eXoDOS/.
fn lang_dir_for_source(source: &str) -> Option<&'static str> {
    match source {
        "eXoDOS_GLP" => Some("!german"),
        "eXoDOS_SLP" => Some("!spanish"),
        "eXoDOS_PLP" => Some("!polish"),
        _ => None,
    }
}

/// Resolve the data directory for a collection.
fn collection_data_dir(data_dir: &str, source: &str) -> PathBuf {
    if source == "eXoDOS" {
        std::path::Path::new(data_dir).to_path_buf()
    } else {
        std::path::Path::new(data_dir).join(source)
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
pub fn get_games(
    state: State<DbState>,
    page: Option<usize>,
    per_page: Option<usize>,
    query: Option<String>,
    language: Option<String>,
    genre: Option<String>,
    sort_by: Option<String>,
    collection: Option<String>,
) -> Result<GameList, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let page = page.unwrap_or(1);
    let per_page = per_page.unwrap_or(50);
    let query = query.unwrap_or_default();
    let language = language.unwrap_or_default();
    let genre = genre.unwrap_or_default();
    let sort_by = sort_by.unwrap_or_default();
    let collection = collection.unwrap_or_default();

    let f = queries::GameFilter {
        query: &query,
        language: &language,
        genre: &genre,
        sort_by: &sort_by,
        collection: &collection,
    };

    let (games, total) = if collection.is_empty() {
        queries::fetch_games_merged(&conn, page, per_page, &f).map_err(|e| e.to_string())?
    } else {
        let total = queries::count_games_filtered(&conn, &f).map_err(|e| e.to_string())?;
        let games = queries::fetch_games_filtered(&conn, page, per_page, &f).map_err(|e| e.to_string())?;
        (games, total)
    };

    Ok(GameList { games, total })
}

#[tauri::command]
pub fn get_genres(state: State<DbState>) -> Result<Vec<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::get_genres(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_game_variants(
    state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    shortcode: String,
) -> Result<Vec<Game>, String> {
    let mut variants = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        queries::fetch_game_variants(&conn, &shortcode).map_err(|e| e.to_string())?
    };

    // Adjust download sizes: if EN GameData already exists, subtract it from LP variant sizes
    let guard = torrent_state.0.read().await;
    if let Some(main_mgr) = guard.get("eXoDOS") {
        let en_variant = variants.iter().find(|v| v.language == "EN");
        let en_game_name = en_variant
            .and_then(|v| v.application_path.as_deref())
            .and_then(crate::commands::setup::game_name_from_app_path);

        if let Some(name) = en_game_name {
            let (_, gamedata_entry) = main_mgr.index().find_game_files(&name);

            if let Some(gd) = gamedata_entry {
                let en_gamedata_size = gd.size as i64;
                let en_gd_on_disk = main_mgr.file_output_path(gd.index)
                    .map(|p| p.exists())
                    .unwrap_or(false);
                let en_installed = en_variant.map(|v| v.installed).unwrap_or(false);

                if en_gd_on_disk || en_installed {
                    for variant in &mut variants {
                        if variant.language != "EN" {
                            if let Some(ref mut size) = variant.download_size {
                                *size = (*size - en_gamedata_size).max(0);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(variants)
}

#[tauri::command]
pub fn get_installed_games(state: State<DbState>) -> Result<Vec<Game>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::fetch_installed_games(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_languages(state: State<DbState>) -> Result<Vec<String>, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    queries::get_languages(&conn).map_err(|e| e.to_string())
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
        crate::import::import_from_zip(path, &conn).map_err(|e| e.to_string())
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

    let guard = torrent_state.0.read().await;
    let manager = guard
        .get(source)
        .ok_or_else(|| format!("Download manager for '{}' not initialized.", source))?;

    let mut files = vec![game_idx];
    if let Some(gd_idx) = game.gamedata_torrent_index {
        files.push(gd_idx as usize);
    }

    // For language pack games, also download EN GameData (videos, animations)
    if source != "eXoDOS" {
        if let Some(main_mgr) = guard.get("eXoDOS") {
            // Find the EN GameData ZIP by game name
            if let Some(app_path) = &game.application_path {
                let game_name = crate::commands::setup::game_name_from_app_path(app_path);
                if let Some(name) = game_name {
                    let (_, gamedata) = main_mgr.index().find_game_files(&name);
                    if let Some(gd) = gamedata {
                        let gd_path = main_mgr.file_output_path(gd.index);
                        let already_exists = gd_path.as_ref().map(|p| p.exists()).unwrap_or(false);
                        if !already_exists {
                            let _ = main_mgr.download_files(vec![gd.index]).await;
                            log::info!("Also downloading EN GameData for {}", name);
                        }
                    }
                }
            }
        }
    }

    // Also queue !DOSmetadata.zip (DOSBox configs) if not already extracted
    if let Some(main_mgr) = guard.get("eXoDOS") {
        let dosbox_dir = main_mgr.torrent_root().join("eXo/eXoDOS/!dos");
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

    let guard = torrent_state.0.read().await;
    let manager = match guard.get(&source) {
        Some(m) => m,
        None => return Ok(None),
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
    if let Some(main_mgr) = guard.get("eXoDOS") {
        if let Some(dosbox_meta) = main_mgr.index().find_dosbox_metadata_zip() {
            if main_mgr.is_file_complete(dosbox_meta.index).await {
                if let Some(zip_path) = main_mgr.file_output_path(dosbox_meta.index) {
                    let lock = zip_path.with_extension("extracted");
                    if zip_path.exists() && !lock.exists() {
                        let torrent_root = main_mgr.torrent_root();
                        tauri::async_runtime::spawn_blocking(move || {
                            if let Ok(file) = std::fs::File::open(&zip_path) {
                                if let Ok(mut archive) = zip::ZipArchive::new(file) {
                                    let _ = archive.extract(&torrent_root);
                                    let _ = std::fs::write(&lock, "");
                                    log::info!("Extracted DOSBox configs");
                                }
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
                    // ZIP reported finished but not on disk.
                    // This can happen when librqbit has piece data but hasn't assembled the file.
                    // Show error so user can retry via uninstall + re-download.
                    log::warn!(
                        "Download reports 100% but ZIP missing: {}. librqbit may need a restart.",
                        zip_path.display()
                    );
                    if let Some(ref mut p) = progress {
                        p.error = Some("Download incomplete — right-click to uninstall and retry".to_string());
                    }
                }
            }
        }
    }

    Ok(progress)
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
    let torrent_root = collection_data_dir(&data_dir, source).join("eXoDOS");

    // Get game name from bat filename for ZIP deletion
    let game_name = game.application_path.as_deref()
        .and_then(crate::commands::setup::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());

    // Determine game directory
    // For EN: eXo/eXoDOS/<shortcode>/
    // For DE: eXo/eXoDOS/!german/<shortcode>/
    let mut game_dir_candidates = vec![torrent_root.join(format!("eXo/eXoDOS/{}", shortcode))];
    for ld in LANG_DIRS {
        game_dir_candidates.push(torrent_root.join(format!("eXo/eXoDOS/{}/{}", ld, shortcode)));
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
                let save_dir = torrent_root.join(format!("eXo/eXoDOS/!save/{}", shortcode));
                if save_dir.exists() {
                    let _ = std::fs::remove_dir_all(&save_dir);
                }
                // Rename is the fastest way to "back up" — atomic move
                if let Err(_) = std::fs::rename(dir, &save_dir) {
                    // Rename failed (cross-device?), fall back to copy + delete
                    let _ = copy_dir_recursive(dir, &save_dir);
                    let _ = std::fs::remove_dir_all(dir);
                }
                log::info!("Backed up saves to {}", save_dir.display());
            }
        }

        let mut zip_paths = vec![torrent_root.join(format!("eXo/eXoDOS/{}.zip", game_name))];
        for ld in LANG_DIRS {
            zip_paths.push(torrent_root.join(format!("eXo/eXoDOS/{}/{}.zip", ld, game_name)));
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
/// For LP games, `lp_info` provides the shortcode and language dir. The EN autoexec's
/// mount paths are redirected to the LP location. If the redirected path doesn't exist
/// (different directory structure), falls back to a generated autoexec.
fn patch_dosbox_conf(
    conf_path: &std::path::Path,
    working_dir: &std::path::Path,
    lp_info: Option<(&str, &str, &std::path::Path)>, // (shortcode, lang_dir, lp_game_dir)
) -> Result<PathBuf, String> {
    let content = std::fs::read_to_string(conf_path)
        .map_err(|e| format!("Failed to read {}: {}", conf_path.display(), e))?;

    let abs_prefix = format!("{}/", working_dir.to_string_lossy());

    let patched = if let Some((shortcode, lang_dir, game_dir)) = lp_info {
        // Strategy 1: Redirect EN mount paths to LP location (preserves CD mounts, etc.)
        let en_path = format!("eXoDOS\\{}", shortcode);
        let lp_path = format!("eXoDOS\\{}\\{}", lang_dir, shortcode);
        let redirected = content.replace(&en_path, &lp_path);

        // Check if the redirected mount path exists AND internal dirs match
        let redirected_dir = working_dir.join(format!("eXoDOS/{}/{}", lang_dir, shortcode));
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
            redirected
                .replace(".\\", &abs_prefix)
                .replace('\\', "/")
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

    let patched_path = working_dir.join(".exodian_launch.conf");
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

    // Strategy 2: Look for a .com file (more likely to be a DOS game than .exe)
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

    None
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

/// Launch a downloaded game via DOSBox Staging.
#[tauri::command]
pub fn launch_game(db_state: State<DbState>, id: i64) -> Result<String, String> {
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

    // Each collection has its own subdirectory (except eXoDOS which is at the root)
    // eXoDOS: <data_dir>/eXoDOS/eXo/...
    // eXoDOS_GLP: <data_dir>/eXoDOS_GLP/eXoDOS/eXo/...
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let main_torrent_root = collection_data_dir(&data_dir, "eXoDOS").join("eXoDOS");
    let torrent_root = collection_data_dir(&data_dir, source).join("eXoDOS");
    let mut working_dir = torrent_root.join("eXo");
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

    // The config might be under a language-specific !dos/ subdirectory
    if !game_conf.exists() {
        if let Some(shortcode) = dosbox_conf
            .strip_suffix("/dosbox.conf")
            .and_then(|p| p.rsplit('/').next())
        {
            let roots = if source != "eXoDOS" {
                vec![&torrent_root, &main_torrent_root]
            } else {
                vec![&torrent_root]
            };
            'outer: for root in &roots {
                for lang_dir in LANG_DIRS {
                    let alt = root.join(format!(
                        "eXo/eXoDOS/!dos/{}/{}/dosbox.conf", lang_dir, shortcode
                    ));
                    if alt.exists() {
                        game_conf = alt;
                        working_dir = root.join("eXo");
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

    // For LP games, determine the language dir and game path for config patching
    let shortcode = game.shortcode.as_deref().unwrap_or("");
    let lp_info = lang_dir_for_source(source).map(|ld| {
        let dir = torrent_root.join(format!("eXo/eXoDOS/{}/{}", ld, shortcode));
        (shortcode, ld, dir)
    });

    let patched_conf = patch_dosbox_conf(
        &game_conf,
        &working_dir,
        lp_info.as_ref().map(|(sc, ld, dir)| (*sc, *ld, dir.as_path())),
    )?;

    log::info!(
        "Launching: {} with config {} (patched: {})",
        game.title,
        game_conf.display(),
        patched_conf.display()
    );

    let mut cmd = Command::new("dosbox-staging");
    cmd.current_dir(&working_dir)
        .arg("-conf")
        .arg(&patched_conf);

    if options_conf.exists() {
        cmd.arg("-conf").arg(&options_conf);
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to launch DOSBox Staging: {}", e))?;

    Ok(format!("Launched: {}", game.title))
}
