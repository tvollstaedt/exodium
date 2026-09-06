use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio::sync::RwLock;

use crate::db::queries;

use super::DbState;
use super::updates::{load_manifest, ContentPackInfo};

// ── Managed state ────────────────────────────────────────────────────────────

/// In-flight content-pack job state, keyed by "<collection>:<pack_id>".
/// Wrapped in Arc so the inner map can be cheaply cloned into spawned tasks
/// without running into Tauri's State<'_> lifetime restrictions.
pub struct ContentPackState(pub Arc<RwLock<HashMap<String, ContentPackJob>>>);

impl ContentPackState {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

pub struct ContentPackJob {
    phase: String,
    downloaded_bytes: u64,
    total_bytes: u64,
    finished: bool,
    installed: bool,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
}

// ── Progress query (polled at 1 Hz by the frontend) ──────────────────────────

#[derive(Debug, Serialize)]
pub struct ContentPackProgress {
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub progress: f64,
    pub finished: bool,
    pub installed: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn get_content_pack_progress(
    state: State<'_, ContentPackState>,
    collection: String,
    pack_id: String,
) -> Result<Option<ContentPackProgress>, String> {
    let jobs = state.0.read().await;
    let key = format!("{}:{}", collection, pack_id);
    Ok(jobs.get(&key).map(|j| ContentPackProgress {
        phase: j.phase.clone(),
        downloaded_bytes: j.downloaded_bytes,
        total_bytes: j.total_bytes,
        progress: if j.total_bytes > 0 {
            j.downloaded_bytes as f64 / j.total_bytes as f64
        } else {
            0.0
        },
        finished: j.finished,
        installed: j.installed,
        error: j.error.clone(),
    }))
}

// ── Installed-pack state in the config table ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPack {
    pub version: u32,
    pub size_bytes: u64,
    pub installed_at: String,
}

/// Installed pack state: { "eXoDOS": { "posters": { version, size_bytes, installed_at } } }
type InstalledPackMap = HashMap<String, HashMap<String, InstalledPack>>;

fn read_installed_packs(conn: &rusqlite::Connection) -> InstalledPackMap {
    queries::get_config(conn, "content_packs")
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn write_installed_packs(
    conn: &rusqlite::Connection,
    state: &InstalledPackMap,
) -> Result<(), String> {
    let json = serde_json::to_string(state).map_err(|e| e.to_string())?;
    queries::set_config(conn, "content_packs", &json).map_err(|e| e.to_string())
}

fn mark_pack_installed(
    conn: &rusqlite::Connection,
    collection: &str,
    pack_id: &str,
    version: u32,
    size_bytes: u64,
) -> Result<(), String> {
    let mut state = read_installed_packs(conn);
    state
        .entry(collection.to_string())
        .or_default()
        .insert(
            pack_id.to_string(),
            InstalledPack {
                version,
                size_bytes,
                installed_at: chrono_now(),
            },
        );
    write_installed_packs(conn, &state)
}

fn mark_pack_uninstalled(
    conn: &rusqlite::Connection,
    collection: &str,
    pack_id: &str,
) -> Result<(), String> {
    let mut state = read_installed_packs(conn);
    if let Some(col_map) = state.get_mut(collection) {
        col_map.remove(pack_id);
        if col_map.is_empty() {
            state.remove(collection);
        }
    }
    write_installed_packs(conn, &state)
}

fn chrono_now() -> String {
    // Simple ISO-ish timestamp without pulling in the chrono crate.
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

// ── List available content packs ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ContentPackStatus {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub size_bytes: u64,
    pub version: u32,
    pub supersedes: Vec<String>,
    /// True if the pack has a valid download URL (not a TODO placeholder).
    pub available: bool,
    pub installed: bool,
    pub installed_version: Option<u32>,
}

/// Record packs present on disk but missing from the ledger (a factory reset
/// keeps `content/` but clears the config table). Adopted at the manifest's
/// version, or the stale-pack cleanup would delete them next start.
fn adopt_packs_on_disk(
    conn: &rusqlite::Connection,
    collection: &str,
    col: &crate::commands::updates::CollectionManifest,
) {
    let Ok(Some(data_dir)) = queries::get_config(conn, "data_dir") else { return };
    let mut state = read_installed_packs(conn);
    let recorded = state.entry(collection.to_string()).or_default();
    let mut adopted = Vec::new();
    let mut vanished = Vec::new();
    for (id, info) in &col.content_packs {
        // A pack the current platform cannot see must not be adopted either,
        // or a data dir moved from another OS would grow ledger rows for
        // binaries this build can never use.
        let Some(info) = info.for_current_platform() else { continue };
        // install_path names the exact directory - it already carries the
        // collection (`content/posters/eXoWin9x`). Appending it a second time
        // meant metadata packs were never adopted at all.
        let dir = Path::new(&data_dir).join(&info.install_path);
        let present =
            dir.is_dir() && !std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(true);
        match (recorded.contains_key(id), present) {
            (false, true) => {
                recorded.insert(
                    id.clone(),
                    InstalledPack {
                        version: info.version,
                        size_bytes: info.size_bytes,
                        installed_at: chrono_now(),
                    },
                );
                adopted.push(id.clone());
            }
            // The ledger is not evidence, the disk is. A pack recorded as
            // installed whose files are gone otherwise shows "Remove" forever
            // and Settings offers no way back to a working state.
            (true, false) => {
                recorded.remove(id);
                vanished.push(id.clone());
            }
            _ => {}
        }
    }
    if adopted.is_empty() && vanished.is_empty() {
        return;
    }
    if !adopted.is_empty() {
        log::info!("Adopting content packs found on disk for {}: {:?}", collection, adopted);
    }
    if !vanished.is_empty() {
        log::info!("Content packs recorded but missing on disk for {}: {:?}", collection, vanished);
    }
    if let Err(e) = write_installed_packs(conn, &state) {
        log::warn!("Could not record adopted packs: {}", e);
    }
}

#[tauri::command]
pub async fn list_content_packs(
    db_state: State<'_, DbState>,
    collection: String,
) -> Result<Vec<ContentPackStatus>, String> {
    let manifest = load_manifest()?;
    let col = manifest
        .collections
        .get(&collection)
        .ok_or_else(|| format!("Unknown collection '{}'", collection))?;

    let conn = db_state.lock()?;
    adopt_packs_on_disk(&conn, &collection, col);
    let installed = read_installed_packs(&conn);
    let col_installed = installed.get(&collection);

    let mut result: Vec<ContentPackStatus> = col
        .content_packs
        .iter()
        .filter_map(|(id, info)| {
            // Platform-mapped packs resolve to their per-OS source or vanish
            // (the emulator packs exist on macOS/Linux only).
            let info = info.for_current_platform()?;
            let inst = col_installed.and_then(|c| c.get(id));
            Some(ContentPackStatus {
                id: id.clone(),
                display_name: info.display_name.clone(),
                description: info.description.clone(),
                size_bytes: info.size_bytes,
                version: info.version,
                supersedes: info.supersedes.clone(),
                available: info.torrent_file_path.is_some()
                    || (!info.url.is_empty() && !info.url.starts_with("TODO")),
                installed: inst.is_some(),
                installed_version: inst.map(|i| i.version),
            })
        })
        .collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

// ── Install a content pack ───────────────────────────────────────────────────

#[tauri::command]
pub async fn install_content_pack(
    app: AppHandle,
    collection: String,
    pack_id: String,
) -> Result<(), String> {
    start_pack_install(&app, &collection, &pack_id).await
}

/// The platform-resolved manifest entry for a pack, but only when it can
/// actually be installed right now (a real source, this platform). Backend
/// auto-triggers gate on this so a TODO-URL manifest entry stays inert.
pub(crate) fn installable_pack(collection: &str, pack_id: &str) -> Option<ContentPackInfo> {
    let manifest = load_manifest().ok()?;
    let info = manifest
        .collections
        .get(collection)?
        .content_packs
        .get(pack_id)?
        .for_current_platform()?;
    let has_http_source = !info.url.is_empty()
        && !info.url.starts_with("TODO")
        && !info.sha256.starts_with("TODO");
    (info.torrent_file_path.is_some() || has_http_source).then_some(info)
}

/// "content-pack-install-started" payload, emitted for every job: the
/// frontend only polls jobs it knows about, and the backend starts some.
#[derive(Clone, Serialize)]
struct PackInstallStarted {
    collection: String,
    pack_id: String,
    display_name: String,
}

/// Start a pack install job. Shared by the `install_content_pack` command
/// and backend triggers; returns once the job is registered (the download
/// runs in a spawned task, polled via `get_content_pack_progress`).
pub(crate) async fn start_pack_install(
    app: &AppHandle,
    collection: &str,
    pack_id: &str,
) -> Result<(), String> {
    use tauri::Manager;
    let db_state: State<'_, DbState> = app.state();
    let pack_state: State<'_, ContentPackState> = app.state();
    let collection = collection.to_string();
    let pack_id = pack_id.to_string();

    // Resolve pack info from the manifest (fast, in-memory).
    let manifest = load_manifest()?;
    let col = manifest
        .collections
        .get(&collection)
        .ok_or_else(|| format!("Unknown collection '{}'", collection))?;
    let pack_info = col
        .content_packs
        .get(&pack_id)
        .ok_or_else(|| format!("Unknown pack '{}' in '{}'", pack_id, collection))?
        .for_current_platform()
        .ok_or_else(|| {
            format!("'{}' is not available on this platform.", pack_id)
        })?;
    let col_packs = col.content_packs.clone();

    // Guard: reject packs without a real source. Torrent-sourced packs (via
    // torrent_file_path) are always considered available since librqbit runs
    // against the collection's already-loaded torrent.
    let has_torrent_source = pack_info.torrent_file_path.is_some();
    let has_http_source =
        !pack_info.url.is_empty() && !pack_info.url.starts_with("TODO");
    if !has_torrent_source && !has_http_source {
        return Err(format!("'{}' is not yet available for download.", pack_info.display_name));
    }
    // HTTP installs are only integrity-checked via the manifest sha256; a
    // placeholder hash would mean installing unverified content. Torrent
    // installs are unaffected (piece hashes cover them).
    if !has_torrent_source && pack_info.sha256.starts_with("TODO") {
        return Err(format!(
            "'{}' has no integrity hash in the manifest; refusing download.",
            pack_info.display_name
        ));
    }

    let key = format!("{}:{}", collection, pack_id);

    // Resolve data_dir (fast DB read, lock released immediately).
    let data_dir = {
        let conn = db_state.lock()?;
        crate::commands::games::configured_data_dir(&conn)?
    };

    // Atomic check-and-insert under a single write lock to prevent TOCTOU race
    // where two concurrent calls both pass the duplicate check.
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jmap = pack_state.0.write().await;
        if let Some(job) = jmap.get(&key) {
            if !job.finished {
                return Err("Install already in progress".to_string());
            }
        }
        jmap.insert(
            key.clone(),
            ContentPackJob {
                phase: "preparing".to_string(),
                downloaded_bytes: 0,
                total_bytes: pack_info.size_bytes,
                finished: false,
                installed: false,
                error: None,
                cancel: cancel.clone(),
            },
        );
    }

    {
        use tauri::Emitter;
        let _ = app.emit(
            "content-pack-install-started",
            PackInstallStarted {
                collection: collection.clone(),
                pack_id: pack_id.clone(),
                display_name: pack_info.display_name.clone(),
            },
        );
    }

    // Clone handles for the spawned task - return immediately so the UI stays responsive.
    let jobs_arc = pack_state.0.clone();
    let collection_clone = collection.clone();
    let pack_id_clone = pack_id.clone();
    let app_handle = app.clone();

    tokio::spawn(async move {
        let result = do_install_full(
            &jobs_arc,
            &app_handle,
            &data_dir,
            &collection_clone,
            &pack_info,
            &col_packs,
            &key,
            cancel,
        )
        .await;

        match result {
            Ok(()) => {
                use tauri::Manager;
                let db_state: State<DbState> = app_handle.state();
                if let Ok(conn) = db_state.0.lock() {
                    let _ = mark_pack_installed(
                        &conn,
                        &collection_clone,
                        &pack_id_clone,
                        pack_info.version,
                        pack_info.size_bytes,
                    );
                }
                let mut jobs = jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&key) {
                    job.phase = "installed".to_string();
                    job.finished = true;
                    job.installed = true;
                }
            }
            Err(e) => {
                log::error!("Content pack install failed: {}", e);
                let mut jobs = jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&key) {
                    job.phase = "failed".to_string();
                    job.finished = true;
                    job.error = Some(e);
                }
            }
        }
    });

    Ok(())
}

/// The directory whose contents become `install_dir`: the staging dir, or
/// its lone wrapper when that repeats the target's own name (§10). The name
/// check keeps a pack whose payload is a single `Images/` intact; AppleDouble
/// sidecars do not count as entries.
fn unwrapped_source(staging_dir: &Path, install_dir: &Path) -> PathBuf {
    let Some(target_name) = install_dir.file_name() else {
        return staging_dir.to_path_buf();
    };
    let Ok(entries) = std::fs::read_dir(staging_dir) else {
        return staging_dir.to_path_buf();
    };
    let mut only: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if crate::commands::paths::is_os_metadata(&path) {
            continue;
        }
        if only.is_some() {
            return staging_dir.to_path_buf();
        }
        only = Some(path);
    }
    match only {
        Some(p) if p.is_dir() && p.file_name() == Some(target_name) => p,
        _ => staging_dir.to_path_buf(),
    }
}

/// Move a finished staging tree into its install dir (both install routes).
fn commit_staging(staging_dir: &Path, install_dir: &Path) -> Result<(), String> {
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create install parent dir: {}", e))?;
    }
    // Whatever a previous version left there goes: packs are replaced whole.
    if install_dir.exists() {
        std::fs::remove_dir_all(install_dir)
            .map_err(|e| format!("Cannot remove old install: {}", e))?;
    }
    let source_dir = unwrapped_source(staging_dir, install_dir);
    // Atomic rename; fall back to copy on EXDEV (staging and install can sit
    // on different filesystems).
    if std::fs::rename(&source_dir, install_dir).is_err() {
        copy_dir_recursive(&source_dir, install_dir)?;
    }
    let _ = std::fs::remove_dir_all(staging_dir);
    Ok(())
}

/// Resolve the install dir for a given pack_id, checking the manifest for its
/// install_path. Falls back to a conventional path if the pack isn't in the manifest.
fn resolve_pack_install_dir(
    data_dir: &str,
    collection: &str,
    pack_id: &str,
    packs: &HashMap<String, ContentPackInfo>,
) -> PathBuf {
    let base = Path::new(data_dir);
    if let Some(info) = packs.get(pack_id) {
        safe_join(base, &info.install_path).unwrap_or_else(|_| base.join("content").join(pack_id).join(collection))
    } else {
        base.join("content").join(pack_id).join(collection)
    }
}

/// Full install pipeline: pre-flight (supersede removal, disk space) + download/verify/extract.
/// Runs entirely inside tokio::spawn so no blocking work stalls the Tauri command handler.
#[allow(clippy::too_many_arguments)]
async fn do_install_full(
    jobs: &Arc<RwLock<HashMap<String, ContentPackJob>>>,
    app_handle: &AppHandle,
    data_dir: &str,
    collection: &str,
    pack_info: &ContentPackInfo,
    col_packs: &HashMap<String, ContentPackInfo>,
    key: &str,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    // ── Pre-flight: remove superseded packs ──────────────────────────────────
    for superseded in &pack_info.supersedes {
        let install_path = resolve_pack_install_dir(data_dir, collection, superseded, col_packs);
        if install_path.exists() {
            log::info!("Removing superseded pack '{}' before installing '{}'", superseded, pack_info.display_name);
            std::fs::remove_dir_all(&install_path)
                .map_err(|e| format!("Failed to remove superseded pack: {}", e))?;
            use tauri::Manager;
            if let Ok(conn) = app_handle.state::<DbState>().0.lock() {
                let _ = mark_pack_uninstalled(&conn, collection, superseded);
            }
        }
    }

    // ── Pre-flight: check disk space ─────────────────────────────────────────
    let required = (pack_info.size_bytes as f64 * 2.2) as u64;
    let available = fs4::available_space(data_dir)
        .map_err(|e| format!("Cannot query disk space: {}", e))?;
    if available < required {
        return Err(format!(
            "Not enough disk space: need {}, available {}",
            format_bytes(required),
            format_bytes(available)
        ));
    }

    // Update phase to "downloading" now that pre-flight passed.
    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.phase = "downloading".to_string();
        }
    }

    // Offline means no network at all, not "no torrent". Fetching a poster pack
    // over HTTP would still be a download the user just declined - the promise
    // of the mode is what matters, not which protocol carries the bytes.
    let offline = {
        use tauri::Manager;
        let db: State<DbState> = app_handle.state();
        crate::commands::setup::is_offline(&db.0)
    };
    if offline {
        return Err(
            "Offline mode is on - nothing is downloaded. Switch to online mode in \
             Settings → Network."
                .to_string(),
        );
    }

    if pack_info.torrent_file_path.is_some() {
        do_install_torrent(jobs, app_handle, data_dir, collection, pack_info, key, cancel).await
    } else {
        do_install(jobs, data_dir, pack_info, key, cancel).await
    }
}

/// Torrent-sourced install: queue the file, poll, extract. The zip stays for
/// seeding and re-installs.
async fn do_install_torrent(
    jobs: &Arc<RwLock<HashMap<String, ContentPackJob>>>,
    app_handle: &AppHandle,
    data_dir: &str,
    collection: &str,
    pack_info: &ContentPackInfo,
    key: &str,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    use tauri::Manager;

    let torrent_file_path = pack_info
        .torrent_file_path
        .as_ref()
        .ok_or("torrent_file_path missing")?;

    // Resolve the collection's download manager.
    let manager = {
        let ts: State<crate::commands::TorrentState> = app_handle.state();
        let guard = ts.0.read().await;
        guard
            .get(collection)
            .cloned()
            .ok_or_else(|| format!("No torrent manager for collection '{}'", collection))?
    };

    // Locate the file inside the torrent's index.
    let file_index = manager
        .index()
        .find_by_path(torrent_file_path)
        .ok_or_else(|| {
            format!(
                "File '{}' not found in {} torrent",
                torrent_file_path, collection
            )
        })?
        .index;

    // Queue selective download (idempotent - safe to call even if already selected).
    manager
        .download_files(vec![file_index])
        .await
        .map_err(|e| format!("Failed to queue torrent download: {}", e))?;

    // A run of None from file_progress means the handle is gone; give up
    // instead of spinning.
    let mut none_streak = 0u32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            manager.deselect_file(file_index).await;
            return Err("Cancelled".to_string());
        }

        let progress = manager.file_progress(file_index).await;
        if let Some(p) = progress {
            none_streak = 0;
            let mut jmap = jobs.write().await;
            if let Some(job) = jmap.get_mut(key) {
                job.downloaded_bytes = p.downloaded_bytes;
                job.total_bytes = p.total_bytes;
            }
            if p.finished {
                break;
            }
        } else {
            none_streak += 1;
            if none_streak >= 10 {
                return Err(format!(
                    "Torrent progress unavailable for '{}' - the download manager \
                     lost the file handle. Restart Exodium and retry.",
                    pack_info.display_name
                ));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // ── Extract the downloaded ZIP into a staging dir, then atomic-rename. ──
    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.phase = "extracting".to_string();
        }
    }

    let zip_path = manager
        .file_output_path(file_index)
        .ok_or("Cannot resolve downloaded file path")?;

    let install_dir = safe_join(Path::new(data_dir), &pack_info.install_path)?;
    let staging_dir = Path::new(data_dir)
        .join(".content-downloads")
        .join(format!("{}.staging", key.replace(':', "_")));

    // Clean stale staging.
    let _ = std::fs::remove_dir_all(&staging_dir);
    std::fs::create_dir_all(&staging_dir)
        .map_err(|e| format!("Cannot create staging dir: {}", e))?;

    let staging_clone = staging_dir.clone();
    let zip_clone = zip_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let file = std::fs::File::open(&zip_clone)
            .map_err(|e| format!("Cannot open downloaded zip: {}", e))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("Invalid zip archive: {}", e))?;
        archive
            .extract(&staging_clone)
            .map_err(|e| format!("Zip extraction failed: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Extract task panicked: {}", e))??;

    // Commit: replace install_dir with staging.
    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.phase = "installing".to_string();
        }
    }

    commit_staging(&staging_dir, &install_dir)?;

    log::info!(
        "Content pack installed (torrent): {} → {}",
        pack_info.display_name,
        install_dir.display()
    );
    Ok(())
}

/// The actual download → verify → extract → commit pipeline.
async fn do_install(
    jobs: &Arc<RwLock<HashMap<String, ContentPackJob>>>,
    data_dir: &str,
    pack_info: &ContentPackInfo,
    key: &str,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    use flate2::read::GzDecoder;
    use sha2::{Digest, Sha256};
    

    let downloads_dir = Path::new(data_dir).join(".content-downloads");
    std::fs::create_dir_all(&downloads_dir)
        .map_err(|e| format!("Cannot create downloads dir: {}", e))?;

    let tmp_file = downloads_dir.join(format!("{}.tar.gz.tmp", key.replace(':', "_")));
    let staging_dir = downloads_dir.join(format!("{}.staging", key.replace(':', "_")));
    let install_dir = safe_join(Path::new(data_dir), &pack_info.install_path)?;

    // Clean up any stale leftovers from a previous attempt.
    let _ = std::fs::remove_file(&tmp_file);
    let _ = std::fs::remove_dir_all(&staging_dir);

    // ── Phase 1: Download + stream-hash ──────────────────────────────────────

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client init failed: {}", e))?;
    let response = client
        .get(&pack_info.url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download returned HTTP {}", response.status()));
    }

    let content_length = response.content_length().unwrap_or(pack_info.size_bytes);

    // Update total_bytes from the server's Content-Length.
    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.total_bytes = content_length;
        }
    }

    let mut hasher = Sha256::new();
    let mut file = std::fs::File::create(&tmp_file)
        .map_err(|e| format!("Cannot create temp file: {}", e))?;
    let mut downloaded: u64 = 0;

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();

    // A lying/misconfigured server must not fill the disk: allow 5% slack over
    // the manifest size, then abort.
    let size_cap = (pack_info.size_bytes > 0)
        .then(|| pack_info.size_bytes + pack_info.size_bytes / 20);

    loop {
        // Stall guard: a server that keeps the connection open without
        // sending data would otherwise hang the job forever.
        let next = tokio::time::timeout(std::time::Duration::from_secs(60), stream.next())
            .await
            .map_err(|_| {
                let _ = std::fs::remove_file(&tmp_file);
                "Download stalled: no data received for 60 seconds.".to_string()
            })?;
        let Some(chunk_result) = next else {
            break;
        };

        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp_file);
            return Err("Cancelled".to_string());
        }

        let chunk = chunk_result.map_err(|e| format!("Download error: {}", e))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| format!("Write error: {}", e))?;

        downloaded += chunk.len() as u64;
        if let Some(cap) = size_cap {
            if downloaded > cap {
                let _ = std::fs::remove_file(&tmp_file);
                return Err(format!(
                    "Download exceeded expected size ({} > {}); aborting.",
                    format_bytes(downloaded),
                    format_bytes(pack_info.size_bytes)
                ));
            }
        }

        // Update progress (not every chunk - throttle to avoid lock contention).
        if downloaded % (256 * 1024) < chunk.len() as u64 || downloaded >= content_length {
            let mut jmap = jobs.write().await;
            if let Some(job) = jmap.get_mut(key) {
                job.downloaded_bytes = downloaded;
            }
        }
    }

    drop(file);

    // ── Phase 2: Verify SHA256 ───────────────────────────────────────────────

    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.phase = "verifying".to_string();
        }
    }

    // No TODO-placeholder exemption here: install_content_pack refuses those
    // upfront, and a placeholder that slips through must fail closed.
    let hash = format!("{:x}", hasher.finalize());
    if hash != pack_info.sha256 {
        let _ = std::fs::remove_file(&tmp_file);
        return Err(format!(
            "Checksum mismatch - expected {}, got {}. Download may be corrupted, please retry.",
            pack_info.sha256, hash
        ));
    }

    // ── Phase 3: Extract ─────────────────────────────────────────────────────

    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.phase = "extracting".to_string();
        }
    }

    // Extract in a blocking thread since tar I/O is synchronous.
    let tmp_file_clone = tmp_file.clone();
    let staging_clone = staging_dir.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&staging_clone)
            .map_err(|e| format!("Cannot create staging dir: {}", e))?;
        let file = std::fs::File::open(&tmp_file_clone)
            .map_err(|e| format!("Cannot open temp file: {}", e))?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        archive
            .unpack(&staging_clone)
            .map_err(|e| format!("Extraction failed: {}", e))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("Extract task panicked: {}", e))?
    .map_err(|e: String| e)?;

    // ── Phase 4: Commit (atomic rename) ──────────────────────────────────────

    {
        let mut jmap = jobs.write().await;
        if let Some(job) = jmap.get_mut(key) {
            job.phase = "installing".to_string();
        }
    }

    commit_staging(&staging_dir, &install_dir)?;

    // Clean up temp tarball.
    let _ = std::fs::remove_file(&tmp_file);

    log::info!(
        "Content pack installed: {} → {}",
        pack_info.display_name,
        install_dir.display()
    );
    Ok(())
}

/// Copy a tree (cross-filesystem fallback for rename). Symlinks are
/// recreated, not followed: .app bundles depend on them and their seal.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {}", dst.display(), e))?;
    for entry in
        std::fs::read_dir(src).map_err(|e| format!("readdir {}: {}", src.display(), e))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        let dest = dst.join(entry.file_name());
        if ft.is_symlink() {
            let target = std::fs::read_link(entry.path())
                .map_err(|e| format!("readlink {}: {}", entry.path().display(), e))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dest)
                .map_err(|e| format!("symlink {}: {}", dest.display(), e))?;
            #[cfg(not(unix))]
            {
                // Windows pack payloads contain no symlinks; if one ever
                // appears, copying the target is the useful degradation.
                let _ = target;
                std::fs::copy(entry.path(), &dest).map_err(|e| format!("copy: {}", e))?;
            }
        } else if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)
                .map_err(|e| format!("copy: {}", e))?;
        }
    }
    Ok(())
}

// ── Uninstall a content pack ─────────────────────────────────────────────────

#[tauri::command]
pub async fn uninstall_content_pack(
    db_state: State<'_, DbState>,
    pack_state: State<'_, ContentPackState>,
    collection: String,
    pack_id: String,
) -> Result<(), String> {
    // A running installer would race this delete (its staging rename can
    // repopulate the dir we just removed, or we can yank the dir out from
    // under its "remove old install" step).
    {
        let jobs = pack_state.0.read().await;
        let key = format!("{}:{}", collection, pack_id);
        if let Some(job) = jobs.get(&key) {
            if !job.finished {
                return Err("Install in progress - cancel it first.".to_string());
            }
        }
    }

    let (_data_dir, install_dir) = {
        let conn = db_state.lock()?;
        let data_dir = crate::commands::games::configured_data_dir(&conn)?;

        let install_dir = match load_manifest() {
            Ok(manifest) => {
                let packs = manifest
                    .collections
                    .get(&collection)
                    .map(|c| &c.content_packs)
                    .cloned()
                    .unwrap_or_default();
                resolve_pack_install_dir(&data_dir, &collection, &pack_id, &packs)
            }
            Err(_) => {
                Path::new(&data_dir)
                    .join("content")
                    .join(&pack_id)
                    .join(&collection)
            }
        };
        (data_dir, install_dir)
    };

    // Filesystem removal can be slow for large packs - run off the command handler thread.
    if install_dir.exists() {
        let dir = install_dir.clone();
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir))
            .await
            .map_err(|e| format!("Uninstall task panicked: {}", e))?
            .map_err(|e| format!("Failed to uninstall: {}", e))?;
        log::info!("Uninstalled content pack: {}/{}", collection, pack_id);
    }

    let conn = db_state.lock()?;
    mark_pack_uninstalled(&conn, &collection, &pack_id)?;
    Ok(())
}

// ── Cancel an in-flight download ─────────────────────────────────────────────

#[tauri::command]
pub async fn cancel_content_pack_install(
    state: State<'_, ContentPackState>,
    collection: String,
    pack_id: String,
) -> Result<(), String> {
    let key = format!("{}:{}", collection, pack_id);
    let jobs = state.0.read().await;
    if let Some(job) = jobs.get(&key) {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

// ── Startup cleanup of stale download artifacts ──────────────────────────────

/// Startup: uninstall packs below the manifest's `min_compatible_version`
/// (a layout change). Fails open.
pub fn cleanup_stale_content_packs(conn: &rusqlite::Connection, data_dir: &Path) {
    let Ok(manifest) = load_manifest() else {
        log::debug!("cleanup_stale_content_packs: manifest unavailable, skipping");
        return;
    };
    cleanup_stale_content_packs_with(conn, data_dir, &manifest);
}

/// Split out so the compatibility rule can be tested without a manifest file.
fn cleanup_stale_content_packs_with(
    conn: &rusqlite::Connection,
    data_dir: &Path,
    manifest: &crate::commands::updates::Manifest,
) {
    let installed = read_installed_packs(conn);
    let data_dir_str = data_dir.to_string_lossy().to_string();

    let mut removed = 0usize;
    for (col_id, col_packs) in &installed {
        let Some(col_manifest) = manifest.collections.get(col_id) else { continue };
        for (pack_id, installed_pack) in col_packs {
            let Some(info) = col_manifest.content_packs.get(pack_id) else { continue };
            // A newer manifest version is an update offer, not a deletion.
            if installed_pack.version >= info.min_compatible_version {
                continue;
            }
            let install_path = resolve_pack_install_dir(
                &data_dir_str,
                col_id,
                pack_id,
                &col_manifest.content_packs,
            );
            log::info!(
                "Removing incompatible content pack {}/{} (installed v{}, minimum v{})",
                col_id, pack_id, installed_pack.version, info.min_compatible_version
            );
            if install_path.exists() {
                if let Err(e) = std::fs::remove_dir_all(&install_path) {
                    log::warn!(
                        "Failed to remove stale pack dir {}: {}",
                        install_path.display(), e
                    );
                    continue;
                }
            }
            if let Err(e) = mark_pack_uninstalled(conn, col_id, pack_id) {
                log::warn!("Failed to clear installed_packs record for {}/{}: {}", col_id, pack_id, e);
            }
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("cleanup_stale_content_packs: removed {} stale pack(s)", removed);
    }
}

/// Called once from lib.rs setup closure. Removes .tmp and .staging leftovers
/// from interrupted installs that are older than 1 hour.
pub fn cleanup_stale_downloads(data_dir: &Path) {
    let downloads_dir = data_dir.join(".content-downloads");
    if !downloads_dir.is_dir() {
        return;
    }

    let one_hour = std::time::Duration::from_secs(3600);
    let now = std::time::SystemTime::now();

    if let Ok(entries) = std::fs::read_dir(&downloads_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".tmp") || name_str.ends_with(".staging") {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age > one_hour {
                                let path = entry.path();
                                if path.is_dir() {
                                    let _ = std::fs::remove_dir_all(&path);
                                } else {
                                    let _ = std::fs::remove_file(&path);
                                }
                                log::info!("Cleaned up stale download artifact: {}", name_str);
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Utility ──────────────────────────────────────────────────────────────────

/// Safely join a base path with a relative subpath, rejecting absolute paths
/// and parent-directory traversals. Prevents manifest entries from escaping the
/// data directory (important once HTTP manifest fetch lands in v0.2).
fn safe_join(base: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.starts_with('/') || relative.starts_with('\\') || relative.contains("..") {
        return Err(format!("Invalid path: {}", relative));
    }
    let candidate = base.join(relative);
    if !candidate.starts_with(base) {
        return Err(format!("Path escapes base directory: {}", relative));
    }
    Ok(candidate)
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

#[cfg(all(test, unix))]
mod copy_tests {
    use super::*;

    /// The EXDEV fallback must recreate symlinks: 86Box.app's Frameworks are
    /// symlink-heavy, and a materialized copy breaks the codesign seal.
    #[test]
    fn copy_dir_recursive_recreates_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("Frameworks")).unwrap();
        std::fs::write(src.join("Frameworks/lib.dylib"), b"x").unwrap();
        std::os::unix::fs::symlink("lib.dylib", src.join("Frameworks/lib.1.dylib")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_recursive(&src, &dst).unwrap();

        let link = dst.join("Frameworks/lib.1.dylib");
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink(), "symlink was materialized");
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            std::path::PathBuf::from("lib.dylib")
        );
    }
}

#[cfg(test)]
mod adopt_tests {
    use super::*;
    use crate::commands::updates::{CollectionManifest, ContentPackInfo};

    fn pack(install_path: &str) -> ContentPackInfo {
        ContentPackInfo {
            display_name: "Box Art".into(),
            description: String::new(),
            url: String::new(),
            sha256: String::new(),
            torrent_file_path: None,
            size_bytes: 42,
            version: 3,
            install_path: install_path.into(),
            supersedes: Vec::new(),
            min_compatible_version: 0,
            platforms: None,
        }
    }

    /// A data dir moved from another OS must not grow ledger rows for
    /// binaries this build can never run - adoption skips packs the current
    /// platform cannot see.
    #[test]
    fn does_not_adopt_a_pack_for_another_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        crate::db::queries::set_config(&conn, "data_dir", tmp.path().to_str().unwrap()).unwrap();

        let dir = tmp.path().join("content/emulators/dosbox-x");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DOSBox-X.AppImage"), b"x").unwrap();

        let mut p = pack("content/emulators/dosbox-x");
        // A platforms map with no entry for any real platform: invisible everywhere.
        p.platforms = Some(HashMap::new());
        let mut content_packs = HashMap::new();
        content_packs.insert("dosbox-x".to_string(), p);
        let col = CollectionManifest {
            torrent_infohash: String::new(),
            game_count: 0,
            content_packs,
        };

        adopt_packs_on_disk(&conn, "eXoWin9x", &col);

        assert!(read_installed_packs(&conn).get("eXoWin9x").is_none_or(|c| c.is_empty()));
    }

    fn manifest(install_path: &str) -> CollectionManifest {
        let mut content_packs = HashMap::new();
        content_packs.insert("posters".to_string(), pack(install_path));
        CollectionManifest {
            torrent_infohash: String::new(),
            game_count: 0,
            content_packs,
        }
    }

    /// factory_reset wipes the whole config table but keeps `content/` unless
    /// the user also asked for their game data to go. The packs it kept must
    /// not come back as "not installed" - 30 GB re-downloaded for nothing.
    #[test]
    fn adopts_a_pack_directory_the_ledger_forgot() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        crate::db::queries::set_config(&conn, "data_dir", tmp.path().to_str().unwrap()).unwrap();

        let dir = tmp.path().join("content/posters/eXoDOS");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("abc.jpg"), b"x").unwrap();

        adopt_packs_on_disk(&conn, "eXoDOS", &manifest("content/posters/eXoDOS"));

        let state = read_installed_packs(&conn);
        let entry = state.get("eXoDOS").and_then(|c| c.get("posters")).expect("adopted");
        // Adopted at the manifest version - version 0 would make
        // cleanup_stale_content_packs delete the directory on next start.
        assert_eq!(entry.version, 3);
    }

    fn full_manifest(col: CollectionManifest) -> crate::commands::updates::Manifest {
        let mut collections = HashMap::new();
        collections.insert("eXoDOS".to_string(), col);
        crate::commands::updates::Manifest {
            schema_version: 2,
            generated_at: String::new(),
            collections,
        }
    }

    fn scenario(installed_version: u32, floor: u32) -> (tempfile::TempDir, rusqlite::Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        crate::db::queries::set_config(&conn, "data_dir", tmp.path().to_str().unwrap()).unwrap();
        let dir = tmp.path().join("content/posters/eXoDOS");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("abc.jpg"), b"x").unwrap();
        mark_pack_installed(&conn, "eXoDOS", "posters", installed_version, 1).unwrap();

        let mut col = manifest("content/posters/eXoDOS");
        let p = col.content_packs.get_mut("posters").unwrap();
        p.version = 5;
        p.min_compatible_version = floor;
        cleanup_stale_content_packs_with(&conn, tmp.path(), &full_manifest(col));
        (tmp, conn)
    }

    /// An outdated pack still works - content packs are replaced whole, so the
    /// user decides when to spend the bandwidth. Deleting it left them with
    /// blurry covers and no explanation.
    #[test]
    fn keeps_an_outdated_but_compatible_pack() {
        let (tmp, conn) = scenario(3, 3);
        assert!(tmp.path().join("content/posters/eXoDOS").exists(), "usable pack must survive");
        assert!(read_installed_packs(&conn)["eXoDOS"].contains_key("posters"));
    }

    /// Below the floor the layout itself is wrong (v0.2 posters were
    /// shortcode-keyed and 404'd every tile), so it has to go.
    #[test]
    fn removes_a_pack_below_the_compatibility_floor() {
        let (tmp, conn) = scenario(2, 3);
        assert!(!tmp.path().join("content/posters/eXoDOS").exists(), "unusable pack must go");
        // The collection entry itself may be dropped once its last pack goes.
        let recorded = read_installed_packs(&conn);
        assert!(recorded.get("eXoDOS").is_none_or(|c| !c.contains_key("posters")));
    }

    #[test]
    fn ignores_a_missing_or_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        crate::db::queries::set_config(&conn, "data_dir", tmp.path().to_str().unwrap()).unwrap();
        std::fs::create_dir_all(tmp.path().join("content/posters/eXoDOS")).unwrap();

        adopt_packs_on_disk(&conn, "eXoDOS", &manifest("content/posters/eXoDOS"));

        assert!(read_installed_packs(&conn).get("eXoDOS").is_none_or(|c| c.is_empty()));
    }

    /// Every poster pack used to unpack over the shared `content/posters`,
    /// so installing one deleted the others' art while the ledger kept
    /// claiming all three were installed - "Remove" with nothing to remove.
    #[test]
    fn forgets_a_pack_whose_directory_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        crate::db::queries::set_config(&conn, "data_dir", tmp.path().to_str().unwrap()).unwrap();
        mark_pack_installed(&conn, "eXoDOS", "posters", 5, 1).unwrap();

        adopt_packs_on_disk(&conn, "eXoDOS", &manifest("content/posters/eXoDOS"));

        let recorded = read_installed_packs(&conn);
        assert!(
            recorded.get("eXoDOS").is_none_or(|c| !c.contains_key("posters")),
            "a pack with no files on disk must read as not installed"
        );
    }

    fn staging_with(entries: &[(&str, bool)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (name, is_dir) in entries {
            let p = tmp.path().join(name);
            if *is_dir {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::write(&p, b"x").unwrap();
            }
        }
        tmp
    }

    #[test]
    fn unwraps_an_archive_that_repeats_the_target_directory() {
        let staging = staging_with(&[("eXoDOS", true)]);
        let install = Path::new("/data/content/posters/eXoDOS");
        assert_eq!(unwrapped_source(staging.path(), install), staging.path().join("eXoDOS"));
    }

    /// The real tarballs open with `._eXoDOS`, the AppleDouble sidecar for the
    /// wrapper directory. `tar tzf` never shows it; the `tar` crate writes it.
    /// Counting it as content hid the wrapper and double-nested every pack.
    #[test]
    fn unwraps_past_the_appledouble_sidecar_of_the_wrapper() {
        let staging = staging_with(&[("._eXoDOS", false), ("eXoDOS", true), (".DS_Store", false)]);
        let install = Path::new("/data/content/posters/eXoDOS");
        assert_eq!(unwrapped_source(staging.path(), install), staging.path().join("eXoDOS"));
    }

    /// posters-eXoWin9x-v1 was tarred from inside its own directory, so its
    /// entries are `./<hash>.jpg`. That has to install just as cleanly.
    #[test]
    fn leaves_a_flat_archive_alone() {
        let staging = staging_with(&[("a.jpg", false), ("b.jpg", false)]);
        let install = Path::new("/data/content/posters/eXoWin9x");
        assert_eq!(unwrapped_source(staging.path(), install), staging.path());
    }

    /// A lone directory is only a wrapper if it repeats the target's name -
    /// otherwise it is the payload, and unwrapping would discard its siblings'
    /// structure (a metadata pack that ships only `Images/`).
    #[test]
    fn keeps_a_lone_directory_that_is_not_the_wrapper() {
        let staging = staging_with(&[("Images", true)]);
        let install = Path::new("/data/content/metadata/eXoWin9x");
        assert_eq!(unwrapped_source(staging.path(), install), staging.path());
    }
}
