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

struct ContentPackJob {
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

#[tauri::command]
pub fn list_content_packs(
    db_state: State<DbState>,
    collection: String,
) -> Result<Vec<ContentPackStatus>, String> {
    let manifest = load_manifest()?;
    let col = manifest
        .collections
        .get(&collection)
        .ok_or_else(|| format!("Unknown collection '{}'", collection))?;

    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
    let installed = read_installed_packs(&conn);
    let col_installed = installed.get(&collection);

    let mut result: Vec<ContentPackStatus> = col
        .content_packs
        .iter()
        .map(|(id, info)| {
            let inst = col_installed.and_then(|c| c.get(id));
            ContentPackStatus {
                id: id.clone(),
                display_name: info.display_name.clone(),
                description: info.description.clone(),
                size_bytes: info.size_bytes,
                version: info.version,
                supersedes: info.supersedes.clone(),
                available: !info.url.starts_with("TODO"),
                installed: inst.is_some(),
                installed_version: inst.map(|i| i.version),
            }
        })
        .collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

// ── Install a content pack ───────────────────────────────────────────────────

#[tauri::command]
pub async fn install_content_pack(
    app: AppHandle,
    db_state: State<'_, DbState>,
    pack_state: State<'_, ContentPackState>,
    collection: String,
    pack_id: String,
) -> Result<(), String> {
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
        .clone();
    let col_packs = col.content_packs.clone();

    // Guard: reject packs with placeholder URLs.
    if pack_info.url.starts_with("TODO") {
        return Err(format!("'{}' is not yet available for download.", pack_info.display_name));
    }

    let key = format!("{}:{}", collection, pack_id);

    // Resolve data_dir (fast DB read, lock released immediately).
    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured. Run setup first.")?
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

    // Clone handles for the spawned task — return immediately so the UI stays responsive.
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
    let available = fs2::available_space(data_dir)
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

    do_install(jobs, data_dir, pack_info, key, cancel).await
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
    use std::io::Read;

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

    let response = reqwest::get(&pack_info.url)
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

    while let Some(chunk_result) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp_file);
            return Err("Cancelled".to_string());
        }

        let chunk = chunk_result.map_err(|e| format!("Download error: {}", e))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| format!("Write error: {}", e))?;

        downloaded += chunk.len() as u64;

        // Update progress (not every chunk — throttle to avoid lock contention).
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

    let hash = format!("{:x}", hasher.finalize());
    if !pack_info.sha256.starts_with("TODO") && hash != pack_info.sha256 {
        let _ = std::fs::remove_file(&tmp_file);
        return Err(format!(
            "Checksum mismatch — expected {}, got {}. Download may be corrupted, please retry.",
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

    // Ensure parent exists.
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create install parent dir: {}", e))?;
    }

    // Remove any existing install dir (e.g. from a previous version).
    if install_dir.exists() {
        std::fs::remove_dir_all(&install_dir)
            .map_err(|e| format!("Cannot remove old install: {}", e))?;
    }

    // Atomic rename; fall back to copy+remove on EXDEV.
    if let Err(_) = std::fs::rename(&staging_dir, &install_dir) {
        // Cross-filesystem: copy then remove.
        copy_dir_recursive(&staging_dir, &install_dir)?;
        let _ = std::fs::remove_dir_all(&staging_dir);
    }

    // Clean up temp tarball.
    let _ = std::fs::remove_file(&tmp_file);

    log::info!(
        "Content pack installed: {} → {}",
        pack_info.display_name,
        install_dir.display()
    );
    Ok(())
}

/// Recursively copy a directory tree (fallback for cross-filesystem rename).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {}", dst.display(), e))?;
    for entry in
        std::fs::read_dir(src).map_err(|e| format!("readdir {}: {}", src.display(), e))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        let dest = dst.join(entry.file_name());
        if ft.is_dir() {
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
    collection: String,
    pack_id: String,
) -> Result<(), String> {
    let (data_dir, install_dir) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured")?;

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

    // Filesystem removal can be slow for large packs — run off the command handler thread.
    if install_dir.exists() {
        let dir = install_dir.clone();
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir))
            .await
            .map_err(|e| format!("Uninstall task panicked: {}", e))?
            .map_err(|e| format!("Failed to uninstall: {}", e))?;
        log::info!("Uninstalled content pack: {}/{}", collection, pack_id);
    }

    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
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

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", bytes / 1024)
    }
}
