mod commands;
pub mod db;
pub mod import;
pub mod models;
pub mod torrent;

// Re-export utilities used by the generate_db binary
pub use commands::game_name_from_app_path;
pub use commands::{CollectionDef, COLLECTION_MAP};

use std::path::Path;
use std::sync::Mutex;

use tauri::Manager;
use tokio::sync::RwLock;

use commands::{
    bundled_metadata_dir, check_for_updates, download_game, factory_reset, get_available_collections,
    get_config, get_default_data_dir, get_download_progress, get_game, get_game_variants, get_games,
    get_genres, get_installed_games, get_languages, uninstall_game, get_setup_status,
    get_thumbnail_dir, get_torrent_info, import_games, init_download_manager, launch_game,
    set_config, setup_from_local, setup_import, setup_start, DbState, TorrentState,
};

/// Copy the bundled pre-built DB to the target path.
fn install_bundled_db(target: &Path) -> Result<(), String> {
    let metadata_dir = bundled_metadata_dir()?;

    let bundled_db = metadata_dir.join("exodian.db");
    let bundled_db_gz = metadata_dir.join("exodian.db.gz");

    // Clean up any stale WAL/SHM files
    let _ = std::fs::remove_file(target.with_extension("db-wal"));
    let _ = std::fs::remove_file(target.with_extension("db-shm"));

    if bundled_db.exists() {
        std::fs::copy(&bundled_db, target)
            .map_err(|e| format!("Failed to copy bundled DB: {}", e))?;
        log::info!("Installed bundled DB from {}", bundled_db.display());
    } else if bundled_db_gz.exists() {
        use flate2::read::GzDecoder;
        let file = std::fs::File::open(&bundled_db_gz)
            .map_err(|e| e.to_string())?;
        let mut decoder = GzDecoder::new(file);
        let mut out = std::fs::File::create(target)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut decoder, &mut out)
            .map_err(|e| e.to_string())?;
        log::info!("Installed bundled DB from {}", bundled_db_gz.display());
    } else {
        return Err(format!(
            "No bundled database found in {}",
            metadata_dir.display()
        ));
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("exodian.db");

            log::info!("Database path: {}", db_path.display());

            // If no DB exists, install the bundled one
            if !db_path.exists() {
                if let Err(e) = install_bundled_db(&db_path) {
                    log::error!("Failed to install bundled DB: {}", e);
                }
            }

            // Open DB, reinstall if corrupt
            let conn = match db::open(&db_path).and_then(|c| { db::init(&c)?; Ok(c) }) {
                Ok(c) => {
                    // Check if DB has games; if empty (factory reset), reinstall
                    let count: i64 = c
                        .query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))
                        .unwrap_or(0);
                    if count == 0 {
                        drop(c);
                        if let Err(e) = install_bundled_db(&db_path) {
                            log::error!("Failed to install bundled DB: {}", e);
                        }
                        db::open(&db_path).expect("failed to open installed DB")
                    } else {
                        c
                    }
                }
                Err(e) => {
                    log::warn!("Database unreadable ({}), reinstalling", e);
                    let _ = std::fs::remove_file(&db_path);
                    if let Err(e) = install_bundled_db(&db_path) {
                        log::error!("Failed to install bundled DB: {}", e);
                    }
                    let c = db::open(&db_path).expect("failed to create database");
                    db::init(&c).expect("failed to initialize schema");
                    c
                }
            };

            // Sync has_thumbnail flags from the thumbnail directory on disk.
            if let Ok(metadata_dir) = commands::bundled_metadata_dir() {
                if let Some(thumb_dir) = metadata_dir.parent().map(|p| p.join("thumbnails").join("eXoDOS")) {
                    if thumb_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&thumb_dir) {
                            let shortcodes: Vec<String> = entries
                                .flatten()
                                .filter_map(|e| {
                                    let p = e.path();
                                    if p.extension().and_then(|s| s.to_str()) == Some("jpg") {
                                        p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if !shortcodes.is_empty() {
                                let _ = conn.execute_batch("UPDATE games SET has_thumbnail = 0");
                                for sc in &shortcodes {
                                    let _ = conn.execute(
                                        "UPDATE games SET has_thumbnail = 1 WHERE shortcode = ?1",
                                        rusqlite::params![sc],
                                    );
                                }
                                log::info!("Synced has_thumbnail for {} shortcodes", shortcodes.len());
                            }
                        }
                    }
                }
            }

            app.manage(DbState(Mutex::new(conn)));
            app.manage(TorrentState(RwLock::new(std::collections::HashMap::new())));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_games,
            get_game,
            get_installed_games,
            get_game_variants,
            get_genres,
            get_languages,
            import_games,
            launch_game,
            get_config,
            set_config,
            get_torrent_info,
            setup_start,
            get_setup_status,
            setup_import,
            setup_from_local,
            get_default_data_dir,
            get_thumbnail_dir,
            get_available_collections,
            init_download_manager,
            factory_reset,
            download_game,
            uninstall_game,
            get_download_progress,
            check_for_updates,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
