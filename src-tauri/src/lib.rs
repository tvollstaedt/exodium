mod commands;
pub mod db;
pub mod import;
pub mod models;
mod support_files;
pub mod torrent;
pub mod vhd;

// Re-export utilities used by the generate_db binary and integration tests
pub use commands::game_name_from_app_path;
pub use commands::torrent_search_names;
pub use commands::paths::game_root;
pub use commands::{collection_base_id, CollectionDef, COLLECTION_MAP};

use std::path::Path;
use std::sync::Mutex;

use tauri::Manager;
use tokio::sync::RwLock;

use commands::{
    bundled_metadata_dir, cancel_content_pack_install, cancel_download,
    download_game, factory_reset, get_available_collections, get_config,
    get_content_pack_progress, get_default_data_dir, get_download_progress, get_game,
    data_dir_is_empty, get_game_metadata, get_game_settings, get_log_dir, get_poster_dir,
    get_preview_dir,
    get_game_variants, get_games, get_genres, get_installed_games, get_recently_played,
    get_section_keys, set_game_settings,
    create_playlist, delete_playlist, get_game_playlists, get_playlists, rename_playlist,
    set_playlist_membership,
    get_setup_status, get_torrent_info, init_download_manager,
    game_printing_unavailable, game_engine_info, init_log_dir, init_resource_dir, install_content_pack, launch_game,
    list_content_packs,
    get_transfer_stats, open_log_folder, open_manual, scan_installed_games, set_config, set_rate_limits,
    set_seeding_enabled, setup_from_local, setup_import,
    update_check_supported,
    reset_game_data, setup_start, toggle_favorite, uninstall_content_pack, uninstall_game,
    validate_exodos_dir,
    ContentPackState, DbState, TorrentState,
};

/// Raise the fd soft limit as far as allowed: librqbit keeps every torrent
/// file open (14,011 for eXoDOS), and the default 1024 fails the first add.
#[cfg(unix)]
fn raise_fd_limit() {
    unsafe {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) != 0 {
            return;
        }
        // Try the hard limit first; macOS reports RLIM_INFINITY but caps
        // setrlimit at kern.maxfilesperproc, so fall back through sane values.
        let mut candidates: Vec<libc::rlim_t> = vec![lim.rlim_max.min(1 << 20), 65536, 10240];

        // macOS: kern.maxfilesperproc scales with RAM (61440 on small
        // machines, below the 65536 ceiling), so query it.
        #[cfg(target_os = "macos")]
        {
            let mut maxfiles: libc::c_int = 0;
            let mut len = std::mem::size_of::<libc::c_int>();
            let name = b"kern.maxfilesperproc\0";
            if libc::sysctlbyname(
                name.as_ptr() as *const libc::c_char,
                &mut maxfiles as *mut _ as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            ) == 0
                && maxfiles > 0
            {
                candidates.push(maxfiles as libc::rlim_t);
            }
        }
        // Descending order so the early-exit below stays correct.
        candidates.sort_unstable_by(|a, b| b.cmp(a));
        candidates.dedup();

        // The eXoDOS torrent has 14 011 files and librqbit holds an fd for
        // each; anything below this leaves the main collection un-addable.
        const COMFORTABLE: libc::rlim_t = 15_000;

        for target in candidates {
            if target <= lim.rlim_cur {
                break; // already high enough
            }
            let new = libc::rlimit {
                rlim_cur: target,
                rlim_max: lim.rlim_max,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &new) == 0 {
                log::info!("Raised open-file limit: {} -> {}", lim.rlim_cur, target);
                if target < COMFORTABLE {
                    log::warn!(
                        "Open-file limit {} is below the ~{} needed for the main eXoDOS \
                         torrent - downloads from it will fail with 'error opening ... in \
                         read/write mode'. Raise kern.maxfilesperproc / ulimit -n.",
                        target, COMFORTABLE
                    );
                }
                return;
            }
        }
        if lim.rlim_cur < COMFORTABLE {
            log::warn!(
                "Could not raise open-file limit above {} - large torrents may fail to add",
                lim.rlim_cur
            );
        }
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

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

/// Grant the asset protocol the media subtrees of the data dir at runtime;
/// the static scope covers $RESOURCE/$APPDATA only, and the data dir itself
/// (often $HOME) would be far too wide.
pub fn allow_asset_dir(app: &tauri::AppHandle, data_dir: &Path) {
    use tauri::Manager;
    // The game root and the content packs under <data>/content.
    let dirs = [
        commands::paths::game_root(&data_dir.to_string_lossy()),
        data_dir.join("content"),
    ];
    for dir in dirs {
        if let Err(e) = app.asset_protocol_scope().allow_directory(&dir, true) {
            log::warn!("Failed to extend asset scope to {}: {}", dir.display(), e);
        }
    }

    // Bundled previews: outside $RESOURCE in `tauri dev`.
    let preview_roots = [
        commands::paths::RESOURCE_DIR.get().map(|d| d.join("previews")),
        // `tauri dev` resolves previews from the source tree - see
        // setup::get_preview_dir, which probes exactly this path first.
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("previews")),
    ];
    for previews in preview_roots.into_iter().flatten() {
        if !previews.is_dir() {
            continue;
        }
        if let Err(e) = app.asset_protocol_scope().allow_directory(&previews, true) {
            log::warn!("Failed to extend asset scope to {}: {}", previews.display(), e);
        }
    }
}

/// Empty in-memory DB used when the real one can't be opened, so the app
/// can still reach the event loop and show the startup-error dialog without
/// commands panicking on missing state.
fn fallback_in_memory_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory()
        .expect("in-memory SQLite cannot fail to open");
    let _ = db::init(&conn);
    conn
}

/// Open the installed DB, (re)installing the bundled one when the file is
/// missing, unreadable, or empty (post factory-reset). Every failure comes
/// back as a message for the startup error dialog instead of a panic.
fn open_or_reinstall_db(db_path: &Path) -> Result<rusqlite::Connection, String> {
    if !db_path.exists() {
        install_bundled_db(db_path)?;
    }

    match db::open(db_path).and_then(|c| {
        db::init(&c)?;
        Ok(c)
    }) {
        Ok(c) => {
            // Check if DB has games; if empty (factory reset), reinstall
            let count: i64 = c
                .query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))
                .unwrap_or(0);
            if count == 0 {
                drop(c);
                install_bundled_db(db_path)?;
                let c = db::open(db_path)
                    .map_err(|e| format!("failed to open freshly installed DB: {}", e))?;
                db::init(&c).map_err(|e| format!("failed to run migrations: {}", e))?;
                Ok(c)
            } else {
                Ok(c)
            }
        }
        Err(e) => {
            log::warn!("Database unreadable ({}), reinstalling", e);
            let _ = std::fs::remove_file(db_path);
            install_bundled_db(db_path)?;
            let c = db::open(db_path)
                .map_err(|e| format!("failed to open freshly installed DB: {}", e))?;
            db::init(&c).map_err(|e| format!("failed to initialize schema: {}", e))?;
            Ok(c)
        }
    }
}

/// Append collections this release added to the `collections` config, which
/// setup wrote once: a catalog refresh alone leaves their games with a
/// download button that cannot work (§15). Nothing is removed or reordered.
fn enable_new_collections(conn: &rusqlite::Connection) {
    let Ok(Some(current)) = db::queries::get_config(conn, "collections") else {
        return;
    };
    let mut ids: Vec<&str> = current.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let added: Vec<&str> = COLLECTION_MAP
        .iter()
        .map(|c| c.id)
        .filter(|id| !ids.contains(id))
        .collect();
    if added.is_empty() {
        return;
    }
    ids.extend(added.iter().copied());
    match db::queries::set_config(conn, "collections", &ids.join(",")) {
        Ok(_) => log::info!("Enabled newly shipped collections: {}", added.join(", ")),
        Err(e) => log::error!("Could not enable new collections {:?}: {}", added, e),
    }
}

/// Copy the bundled catalog DB into the data dir for ATTACH: in place it
/// would write WAL sidecars into the signed resources dir. Caller deletes.
fn stage_bundled_catalog(data_dir: &Path) -> Result<std::path::PathBuf, String> {
    let metadata_dir = bundled_metadata_dir()?;
    let tmp = data_dir.join("catalog-refresh.db");

    let bundled_db = metadata_dir.join("exodium.db");
    if bundled_db.exists() {
        std::fs::copy(&bundled_db, &tmp).map_err(|e| e.to_string())?;
        return Ok(tmp);
    }
    let bundled_db_gz = metadata_dir.join("exodium.db.gz");
    if bundled_db_gz.exists() {
        let file = std::fs::File::open(&bundled_db_gz).map_err(|e| e.to_string())?;
        let mut decoder = flate2::read::GzDecoder::new(file);
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut decoder, &mut out).map_err(|e| e.to_string())?;
        return Ok(tmp);
    }
    Err(format!(
        "No bundled database found in {}",
        metadata_dir.display()
    ))
}

/// Make-writer that locks a shared file handle on every write. Cloning the
/// `Arc` is cheap; locking is per-event and brief, so contention is not a
/// concern for our log volume.
#[derive(Clone)]
struct SharedFileMakeWriter(std::sync::Arc<std::sync::Mutex<std::fs::File>>);

impl std::io::Write for SharedFileMakeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            // `write_all` rather than `write`: short writes on a regular file
            // are vanishingly rare but possible, and a partial log line that
            // tracing-subscriber doesn't retry would corrupt the log file.
            Ok(mut f) => f.write_all(buf).map(|_| buf.len()),
            // If the mutex is poisoned, drop the bytes rather than panic in
            // the logger. We still return Ok so the subscriber doesn't loop.
            Err(_) => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut f) => f.flush(),
            Err(_) => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedFileMakeWriter {
    type Writer = SharedFileMakeWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The tracing subscriber: stderr plus `<log_dir>/exodium.log` (the only
/// sink a packaged Windows build has), with `log!` bridged in. Returns the
/// log path.
fn init_logger(log_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::io::Write;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let _ = std::fs::create_dir_all(log_dir);
    let log_path = log_dir.join("exodium.log");

    // Rotate once the log grows past ~10 MB: keep exactly one predecessor
    // (exodium.log.1) so a crash/wedge can't destroy its own evidence and
    // the log can't grow unbounded (55 MB observed in the field).
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 10 * 1024 * 1024 {
            let _ = std::fs::rename(&log_path, log_dir.join("exodium.log.1"));
        }
    }

    let file_result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);

    // info by default; `RUST_LOG=librqbit=debug,exodium_lib=debug` to dig.
    let default_filter = "info";
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter));

    // Write a session separator before the subscriber takes over so multi-run
    // log files remain readable.
    let file_writer: Option<SharedFileMakeWriter> = match file_result {
        Ok(mut file) => {
            let epoch_secs = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(
                file,
                "\n=== exodium session start (epoch {}, log_dir {}) ===",
                epoch_secs,
                log_dir.display()
            );
            Some(SharedFileMakeWriter(std::sync::Arc::new(std::sync::Mutex::new(
                file,
            ))))
        }
        Err(_) => None,
    };

    // Build the subscriber: stderr layer + (optional) file layer + filter.
    // `with_target(true)` keeps "librqbit::session" prefixes so we can tell
    // librqbit events apart from our own.
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(false);
    let registry = tracing_subscriber::registry().with(env_filter).with(stderr_layer);

    let result = if let Some(writer) = file_writer.clone() {
        let file_layer = fmt::layer()
            .with_writer(writer)
            .with_target(true)
            .with_ansi(false);
        registry.with(file_layer).try_init()
    } else {
        registry.try_init()
    };

    if result.is_err() {
        // A subscriber was already installed (e.g. tests) - not fatal.
        return None;
    }

    // Bridge `log!` → tracing so log-only crates land in the same sink.
    // Ignore failure: it just means log was already initialized elsewhere.
    let _ = tracing_log::LogTracer::init();

    file_writer.map(|_| log_path)
}

/// Which WebKitGTK render-path workaround this Linux session needs.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderPath {
    /// Drop accelerated compositing entirely (WebProcess rasterizes on its
    /// main thread). Costly, but the only thing that renders on X11/NVIDIA.
    pub disable_dmabuf: bool,
    /// Keep the DMA-BUF renderer but take NVIDIA's implicit-sync path.
    pub disable_explicit_sync: bool,
    /// Override the NVIDIA blocklist WebKitGTK < 2.52 applies to its own
    /// DMA-BUF renderer. Unread by 2.52+, which dropped the check.
    pub force_dmabuf: bool,
}

/// The WebKitGTK render path per GPU vendor and GDK backend (§17 has the
/// measurements). NVIDIA needs a different workaround on Wayland (explicit
/// sync off) and X11 (DMA-BUF off); everyone else gets nothing set.
/// `accel_known_bad` (see `accel_sentinel`) forces the path that always
/// renders after an accelerated start died.
#[cfg(target_os = "linux")]
pub fn choose_render_path(nvidia: bool, wayland: bool, accel_known_bad: bool) -> RenderPath {
    if !nvidia {
        return RenderPath {
            disable_dmabuf: false,
            disable_explicit_sync: false,
            force_dmabuf: false,
        };
    }
    if wayland && !accel_known_bad {
        return RenderPath {
            disable_dmabuf: false,
            disable_explicit_sync: true,
            force_dmabuf: true,
        };
    }
    RenderPath {
        disable_dmabuf: true,
        disable_explicit_sync: false,
        force_dmabuf: false,
    }
}

/// True when the proprietary NVIDIA driver is loaded. nouveau creates neither
/// of these, so Intel/AMD/nouveau fall through to the upstream default.
#[cfg(target_os = "linux")]
fn nvidia_proprietary_in_use() -> bool {
    Path::new("/sys/module/nvidia_drm").exists() || Path::new("/dev/nvidia0").exists()
}

/// The GDK backend, decided as GTK does (`GDK_BACKEND`, else
/// `WAYLAND_DISPLAY`). Read AFTER `prefer_wayland_backend_in_appimage`.
#[cfg(target_os = "linux")]
fn on_wayland_backend() -> bool {
    match std::env::var("GDK_BACKEND") {
        Ok(v) => v
            .split(',')
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case("wayland")),
        Err(_) => std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

/// Marker armed before an accelerated start, cleared after 6 s or a clean
/// exit. Found at startup, it means the last attempt died at first paint,
/// which is after `setup()` - so only elapsed time counts as survival.
#[cfg(target_os = "linux")]
fn accel_sentinel() -> Option<std::path::PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() => std::path::PathBuf::from(d),
        _ => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(base.join("com.redfox.exodium").join("accel-attempt"))
}

/// The AppImage no longer exports `GDK_BACKEND=x11`; after a failed start
/// this puts it back. Returns whether Wayland is being attempted, so the
/// sentinel is armed for every GPU (tauri#8541 is not vendor-specific).
#[cfg(target_os = "linux")]
fn prefer_wayland_backend_in_appimage(accel_known_bad: bool) -> bool {
    // APPDIR is set by AppRun, and the AppImage is the only build whose GDK
    // backend anybody ever forced.
    if std::env::var_os("APPDIR").is_none()
        || std::env::var_os("GDK_BACKEND").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return false;
    }
    if accel_known_bad {
        std::env::set_var("GDK_BACKEND", "x11");
        return false;
    }
    true
}

/// Apply the chosen render path, leaving any variable the user set themselves
/// alone - that is the escape hatch when this guess is wrong on their box.
#[cfg(target_os = "linux")]
fn apply_render_path() {
    let sentinel = accel_sentinel();
    let known_bad = sentinel.as_ref().is_some_and(|p| p.exists());
    let appimage_wayland = prefer_wayland_backend_in_appimage(known_bad);
    let nvidia = nvidia_proprietary_in_use();
    let wayland = on_wayland_backend();
    let path = choose_render_path(nvidia, wayland, known_bad);

    let mut applied: Vec<&str> = Vec::new();
    if path.disable_dmabuf && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        applied.push("WEBKIT_DISABLE_DMABUF_RENDERER=1");
    }
    if path.force_dmabuf && std::env::var_os("WEBKIT_FORCE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_FORCE_DMABUF_RENDERER", "1");
        applied.push("WEBKIT_FORCE_DMABUF_RENDERER=1");
    }
    if path.disable_explicit_sync && std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none() {
        std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        applied.push("__NV_DISABLE_EXPLICIT_SYNC=1");
    }

    // Logging is not up yet (init_logger needs the app handle), so this goes
    // to stderr - it still lands in the journal and in a terminal bug report.
    eprintln!(
        "render path: nvidia={nvidia} wayland={wayland} accel_known_bad={known_bad} -> [{}]",
        if applied.is_empty() {
            "upstream defaults".to_string()
        } else {
            applied.join(", ")
        }
    );

    if (!path.disable_dmabuf && nvidia) || appimage_wayland {
        // Arm the sentinel; first paint or a clean exit disarms it.
        if let Some(p) = sentinel {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&p, "1");
            // The GDK failure hits within a second; a kill inside this
            // window costs the next start its acceleration, nothing more.
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(6));
                disarm_accel_sentinel();
            });
        }
    } else {
        disarm_accel_sentinel();
    }
}

/// Record that this render path got the app running.
#[cfg(target_os = "linux")]
fn disarm_accel_sentinel() {
    if let Some(p) = accel_sentinel() {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    raise_fd_limit();

    #[cfg(target_os = "linux")]
    apply_render_path();

    tauri::Builder::default()
        // A second instance would contend on the SQLite DB and corrupt the
        // torrent session; focus the existing window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Logger first, so the rest of setup is captured.
            let log_dir = app.path().app_log_dir().ok();
            // Cache the log directory so the `get_log_dir` Tauri command can
            // serve it without going through `app.path()` again - the
            // round-trip was observed failing in shipped Windows builds.
            if let Some(ref dir) = log_dir {
                init_log_dir(dir.clone());
            }
            let log_path = log_dir.as_deref().and_then(init_logger);
            if let Some(ref p) = log_path {
                log::info!("Log file: {}", p.display());
            }

            // Cache the resource_dir BEFORE any code tries to read bundled metadata,
            // torrents, or shaders - the sync helpers in setup.rs rely on this.
            if let Ok(res_dir) = app.path().resource_dir() {
                init_resource_dir(res_dir);
            } else {
                log::warn!("resource_dir() unavailable; bundled assets may not be found");
            }

            // The main window is built here rather than by the config
            // (`create: false`) for one reason: on Windows a WebDriver asks
            // for the remote-debugging port through
            // WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS, and WebView2 ignores that
            // variable once the app passes its own arguments - which wry
            // always does. With the variable unset the window is identical.
            let win_cfg = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or("tauri.conf.json defines no window")?;
            #[allow(unused_mut)]
            let mut window = tauri::WebviewWindowBuilder::from_config(app.handle(), &win_cfg)?;
            #[cfg(target_os = "windows")]
            if let Ok(extra) = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS") {
                // wry's own defaults, which the override would otherwise drop.
                window = window.additional_browser_args(&format!(
                    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
                     --autoplay-policy=no-user-gesture-required {extra}"
                ));
            }
            window.build()?;

            // A fatal error here must be VISIBLE: a panic shows nothing, and
            // a blocking dialog deadlocks before the event loop on macOS. So:
            // non-blocking dialog, in-memory DB, let the loop start.
            let mut startup_error: Option<String> = None;

            let data_dir = match app.path().app_data_dir() {
                Ok(d) => d,
                Err(e) => {
                    startup_error =
                        Some(format!("Could not resolve the application data directory: {}", e));
                    std::env::temp_dir().join("exodium-fallback")
                }
            };
            if startup_error.is_none() {
                if let Err(e) = std::fs::create_dir_all(&data_dir) {
                    startup_error = Some(format!(
                        "Could not create the application data directory {}: {}",
                        data_dir.display(),
                        e
                    ));
                }
            }
            let db_path = data_dir.join("exodium.db");
            log::info!("Database path: {}", db_path.display());

            let mut conn = if startup_error.is_none() {
                match open_or_reinstall_db(&db_path) {
                    Ok(c) => c,
                    Err(msg) => {
                        startup_error = Some(format!("Could not open the game database: {}", msg));
                        fallback_in_memory_db()
                    }
                }
            } else {
                fallback_in_memory_db()
            };

            if let Some(msg) = &startup_error {
                log::error!("Fatal startup error: {}", msg);
                use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                let exit_handle = app.handle().clone();
                app.dialog()
                    .message(format!("{}\n\nSee the log folder for details.", msg))
                    .title("Exodium failed to start")
                    .kind(MessageDialogKind::Error)
                    .show(move |_| exit_handle.exit(1));
            }

            // Catalog refresh when the bundled version is ahead; user state
            // and ids survive (`db::refresh_catalog`).
            let installed_ver = db::catalog_version(&conn);
            if startup_error.is_none() && installed_ver < db::CATALOG_VERSION {
                match stage_bundled_catalog(&data_dir) {
                    Ok(cat_path) => {
                        match db::refresh_catalog(&mut conn, &cat_path) {
                            Ok((updated, inserted)) => {
                                log::info!(
                                    "Catalog refreshed v{} -> v{}: {} rows updated, {} inserted",
                                    installed_ver, db::CATALOG_VERSION, updated, inserted
                                );
                                enable_new_collections(&conn);
                            }
                            Err(e) => log::error!("Catalog refresh failed: {}", e),
                        }
                        let _ = std::fs::remove_file(&cat_path);
                    }
                    Err(e) => log::error!("Catalog refresh: bundled DB unavailable: {}", e),
                }
            }

            // Establishes the game root - and repairs a pre-single-root install
            // on the way, which rewrites data_dir. Must run before anything
            // below reads it.
            commands::paths::load_root_folder(&conn);

            // Clean up stale content-pack download artifacts from interrupted installs.
            if let Ok(Some(user_data_dir)) = db::queries::get_config(&conn, "data_dir") {
                let user_data_path = std::path::Path::new(&user_data_dir);
                // Asset protocol must reach game media in the user-chosen dir.
                allow_asset_dir(app.handle(), user_data_path);
                commands::content_packs::cleanup_stale_downloads(user_data_path);
                commands::content_packs::cleanup_stale_content_packs(&conn, user_data_path);
            }

            app.manage(DbState(Mutex::new(conn)));
            app.manage(TorrentState(RwLock::new(std::collections::HashMap::new())));
            app.manage(ContentPackState::new());
            app.manage(commands::media::VideoState::new());
            app.manage(commands::media::MusicState::new());
            app.manage(commands::media::MediaServerState::new());

            // macOS: native traffic lights; elsewhere the custom frame.
            #[cfg(target_os = "macos")]
            {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.set_decorations(true);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_games,
            get_game,
            get_installed_games,
            get_game_variants,
            get_genres,
            launch_game,
            game_printing_unavailable,
            game_engine_info,
            get_config,
            set_config,
            set_seeding_enabled,
            set_rate_limits,
            get_transfer_stats,
            update_check_supported,
            get_torrent_info,
            setup_start,
            get_setup_status,
            setup_import,
            setup_from_local,
            get_default_data_dir,
            get_available_collections,
            init_download_manager,
            commands::media::start_game_video,
            commands::media::get_video_status,
            commands::media::media_url,
            commands::media::video_playback_supported,
            commands::media::cancel_game_video,
            commands::media::start_game_music,
            commands::media::get_music_status,
            commands::media::cancel_game_music,
            commands::media::music_playback_supported,
            commands::media::music_shuffle_candidates,
            commands::media::music_cache_index,
            factory_reset,
            download_game,
            cancel_download,
            uninstall_game,
            reset_game_data,
            get_download_progress,
            toggle_favorite,
            get_section_keys,
            validate_exodos_dir,
            scan_installed_games,
            commands::layout::pending_layout_migration,
            commands::layout::migrate_layout,
            commands::layout::skip_layout_migration,
            commands::win9x::get_win9x_support_status,
            commands::win9x::win9x_engine_available,
            commands::scummvm::scummvm_engine_info,
            commands::scummvm::scummvm_variants,
            commands::scummvm::set_scummvm_options,
            commands::win9x::win9x_network_status,
            commands::win9x::enable_win9x_network,
            commands::win9x::disable_win9x_network,
            commands::win9x::win9x_multiplayer_info,
            commands::win9x::dismiss_win9x_network_prompt,
            list_content_packs,
            install_content_pack,
            uninstall_content_pack,
            get_content_pack_progress,
            cancel_content_pack_install,
            get_preview_dir,
            get_poster_dir,
            data_dir_is_empty,
            get_game_metadata,
            get_game_settings,
            set_game_settings,
            get_recently_played,
            get_log_dir,
            open_log_folder,
            open_manual,
            get_playlists,
            create_playlist,
            rename_playlist,
            delete_playlist,
            set_playlist_membership,
            get_game_playlists,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app, _event| {
            // Reaching a clean exit proves the render path works, whether or
            // not the timer got there first.
            #[cfg(target_os = "linux")]
            if matches!(_event, tauri::RunEvent::Exit) {
                disarm_accel_sentinel();
            }
        });
}

#[cfg(test)]
mod collection_migration_tests {
    use super::*;

    fn config_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    /// The upgrade path for a pack shipped after the user's install: its games
    /// arrive with the catalog refresh, so its manager has to start too.
    #[test]
    fn appends_collections_the_install_predates() {
        let conn = config_db();
        db::queries::set_config(&conn, "collections", "eXoDOS,eXoDOS_GLP").unwrap();
        enable_new_collections(&conn);
        let after = db::queries::get_config(&conn, "collections").unwrap().unwrap();
        let ids: Vec<&str> = after.split(',').collect();
        for c in COLLECTION_MAP {
            assert!(ids.contains(&c.id), "{} missing from {:?}", c.id, ids);
        }
        // The user's existing entries keep their place.
        assert_eq!(&ids[..2], &["eXoDOS", "eXoDOS_GLP"]);
    }

    /// Nothing to do must mean no write - and no key must stay no key, so a
    /// pre-setup install is not handed a collection list it never chose.
    #[test]
    fn leaves_complete_and_unset_configs_alone() {
        let conn = config_db();
        let all = COLLECTION_MAP.iter().map(|c| c.id).collect::<Vec<_>>().join(",");
        db::queries::set_config(&conn, "collections", &all).unwrap();
        enable_new_collections(&conn);
        assert_eq!(db::queries::get_config(&conn, "collections").unwrap().unwrap(), all);

        let fresh = config_db();
        enable_new_collections(&fresh);
        assert_eq!(db::queries::get_config(&fresh, "collections").unwrap(), None);
    }
}

#[cfg(all(test, unix))]
mod fd_limit_tests {
    #[test]
    fn raise_fd_limit_reaches_torrent_scale() {
        super::raise_fd_limit();
        unsafe {
            let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim), 0);
            println!("soft limit after raise: {} (hard {})", lim.rlim_cur, lim.rlim_max);
            // Floor candidate is 10240; anything less means the raise loop broke.
            assert!(lim.rlim_cur >= 10240.min(lim.rlim_max));
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod render_path_tests {
    use super::choose_render_path;

    /// Intel/AMD/nouveau must keep WebKit's default DMA-BUF path - it is the
    /// fast one there, and the old blanket disable made them pay for a bug
    /// only the NVIDIA proprietary driver has.
    #[test]
    fn non_nvidia_gets_no_workaround() {
        for wayland in [true, false] {
            for bad in [true, false] {
                let p = choose_render_path(false, wayland, bad);
                assert!(!p.disable_dmabuf);
                assert!(!p.disable_explicit_sync);
                assert!(!p.force_dmabuf);
            }
        }
    }

    /// NVIDIA on Wayland: keep accelerated compositing, fix it with implicit
    /// sync. Measured 60.8 fps vs 10.6 for the disable.
    #[test]
    fn nvidia_wayland_keeps_acceleration() {
        let p = choose_render_path(true, true, false);
        assert!(!p.disable_dmabuf);
        assert!(p.disable_explicit_sync);
    }

    /// Keeping the renderer is not enough before WebKitGTK 2.52 - it refuses
    /// to use it on NVIDIA unless forced, which is what left the AppImage's
    /// bundled 2.50.4 at 12.8 fps while the host's 2.52.5 did 60.
    #[test]
    fn nvidia_wayland_overrides_the_pre_2_52_blocklist() {
        assert!(choose_render_path(true, true, false).force_dmabuf);
    }

    /// NVIDIA on X11/XWayland cannot allocate GBM buffers at all, so the
    /// DMA-BUF renderer has to go or nothing ever paints - forcing it there
    /// would be forcing the path that never paints.
    #[test]
    fn nvidia_x11_disables_dmabuf() {
        let p = choose_render_path(true, false, false);
        assert!(p.disable_dmabuf);
        assert!(!p.disable_explicit_sync);
        assert!(!p.force_dmabuf);
    }

    /// A start that already died with acceleration on falls back to the path
    /// that always renders, rather than looping on a black window.
    #[test]
    fn a_failed_accelerated_start_falls_back() {
        let p = choose_render_path(true, true, true);
        assert!(p.disable_dmabuf);
        assert!(!p.disable_explicit_sync);
        assert!(!p.force_dmabuf);
    }
}
