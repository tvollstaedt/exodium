//! Getting a game onto disk: queueing its torrent files, watching progress
//! until the zip is extracted, and the launch-time unpack for imports.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::db;
use crate::db::queries;
use crate::models::Game;
use crate::torrent::manager::DownloadProgress;

use super::collections::{collection_game_prefix, collection_rel_game_dir, collection_rel_zip};
use super::games::game_op_lock;
use super::{DbState, TorrentState};

/// Per-game (last_retry_at, attempts) for the stuck-download nudge in
/// `get_download_progress`; cleared once the zip appears.
static RETRY_STATE: OnceLock<
    Mutex<std::collections::HashMap<i64, (std::time::Instant, u32)>>,
> = OnceLock::new();

fn retry_state() -> &'static Mutex<std::collections::HashMap<i64, (std::time::Instant, u32)>> {
    RETRY_STATE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
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
        let conn = db_state.lock()?;
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

    let launcher = crate::commands::collections::collection_def(source).map(|c| c.launcher);
    let is_win9x_collection = launcher == Some(crate::commands::collections::Launcher::Win9x);
    let is_scummvm_collection = launcher == Some(crate::commands::collections::Launcher::ScummVm);

    // The Win9x support payload is queued below; the disk preflight has to
    // budget it here (zip + inner temp + extracted, ~7.5 GB).
    let win9x_support_missing = is_win9x_collection
        && !crate::commands::win9x::win9x_support_ready(
            &manager.torrent_root(),
            game.dosbox_variant.as_deref(),
        );

    // ScummVM's support payload (MT-32 ROMs, eXo's builds on Windows), same
    // shape as Win9x's; the preflight budgets it below.
    let scummvm_support_missing =
        is_scummvm_collection && !crate::support_files::scummvm_ready(&manager.torrent_root());

    let data_dir = {
        let conn = db_state.lock()?;
        queries::get_config(&conn, "data_dir").ok().flatten()
    };

    // A localized variant that is only a patch needs the English game on
    // disk; it is installed alongside as a real dependency, not folded in
    // silently, so it shows up in the library and can be played on its own.
    let base: Option<crate::models::Game> = {
        let conn = db_state.lock()?;
        crate::commands::lp_overlay::base_for(&conn, &game)
    };

    // macOS/Linux: the emulator pack rides along. Win9x is resolver-gated so
    // a system install never pays for it; ScummVM queues unless eXo's build
    // or the pack itself is present - a system ScummVM ignores the pin.
    let emulator_pack: Option<(String, crate::commands::updates::ContentPackInfo)> = if cfg!(windows) {
        None
    } else if is_win9x_collection {
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
                    .map(|info| (pack_id.to_string(), info))
            })
    } else if is_scummvm_collection {
        use crate::commands::scummvm::{pack_id, resolve_scummvm, EngineSource};
        let dd = data_dir.clone().unwrap_or_default();
        let slug = game.dosbox_variant.as_deref().unwrap_or("default");
        let pinned = resolve_scummvm(&manager.torrent_root(), &dd, slug)
            .is_some_and(|r| matches!(r.source, EngineSource::Exo | EngineSource::Pack));
        if pinned {
            None
        } else {
            let id = pack_id(slug);
            crate::commands::content_packs::installable_pack(source, &id).map(|info| (id, info))
        }
    } else {
        None
    };

    // Disk preflight before set_in_library, so a refusal leaves no phantom
    // entry. Bytes already on disk reduce the download, not the extraction.
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
            if scummvm_support_missing {
                needed += 3 * 1024 * 1024 * 1024;
            }
            if let Some(b) = base.as_ref().and_then(|b| b.download_size) {
                // Archive plus its own extraction, plus the second copy in
                // the overlay's directory.
                needed = needed.saturating_add((b as u64).saturating_mul(3));
            }
            if let Some((_, info)) = &emulator_pack {
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
        crate::support_files::ensure_queued(&crate::support_files::WIN9X_SUPPORT, &manager).await;
    }
    if scummvm_support_missing {
        crate::support_files::ensure_queued(&crate::support_files::SCUMMVM_SUPPORT, &manager).await;
    }

    // Queue the emulator pack alongside (see emulator_pack above). "Install
    // already in progress" is the normal second-download case.
    if let Some((pack_id, _)) = &emulator_pack {
        if let Err(e) =
            crate::commands::content_packs::start_pack_install(&app, source, pack_id).await
        {
            log::info!("Emulator pack '{pack_id}' not queued: {e}");
        }
    }

    if let Some(ref main_mgr) = main_mgr_opt {
        // Queue !DOSmetadata.zip (DOSBox configs) if the configs tree is
        // missing (normally pre-created by the bundled configs zip).
        let main_prefix = collection_game_prefix("eXoDOS");
        let main_segment = crate::commands::collections::collection_def("eXoDOS")
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

        // MT-32 ROMs and soundfont (util.zip, ~630 MB) fetched once, with
        // the first game whose conf requests MIDI.
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
            crate::support_files::ensure_queued(&crate::support_files::DOS_SUPPORT, main_mgr).await;
        }
    }

    manager
        .download_files(files)
        .await
        .map_err(|e| format!("Failed to queue download: {}", e))?;

    // The English base rides along as its own library entry. Queued after
    // the patch so a failure there cannot leave a dependency without the
    // game that asked for it.
    if let Some(base) = base.as_ref() {
        if let Err(e) = queue_base_game(&app, &db_state, &main_mgr_opt, base).await {
            log::warn!("English base for {} not queued: {e}", game.title);
        }
    }

    // Mark as in library only after the download is actually queued - doing
    // it earlier left a phantom "My Games" card when queueing failed.
    {
        let conn = db_state.lock()?;
        queries::set_in_library(&conn, id).map_err(|e| e.to_string())?;
    }

    Ok(format!("Downloading: {}", game.title))
}

/// Select the English base game's archive and put it in the library, so the
/// overlay variant has something to sit on. The frontend hears about it
/// through `dependency-download-started` and shows it like any download.
async fn queue_base_game(
    app: &AppHandle,
    db_state: &State<'_, DbState>,
    main_mgr: &Option<std::sync::Arc<crate::torrent::manager::DownloadManager>>,
    base: &crate::models::Game,
) -> Result<(), String> {
    let Some(id) = base.id else { return Err("base row has no id".into()) };
    if base.installed {
        return Ok(());
    }
    let mgr = main_mgr.as_ref().ok_or("eXoDOS manager not initialized")?;
    let idx = base
        .game_torrent_index
        .ok_or("base row has no torrent index")? as usize;
    let mut files = vec![idx];
    if let Some(gd) = base.gamedata_torrent_index {
        files.push(gd as usize);
    }
    mgr.download_files(files)
        .await
        .map_err(|e| e.to_string())?;
    {
        let conn = db_state.lock()?;
        queries::set_in_library(&conn, id).map_err(|e| e.to_string())?;
    }
    let _ = app.emit(
        "dependency-download-started",
        serde_json::json!({
            "id": id,
            "title": base.title,
            "torrentSource": base.torrent_source,
            "thumbnailKey": base.thumbnail_key,
        }),
    );
    log::info!("Queued English base '{}' for an overlay variant", base.title);
    Ok(())
}

/// A library row whose torrent file the session has selected. Complete-but-
/// unextracted rows count too - the extraction runs from the progress poll,
/// so they need a watcher as well. `installed && !complete` is a false
/// install: a sparse archive that passed the size test mid-download.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingDownload {
    pub id: i64,
    pub title: String,
    /// For the download sheet's cover (`thumbnailCandidates`).
    pub torrent_source: String,
    pub thumbnail_key: Option<String>,
    #[serde(skip)]
    pub rel_path: String,
    pub installed: bool,
    pub complete: bool,
}

pub(crate) async fn pending_downloads(
    db: &Mutex<rusqlite::Connection>,
    torrent_state: &TorrentState,
) -> Result<Vec<PendingDownload>, String> {
    let rows: Vec<(i64, String, String, i64, bool, Option<String>)> = {
        let conn = db.lock().map_err(|_| "database lock poisoned".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, torrent_source, game_torrent_index, installed, thumbnail_key \
                 FROM games WHERE in_library = 1 AND game_torrent_index IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, i64>(4)? != 0, r.get(5)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let managers = torrent_state.0.read().await.clone();
    let mut out = Vec::new();
    for (id, title, source, idx, installed, thumbnail_key) in rows {
        let Some(mgr) = managers.get(&source) else { continue };
        let idx = idx as usize;
        if !mgr.is_file_selected(idx).await {
            continue;
        }
        let Some(file) = mgr.index().files.get(idx) else { continue };
        let complete = mgr.is_file_complete(idx).await;
        // An installed row whose archive is complete is simply done.
        if installed && complete {
            continue;
        }
        out.push(PendingDownload {
            id,
            title,
            torrent_source: source.clone(),
            thumbnail_key,
            rel_path: file.path.clone(),
            installed,
            complete,
        });
    }
    Ok(out)
}

/// Downloads to pick up after a restart: the frontend re-arms a tracker per
/// row, which is what shows progress and, on completion, extracts.
#[tauri::command]
pub async fn list_active_downloads(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<Vec<PendingDownload>, String> {
    pending_downloads(&db_state.0, &torrent_state).await
}

/// Get download progress for a game. If complete, extract and mark installed.
#[tauri::command]
pub async fn get_download_progress(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<Option<DownloadProgress>, String> {
    let (game_idx, gamedata_idx, title, already_installed, source, base) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        let base = crate::commands::lp_overlay::base_for(&conn, &game);
        match game.game_torrent_index {
            Some(idx) => (
                idx as usize,
                game.gamedata_torrent_index.map(|i| i as usize),
                game.title,
                game.installed,
                game.torrent_source.unwrap_or_else(|| "eXoDOS".to_string()),
                base,
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

    // librqbit's per-file stat can stall short of total for a file fully
    // on disk ("Waiting for last pieces..." forever): trust the disk size.
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
                        // An overlay variant unpacks onto a copy of the
                        // English game. Its download is queued with this
                        // one, so the wait is bounded by that transfer.
                        let base_src = match base.as_ref() {
                            Some(b) => {
                                match crate::commands::lp_overlay::base_game_dir(
                                    &manager.torrent_root(),
                                    b,
                                ) {
                                    Some(dir) => Some((dir, b.shortcode.clone(), b.id)),
                                    None => {
                                        let _ = std::fs::remove_file(&lock_path);
                                        log::debug!(
                                            "{title}: waiting for the English base before unpacking"
                                        );
                                        if let Some(ref mut p) = progress {
                                            p.waiting_for = Some(b.title.clone());
                                        }
                                        return Ok(progress);
                                    }
                                }
                            }
                            None => None,
                        };
                        let game_id = id;
                        let db_path = {
                            let conn = db_state.lock()?;
                            conn.path().map(PathBuf::from)
                                .ok_or_else(|| "Cannot determine database path".to_string())?
                        };

                        tauri::async_runtime::spawn(async move {
                            // Under the game lock: an uninstall mid-extraction
                            // would back up a half-extracted dir.
                            let op_lock = game_op_lock(game_id);
                            let _op_guard = op_lock.lock().await;
                            // The English tree must not be uninstalled while
                            // it is being copied into this variant.
                            let _base_guard = match base_src.as_ref().and_then(|(_, _, id)| *id) {
                                Some(base_id) => Some(game_op_lock(base_id).lock_owned().await),
                                None => None,
                            };
                            // Re-check: uninstall/cancel may have removed the ZIP
                            // while we waited for the lock.
                            if !zip_path.exists() {
                                let _ = std::fs::remove_file(&lock_path);
                                return;
                            }
                            log::info!("Extracting {} from {}", title, zip_path.display());
                            let extract_result = {
                                let (z, d) = (zip_path.clone(), extract_dir.clone());
                                let base_src = base_src.clone();
                                tauri::async_runtime::spawn_blocking(move || {
                                    if let Some((src, Some(sc), _)) = base_src {
                                        let dst = d.join(&sc);
                                        if let Err(e) =
                                            crate::commands::lp_overlay::clone_tree(&src, &dst)
                                        {
                                            log::error!(
                                                "Could not place the English base at {}: {e}",
                                                dst.display()
                                            );
                                            return Err(ExtractError::Other(e.to_string()));
                                        }
                                    }
                                    extract_game_zip(&z, &d)
                                })
                                .await
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
                                    // Placeholder/fragment: drop it and clear in_library
                                    // so the 1 Hz poll stops re-extracting it forever.
                                    if e.is_not_an_archive() {
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
                    // 100% but no zip: the pieces arrived with a neighbour
                    // and were never assembled. Toggle the selection
                    // (deselect, yield, re-add) to make librqbit look again;
                    // one attempt per 5 s.
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
                    // (attempts, ticked): the counter moves once per 5 s, so
                    // the polls in between see a stable value.
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
) -> Result<Vec<CancelledDependent>, String> {
    let (game_idx, gamedata_idx, source) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;

        // The GameData ZIP is shared with this game's other language
        // variants (LP installs auto-download the EN GameData). Only
        // deselect it when no other in-flight download still needs it.
        let gamedata_idx = match game.gamedata_torrent_index {
            Some(gd) => {
                // in_library, not installed=0: an extracted variant may still
                // be fetching this GameData.
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
        let conn = db_state.lock()?;
        queries::clear_in_library(&conn, id).map_err(|e| e.to_string())?;
    }

    // Overlay variants waiting on this English base can never finish without
    // it, and would sit at "waiting" forever. They go with it, and the
    // frontend is told so it can drop their cards.
    let dependents = cancel_dependents(&db_state, &torrent_state, id).await?;

    Ok(dependents)
}

/// Cancel every not-yet-installed overlay variant that was waiting on this
/// row, and report their ids. Empty for anything that is not such a base.
async fn cancel_dependents(
    db_state: &State<'_, DbState>,
    torrent_state: &TorrentState,
    base_id: i64,
) -> Result<Vec<CancelledDependent>, String> {
    let waiting: Vec<(i64, String, usize, String)> = {
        let conn = db_state.lock()?;
        let Some(base) = queries::fetch_game_by_id(&conn, base_id).map_err(|e| e.to_string())? else {
            return Ok(Vec::new());
        };
        crate::commands::lp_overlay::dependents_of(&conn, &base, false)
            .into_iter()
            .filter_map(|g| {
                Some((
                    g.id?,
                    g.title.clone(),
                    g.game_torrent_index? as usize,
                    g.torrent_source.clone()?,
                ))
            })
            .collect()
    };

    let mut cancelled = Vec::new();
    for (id, title, idx, source) in waiting {
        let manager = {
            let guard = torrent_state.0.read().await;
            guard.get(&source).cloned()
        };
        if let Some(manager) = manager {
            manager.deselect_file(idx).await;
        }
        {
            let conn = db_state.lock()?;
            queries::clear_in_library(&conn, id).map_err(|e| e.to_string())?;
        }
        log::info!("cancel_download: also cancelled '{title}', which needed the English base");
        cancelled.push(CancelledDependent { id, title });
    }
    Ok(cancelled)
}

/// A localized variant cancelled along with the English game it needed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CancelledDependent {
    pub id: i64,
    pub title: String,
}

/// (in_flight, last_failure) for the !DOSmetadata.zip extraction: one at a
/// time, and a corrupt zip is not retried every poll.
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

/// Copy a directory tree. Returns Err when ANY entry failed - callers that
/// delete the source after a "successful" backup must see partial copies as
/// failures, otherwise locked/unreadable files are silently lost.
pub(crate) fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
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

/// Why a zip did not unpack. `NotAnArchive` is what a librqbit placeholder
/// or a piece fragment looks like (§16) - the one case callers act on.
#[derive(Debug)]
pub(crate) enum ExtractError {
    NotAnArchive(String),
    Other(String),
}

impl ExtractError {
    pub(crate) fn is_not_an_archive(&self) -> bool {
        matches!(self, Self::NotAnArchive(_))
    }
}

impl From<zip::result::ZipError> for ExtractError {
    fn from(e: zip::result::ZipError) -> Self {
        use zip::result::ZipError::{InvalidArchive, UnsupportedArchive};
        match e {
            InvalidArchive(_) | UnsupportedArchive(_) => Self::NotAnArchive(e.to_string()),
            _ => Self::Other(e.to_string()),
        }
    }
}

impl From<std::io::Error> for ExtractError {
    fn from(e: std::io::Error) -> Self {
        Self::Other(e.to_string())
    }
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnArchive(m) | Self::Other(m) => f.write_str(m),
        }
    }
}

impl From<ExtractError> for String {
    fn from(e: ExtractError) -> String {
        e.to_string()
    }
}

/// Why `extract_before_launch` delivered no game directory.
#[derive(Debug)]
pub(crate) enum LaunchError {
    /// Torrent placeholder or fragment; `installed` has been cleared.
    Placeholder { title: String },
    /// The session is still fetching the archive.
    StillDownloading { title: String },
    NoShortcode { title: String },
    FilesMissing { title: String },
    /// Unpacked, but the folder is not where the catalogue expects it.
    Misplaced { title: String, expected: PathBuf },
    Extract(ExtractError),
    Task(String),
}

impl From<LaunchError> for String {
    fn from(e: LaunchError) -> String {
        match e {
            LaunchError::Placeholder { title } => format!(
                "Game ZIP for '{title}' is incomplete or corrupted (torrent placeholder). \
                 Please re-download the game."
            ),
            LaunchError::StillDownloading { title } => {
                format!("'{title}' is still downloading - it will install by itself when done.")
            }
            LaunchError::NoShortcode { title } => {
                format!("'{title}' has no game directory in the catalogue")
            }
            LaunchError::FilesMissing { title } => format!(
                "Game files not found for '{title}'. The game may need to be re-downloaded."
            ),
            LaunchError::Misplaced { title, expected } => format!(
                "'{title}' unpacked, but its folder is not where the catalogue expects it: {}",
                expected.display()
            ),
            LaunchError::Extract(e) => format!("Failed to extract game before launch: {e}"),
            LaunchError::Task(e) => format!("extraction task failed: {e}"),
        }
    }
}

/// Where a game's zip may sit: the collection layout first (lang dir for LP,
/// year dir for Win9x), then the prefix root.
fn launch_zip_candidates(torrent_root: &Path, source: &str, game_name: &str, app_path: Option<&str>) -> Vec<PathBuf> {
    let mut out = vec![torrent_root.join(collection_rel_zip(source, game_name, app_path))];
    let root_zip = torrent_root.join(format!("{}/{}.zip", collection_game_prefix(source), game_name));
    if root_zip != out[0] {
        out.push(root_zip);
    }
    out
}

/// Unpack a game's zip when its directory is missing - imports and rescans
/// leave games zipped, and LaunchBox unpacks on demand too. The one launch
/// pre-step for every collection. `zip.exists()` is true for almost every
/// uninstalled game (placeholders, fragments - §16), so a zip that will not
/// open is reported as such and `installed` is cleared.
pub(crate) async fn extract_before_launch(
    app: &AppHandle,
    game: &Game,
    id: i64,
    source: &str,
    torrent_root: &Path,
) -> Result<PathBuf, LaunchError> {
    let title = || game.title.clone();
    let shortcode = game
        .shortcode
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| LaunchError::NoShortcode { title: title() })?;
    let app_path = game.application_path.as_deref();
    let game_dir = torrent_root.join(collection_rel_game_dir(source, shortcode, app_path));
    if game_dir.exists() {
        return Ok(game_dir);
    }
    let game_name = app_path
        .and_then(crate::commands::library::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());
    let zip = launch_zip_candidates(torrent_root, source, &game_name, app_path)
        .into_iter()
        .find(|z| z.exists())
        .ok_or_else(|| LaunchError::FilesMissing { title: title() })?;
    // A zip the session is still filling is sparse and may open (its tail
    // piece can land first) - unpacking it would leave a half game dir that
    // reads as installed from then on.
    if let Some(idx) = game.game_torrent_index {
        let mgr = app.state::<TorrentState>().0.read().await.get(source).cloned();
        if let Some(mgr) = mgr {
            let idx = idx as usize;
            if mgr.is_file_selected(idx).await && !mgr.is_file_complete(idx).await {
                return Err(LaunchError::StillDownloading { title: title() });
            }
        }
    }
    log::info!("Auto-extracting {} before launch", zip.display());
    // Next to the zip, so the dir lands where the probe above looks.
    let dest = zip.parent().map(PathBuf::from).unwrap_or_else(|| torrent_root.to_path_buf());
    let extract = {
        let (z, d) = (zip.clone(), dest.clone());
        tauri::async_runtime::spawn_blocking(move || extract_game_zip(&z, &d))
            .await
            .map_err(|e| LaunchError::Task(e.to_string()))?
    };
    if let Err(e) = extract {
        if e.is_not_an_archive() {
            if let Ok(conn) = app.state::<DbState>().0.lock() {
                let _ = queries::set_game_installed(&conn, id, false);
            }
            return Err(LaunchError::Placeholder { title: title() });
        }
        return Err(LaunchError::Extract(e));
    }
    if !game_dir.exists() {
        return Err(LaunchError::Misplaced { title: title(), expected: game_dir });
    }
    Ok(game_dir)
}

/// Extract a game zip in place, then restore saves from `!save/` if present.
pub(crate) fn extract_game_zip(zip_path: &std::path::Path, dest: &std::path::Path) -> Result<(), ExtractError> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

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

    archive.extract(dest)?;
    log::info!("Extracted: {} -> {}", zip_path.display(), dest.display());

    // Restore `!save/<shortcode>` from beside the game dir or one level up
    // (the legacy shared location).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn placeholder_and_fragment_read_as_not_an_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty.zip");
        fs::write(&empty, b"").unwrap();
        let fragment = tmp.path().join("fragment.zip");
        fs::write(&fragment, vec![0u8; 4096]).unwrap();
        for zip in [empty, fragment] {
            let err = extract_game_zip(&zip, tmp.path()).unwrap_err();
            assert!(err.is_not_an_archive(), "{}: {err}", zip.display());
        }
        let missing = extract_game_zip(&tmp.path().join("nope.zip"), tmp.path()).unwrap_err();
        assert!(!missing.is_not_an_archive(), "a missing file is an I/O error, not a placeholder");
    }

    #[test]
    fn lp_zip_candidates_probe_lang_dir_then_root() {
        let root = std::path::Path::new("/r");
        let c = launch_zip_candidates(root, "eXoDOS_GLP", "Das Amt (1994)", None);
        assert_eq!(
            c,
            vec![
                root.join("eXo/eXoDOS/!german/Das Amt (1994).zip"),
                root.join("eXo/eXoDOS/Das Amt (1994).zip"),
            ]
        );
        let c = launch_zip_candidates(root, "eXoDOS", "Doom (1993)", None);
        assert_eq!(c, vec![root.join("eXo/eXoDOS/Doom (1993).zip")], "no duplicate for EN");
    }
}
