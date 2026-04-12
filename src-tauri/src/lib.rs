mod commands;
pub mod db;
pub mod import;
pub mod models;
pub mod torrent;

// Re-export utilities used by the generate_db binary and integration tests
pub use commands::game_name_from_app_path;
pub use commands::{collection_data_dir, CollectionDef, COLLECTION_MAP};

use std::path::Path;
use std::sync::Mutex;

use tauri::Manager;
use tokio::sync::RwLock;

use commands::{
    bundled_metadata_dir, cancel_content_pack_install, cancel_download, check_for_updates,
    download_game, factory_reset, get_available_collections, get_config,
    get_content_pack_progress, get_default_data_dir, get_download_progress, get_game,
    get_poster_dir, get_preview_dir,
    get_game_variants, get_games, get_genres, get_installed_games, get_section_keys,
    get_setup_status, get_thumbnail_dir, get_torrent_info, init_download_manager,
    init_resource_dir, install_content_pack, launch_game, list_content_packs,
    scan_installed_games, set_config, setup_from_local, setup_import, setup_start,
    toggle_favorite, uninstall_content_pack, uninstall_game, validate_exodos_dir,
    ContentPackState, DbState, TorrentState,
};

/// Copy the bundled pre-built DB to the target path.
pub fn install_bundled_db(target: &Path) -> Result<(), String> {
    let metadata_dir = bundled_metadata_dir()?;

    let bundled_db = metadata_dir.join("exodium.db");
    let bundled_db_gz = metadata_dir.join("exodium.db.gz");

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
    // Default to info-level so our diagnostic log::info! / log::warn! messages
    // appear when users launch from a terminal. Override with RUST_LOG=debug for more.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Cache the resource_dir BEFORE any code tries to read bundled metadata,
            // torrents, or shaders — the sync helpers in setup.rs rely on this.
            if let Ok(res_dir) = app.path().resource_dir() {
                init_resource_dir(res_dir);
            } else {
                log::warn!("resource_dir() unavailable; bundled assets may not be found");
            }

            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("exodium.db");

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
                        let c = db::open(&db_path).expect("failed to open installed DB");
                        db::init(&c).expect("failed to run migrations on bundled DB");
                        c
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

            // Clean up stale content-pack download artifacts from interrupted installs.
            if let Ok(Some(user_data_dir)) = db::queries::get_config(&conn, "data_dir") {
                commands::content_packs::cleanup_stale_downloads(std::path::Path::new(&user_data_dir));
            }

            app.manage(DbState(Mutex::new(conn)));
            app.manage(TorrentState(RwLock::new(std::collections::HashMap::new())));
            app.manage(ContentPackState::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_games,
            get_game,
            get_installed_games,
            get_game_variants,
            get_genres,
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
            cancel_download,
            uninstall_game,
            get_download_progress,
            check_for_updates,
            toggle_favorite,
            get_section_keys,
            validate_exodos_dir,
            scan_installed_games,
            list_content_packs,
            install_content_pack,
            uninstall_content_pack,
            get_content_pack_progress,
            cancel_content_pack_install,
            get_preview_dir,
            get_poster_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
