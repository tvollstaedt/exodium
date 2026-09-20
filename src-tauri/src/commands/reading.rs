//! The Lesesaal: one issue at a time out of the Media Pack's archives (§19).
//!
//! Same road as the preview videos - `torrent::zip_range` over a torrent
//! stream - with two differences that the corpus forces: the issues are large
//! (222 MB median for a book, 1.8 GB at the top), so they stream to disk
//! instead of through a `Vec<u8>`, and the archives are few and huge, so the
//! central directory of each is read once per session and kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::db::reading as reading_db;
use crate::media_sources::{ensure_manager, media_source, MediaTorrentState};
use crate::models::{Article, Issue, Publication};
use crate::torrent::manager::DownloadManager;
use crate::torrent::zip_range::{self, OffsetReader, ZipEntry};

use super::media::OFFLINE_TOKEN;
use super::{DbState, TorrentState};

const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a freshly added torrent may take to become readable.
const INITIALIZING_TIMEOUT: Duration = Duration::from_secs(120);
/// A book can be 1.5 GB over a cold swarm; the video timeout would give up on
/// a fetch that is making progress.
const ENTRY_TIMEOUT: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, Serialize)]
pub struct ReadingStatus {
    /// "fetching" | "ready" | "none" | "error"
    pub phase: String,
    pub progress: f64,
    pub total_bytes: u64,
    pub path: Option<String>,
    pub error: Option<String>,
}

impl ReadingStatus {
    fn phase(phase: &str) -> Self {
        Self {
            phase: phase.to_string(),
            progress: 0.0,
            total_bytes: 0,
            path: None,
            error: None,
        }
    }

    fn ready(path: String) -> Self {
        let mut status = Self::phase("ready");
        status.progress = 1.0;
        status.path = Some(path);
        status
    }

    /// Offline is a state, not a failure - but it must not be remembered as
    /// "this issue cannot be had" (§14).
    fn offline() -> Self {
        let mut status = Self::phase("none");
        status.error = Some(OFFLINE_TOKEN.to_string());
        status
    }
}

struct ReadingJob {
    status: ReadingStatus,
    cancel: Arc<AtomicBool>,
}

/// `spawn_job` removes a job that ends with this, so a cancelled fetch leaves
/// no status behind for the frontend to poll.
const CANCELLED: &str = "cancelled";

/// Every phase before the first byte is written can outlast the user's
/// patience: the initializing wait runs to 120 s and the central directory to
/// 45 s, so each await is followed by this.
fn check_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCELLED.to_string());
    }
    Ok(())
}

type JobMap = Arc<RwLock<HashMap<String, ReadingJob>>>;

/// A zip's central directory plus the window it was read through: `0`/size of
/// the torrent file for a flat archive, the STORED inner archive's data range
/// for a nested one. Every read replays that window, so the base has to be
/// cached with the entries - deriving it again costs the outer directory.
struct ArchiveDirectory {
    entries: Vec<ZipEntry>,
    base: u64,
    len: u64,
}

/// Central directories, keyed by archive path and inner archive. Reading one
/// costs a tail read plus up to 64 MB of directory; the magazines archive
/// alone has 48,897 entries.
type DirectoryCache = Arc<RwLock<HashMap<String, Arc<ArchiveDirectory>>>>;

#[derive(Default)]
pub struct ReadingState {
    jobs: JobMap,
    directories: DirectoryCache,
}

impl ReadingState {
    pub fn new() -> Self {
        Self::default()
    }
}

// ── Cache ────────────────────────────────────────────────────────────────────

/// No leading dot: the asset-protocol scope skips hidden components.
fn cache_dir(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("content").join("magazinecache")
}

/// Names a file that outlives the process, so the digest has to be one whose
/// algorithm is pinned: `DefaultHasher` is unspecified across Rust releases
/// and a toolchain bump would orphan every cached document.
fn cache_stem(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The cached file keeps the entry's own extension - the reader picks its
/// renderer from it, and on Linux the media server derives the Content-Type
/// the same way.
fn cache_file(data_dir: &str, issue: &Issue) -> PathBuf {
    let ext = issue
        .entry_path
        .as_deref()
        .and_then(|p| Path::new(p).extension())
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "pdf".to_string());
    cache_dir(data_dir).join(format!("{}.{}", cache_stem(&issue.key), ext))
}

fn cached_file(data_dir: &str, issue: &Issue) -> Option<PathBuf> {
    let path = cache_file(data_dir, issue);
    path.is_file().then_some(path)
}

/// A downloaded issue stays until the user removes it (§19); the one thing
/// swept at startup is the leftover of a fetch killed mid-write - a truncated
/// document would otherwise be served forever, and a disk magazine's shards
/// sit in the game root where nothing else ever looks at them.
pub fn sweep_partial_downloads(data_dir: &str) {
    if let Ok(entries) = std::fs::read_dir(cache_dir(data_dir)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("part") {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    let magazines = super::paths::game_root(data_dir).join("eXo").join("Magazines");
    for entry in walkdir::WalkDir::new(&magazines).into_iter().flatten() {
        if entry.file_type().is_file()
            && entry.file_name().to_string_lossy().ends_with(".exodium-part")
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Asked only of a path that exists, so both sides canonicalize: a symlinked
/// data dir still matches and no catalogue path can point a delete out of the
/// tree.
fn under_data_dir(path: &Path, data_dir: &str) -> bool {
    let (Ok(target), Ok(root)) = (std::fs::canonicalize(path), std::fs::canonicalize(data_dir))
    else {
        return false;
    };
    target.starts_with(root)
}

/// What a downloaded issue occupies: one cached document, or the directory a
/// runnable issue was extracted into. `issue_dir` is that issue's own; a
/// series' `launch_dir` also holds its siblings and is left standing (§19).
fn remove_issue_files(issue: &Issue, data_dir: &str) -> Result<(), String> {
    let target = if issue.runnable {
        let Some(launch_dir) = issue.launch_dir.as_deref() else { return Ok(()) };
        super::paths::game_root(data_dir).join(issue.issue_dir.as_deref().unwrap_or(launch_dir))
    } else {
        cache_file(data_dir, issue)
    };
    // Nothing on disk is not an error: a card can outlive its file.
    if !target.exists() {
        return Ok(());
    }
    if !under_data_dir(&target, data_dir) {
        return Err(format!(
            "Refusing to remove a path outside the data directory: {}",
            target.display()
        ));
    }
    let removed = if target.is_dir() {
        std::fs::remove_dir_all(&target)
    } else {
        std::fs::remove_file(&target)
    };
    removed.map_err(|e| format!("Could not remove {}: {e}", target.display()))
}

// ── Catalogue ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_publications(
    db_state: State<'_, DbState>,
    kind: Option<String>,
) -> Result<Vec<Publication>, String> {
    let conn = db_state.lock()?;
    reading_db::fetch_publications(&conn, kind.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_issues(
    db_state: State<'_, DbState>,
    kind: Option<String>,
    publication_id: Option<i64>,
    query: Option<String>,
) -> Result<Vec<Issue>, String> {
    let conn = db_state.lock()?;
    reading_db::fetch_issues(&conn, kind.as_deref(), publication_id, query.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_issue(db_state: State<'_, DbState>, key: String) -> Result<Option<Issue>, String> {
    let conn = db_state.lock()?;
    reading_db::fetch_issue_by_key(&conn, &key).map_err(|e| e.to_string())
}

/// eXo's article index for one game. Empty unless one of the indexed
/// publications covered it.
#[tauri::command]
pub async fn game_articles(db_state: State<'_, DbState>, id: i64) -> Result<Vec<Article>, String> {
    let conn = db_state.lock()?;
    let Some(game) = crate::db::queries::fetch_game_by_id(&conn, id).map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    let Some(shortcode) = game.shortcode else {
        return Ok(Vec::new());
    };
    reading_db::articles_for_shortcode(&conn, &shortcode).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_issue_favorited(
    db_state: State<'_, DbState>,
    key: String,
    favorited: bool,
) -> Result<(), String> {
    let conn = db_state.lock()?;
    reading_db::set_issue_flag(&conn, &key, "favorited", favorited).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_issue_page(
    db_state: State<'_, DbState>,
    key: String,
    page: i64,
) -> Result<(), String> {
    let conn = db_state.lock()?;
    reading_db::set_last_page(&conn, &key, page).map_err(|e| e.to_string())
}

/// Which issues are already on disk. The cache is keyed by a hash of the
/// issue key, so the answer is one directory read matched against the
/// catalogue rather than a probe per row.
#[tauri::command]
pub async fn reading_cache_index(db_state: State<'_, DbState>) -> Result<Vec<String>, String> {
    let (data_dir, keys) = {
        let conn = db_state.lock()?;
        let data_dir = crate::commands::games::configured_data_dir(&conn)?;
        let mut stmt = conn
            .prepare("SELECT key FROM issues")
            .map_err(|e| e.to_string())?;
        let keys: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        (data_dir, keys)
    };
    let dir = cache_dir(&data_dir);
    let stems: std::collections::HashSet<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) != Some("part"))
                .filter_map(|e| e.path().file_stem()?.to_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(keys
        .into_iter()
        .filter(|key| stems.contains(&cache_stem(key)))
        .collect())
}

/// Give one downloaded issue's disk space back. Nothing sweeps the reading
/// room, so this is the only way a document or an extracted disk magazine
/// leaves the disk (§19).
#[tauri::command]
pub async fn remove_issue(
    db_state: State<'_, DbState>,
    reading_state: State<'_, ReadingState>,
    key: String,
) -> Result<(), String> {
    let (issue, data_dir) = issue_and_data_dir(&db_state, &key)?;
    // A fetch still running would rename its result back into place after the
    // removal, so it is stopped and forgotten before anything is deleted.
    if let Some(job) = reading_state.jobs.write().await.remove(&key) {
        job.cancel.store(true, Ordering::Relaxed);
    }
    let _ = std::fs::remove_file(cache_file(&data_dir, &issue).with_extension("part"));
    remove_issue_files(&issue, &data_dir)?;
    if issue.runnable {
        let conn = db_state.lock()?;
        reading_db::set_issue_installed(&conn, &key, false).map_err(|e| e.to_string())?;
    }
    log::info!("Lesesaal: {} removed from disk", issue.title);
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct ReadingUsage {
    pub issues: u64,
    pub bytes: u64,
}

/// What the reading room occupies. A runnable issue is sized from
/// `issues.size_bytes`, not from the disk: its files are extracted into the
/// shared game root, where a walk would count the collections' as well.
#[tauri::command]
pub async fn reading_storage_usage(db_state: State<'_, DbState>) -> Result<ReadingUsage, String> {
    let (data_dir, installed) = {
        let conn = db_state.lock()?;
        let data_dir = crate::commands::games::configured_data_dir(&conn)?;
        let installed = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(i.size_bytes), 0) FROM issues i \
                 JOIN issue_state s ON s.issue_key = i.key WHERE s.installed = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|e| e.to_string())?;
        (data_dir, installed)
    };
    let mut usage = ReadingUsage {
        issues: installed.0.max(0) as u64,
        bytes: installed.1.max(0) as u64,
    };
    if let Ok(entries) = std::fs::read_dir(cache_dir(&data_dir)) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || entry.path().extension().and_then(|e| e.to_str()) == Some("part") {
                continue;
            }
            usage.issues += 1;
            usage.bytes += meta.len();
        }
    }
    Ok(usage)
}

// ── Fetch ────────────────────────────────────────────────────────────────────

enum SourceManager {
    Ready(Arc<DownloadManager>),
    /// No session at all - offline, or setup never finished (§11).
    Offline,
    /// The source is a collection this install does not run.
    Missing,
}

/// The manager for an issue's source. A media source joins the session on
/// demand (§19); a collection's manager already exists or the collection is
/// not enabled - and only an empty `TorrentState` means offline.
async fn manager_for(
    torrent_state: &TorrentState,
    media_state: &MediaTorrentState,
    source: &str,
) -> SourceManager {
    if media_source(source).is_some() {
        return match ensure_manager(torrent_state, media_state, source).await {
            Some(manager) => SourceManager::Ready(manager),
            None => SourceManager::Offline,
        };
    }
    let managers = torrent_state.0.read().await;
    if managers.is_empty() {
        return SourceManager::Offline;
    }
    match managers.get(source) {
        Some(manager) => SourceManager::Ready(Arc::clone(manager)),
        None => SourceManager::Missing,
    }
}

/// The collection an issue needs is not enabled, so there is nothing to read
/// from - an error, not a state: the reader's own retry covers it.
fn missing_source(issue: &Issue) -> String {
    format!(
        "{} is in the {} torrent, which this install has not enabled",
        issue.title, issue.source
    )
}

#[tauri::command]
pub async fn open_issue(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    media_state: State<'_, MediaTorrentState>,
    reading_state: State<'_, ReadingState>,
    key: String,
) -> Result<ReadingStatus, String> {
    let (issue, data_dir) = {
        let conn = db_state.lock()?;
        let issue = reading_db::fetch_issue_by_key(&conn, &key)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Issue {key} not found"))?;
        let data_dir = crate::commands::games::configured_data_dir(&conn)?;
        (issue, data_dir)
    };
    let Some(entry_path) = issue.entry_path.clone() else {
        // The disk magazines are launched, not read (§19 Phase 6).
        return Err(format!("{} is a runnable issue, not a document", issue.title));
    };

    if let Some(path) = cached_file(&data_dir, &issue) {
        return Ok(ReadingStatus::ready(super::paths::path_to_fwd_slash(&path)));
    }

    let manager = match manager_for(&torrent_state, &media_state, &issue.source).await {
        SourceManager::Ready(manager) => manager,
        SourceManager::Offline => return Ok(ReadingStatus::offline()),
        SourceManager::Missing => return Err(missing_source(&issue)),
    };
    let archive = manager
        .index()
        .find_by_path(&issue.zip_file)
        .ok_or_else(|| format!("{} is not in the {} torrent", issue.zip_file, issue.source))?
        .clone();

    // The entry is written whole before it is served, so the disk has to hold
    // it outright - no torrent placeholder covers this one.
    // Clamped: a catalogue row carrying a negative size would wrap into an
    // impossible requirement and refuse every fetch.
    let needed = issue.size_bytes.max(0) as u64 + 256 * 1024 * 1024;
    if let Ok(free) = fs4::available_space(Path::new(&data_dir)) {
        if free < needed {
            return Err(format!(
                "Not enough disk space: {} needs {:.1} GB, {:.1} GB free",
                issue.title,
                needed as f64 / 1e9,
                free as f64 / 1e9
            ));
        }
    }

    let jobs = Arc::clone(&reading_state.jobs);
    let directories = Arc::clone(&reading_state.directories);
    let target = cache_file(&data_dir, &issue);
    let job_key = key.clone();
    let title = issue.title.clone();
    let inner_zip = issue.inner_zip.clone();
    Ok(spawn_job(
        &reading_state.jobs,
        &key,
        issue.size_bytes.max(0) as u64,
        title,
        move |cancel| async move {
            fetch_issue(
                &manager,
                &directories,
                &jobs,
                &job_key,
                &archive,
                inner_zip.as_deref(),
                &entry_path,
                &target,
                &cancel,
            )
            .await
        },
    )
    .await)
}

#[tauri::command]
pub async fn get_issue_status(
    reading_state: State<'_, ReadingState>,
    key: String,
) -> Result<Option<ReadingStatus>, String> {
    Ok(reading_state
        .jobs
        .read()
        .await
        .get(&key)
        .map(|job| job.status.clone()))
}

#[tauri::command]
pub async fn cancel_issue_fetch(
    reading_state: State<'_, ReadingState>,
    key: String,
) -> Result<(), String> {
    if let Some(job) = reading_state.jobs.read().await.get(&key) {
        job.cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

// ── Disk magazines ───────────────────────────────────────────────────────────

/// Fetch a runnable issue's files into the game root: its own directory, or
/// the whole series directory for the three series that keep every issue in
/// one (§19). The series launcher at the Magazines root comes along, so the
/// tree matches what eXo's own installer leaves behind.
#[tauri::command]
pub async fn install_issue(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    media_state: State<'_, MediaTorrentState>,
    reading_state: State<'_, ReadingState>,
    key: String,
) -> Result<ReadingStatus, String> {
    let (issue, data_dir) = issue_and_data_dir(&db_state, &key)?;
    let launch_dir = runnable_launch_dir(&issue)?;
    let torrent_root = super::paths::game_root(&data_dir);

    if issue.installed && torrent_root.join(&launch_dir).join("dosbox.conf").is_file() {
        return Ok(ReadingStatus::ready(super::paths::path_to_fwd_slash(
            &torrent_root.join(&launch_dir),
        )));
    }
    let manager = match manager_for(&torrent_state, &media_state, &issue.source).await {
        SourceManager::Ready(manager) => manager,
        SourceManager::Offline => return Ok(ReadingStatus::offline()),
        SourceManager::Missing => return Err(missing_source(&issue)),
    };
    let archive = manager
        .index()
        .find_by_path(&issue.zip_file)
        .ok_or_else(|| format!("{} is not in the {} torrent", issue.zip_file, issue.source))?
        .clone();

    // The archive streams past, only the subtree is written out.
    let needed = (issue.size_bytes.max(0) as f64 * 1.2) as u64;
    if let Ok(free) = fs4::available_space(Path::new(&data_dir)) {
        if free < needed {
            return Err(format!(
                "Not enough disk space: {} needs {:.1} GB, {:.1} GB free",
                issue.title,
                needed as f64 / 1e9,
                free as f64 / 1e9
            ));
        }
    }

    // One issue is its own directory; the series' conf and launcher sit beside
    // it and are taken flat, never the sibling issues (a Big Blue Disk issue is
    // 0.3 MB, the series 210 MB).
    let own_dir = issue.issue_dir.clone().unwrap_or_else(|| launch_dir.clone());
    let shallow: Vec<String> = if own_dir == launch_dir {
        Vec::new()
    } else {
        vec![launch_dir.clone()]
    };
    let prefixes = vec![own_dir];
    // A series launcher stands above its directory, so it is asked for by name.
    let files: Vec<String> = match (&issue.launch_bat, launch_dir.rsplit_once('/')) {
        (Some(bat), Some((parent, _))) if parent.ends_with("/Magazines") || parent == "eXo/Magazines" => {
            vec![format!("{parent}/{bat}")]
        }
        _ => Vec::new(),
    };

    let directories = Arc::clone(&reading_state.directories);
    let jobs = Arc::clone(&reading_state.jobs);
    let title = issue.title.clone();
    let issue_key = issue.key.clone();
    let dest = torrent_root.clone();
    let ready_path = super::paths::path_to_fwd_slash(&torrent_root.join(&launch_dir));
    let inner_zip = issue.inner_zip.clone();
    Ok(spawn_job(
        &reading_state.jobs,
        &key,
        issue.size_bytes.max(0) as u64,
        title.clone(),
        move |cancel| async move {
            fetch_subtree(
                &manager,
                &directories,
                &jobs,
                &issue_key,
                &archive,
                inner_zip.as_deref(),
                &prefixes,
                &files,
                &shallow,
                &dest,
                &cancel,
            )
            .await?;
            let db = app.state::<DbState>();
            let conn = db.lock()?;
            reading_db::set_issue_installed(&conn, &issue_key, true).map_err(|e| e.to_string())?;
            drop(conn);
            log::info!("Lesesaal: {} installed", title);
            Ok(ready_path)
        },
    )
    .await)
}

/// Run a disk magazine the way eXo's own launcher does: its series
/// `dosbox.conf` from the game root, with the issue selected through the
/// placeholders in `run.bat` (§19).
#[tauri::command]
pub async fn launch_issue(
    app: AppHandle,
    db_state: State<'_, DbState>,
    key: String,
) -> Result<String, String> {
    let (issue, data_dir) = issue_and_data_dir(&db_state, &key)?;
    let launch_dir = runnable_launch_dir(&issue)?;
    if !issue.installed {
        return Err(format!("{} is not installed. Download it first.", issue.title));
    }

    let torrent_root = super::paths::game_root(&data_dir);
    let dir = torrent_root.join(&launch_dir);
    let conf = dir.join("dosbox.conf");
    if !conf.is_file() {
        return Err(format!(
            "{} is incomplete: {} is missing. Download it again.",
            issue.title,
            conf.display()
        ));
    }
    // eXo runs the launcher from the pack root, one level above Magazines.
    let working_dir = torrent_root.join(launch_dir.split('/').next().unwrap_or("eXo"));

    // Keyed on the series, not the issue: `run.bat` is one file per
    // `launch_dir`, and launching a second issue would rewrite it under the
    // first one's emulator (§19).
    let run_key = issue_run_key(&launch_dir);
    if super::games::running_games()
        .lock()
        .map(|s| s.contains(&run_key))
        .unwrap_or(false)
    {
        return Err(format!(
            "Another issue of '{}' is already running. Close it first.",
            issue.publication
        ));
    }

    write_issue_launcher(&dir, issue.substitutions.as_deref())?;

    // The catalogue names no emulator variant for an issue, so this answers
    // Staging everywhere - asked through resolve_engine all the same (§9).
    let bin = super::games::resolve_engine(None, &torrent_root, &Default::default())
        .unwrap_or_else(|| super::games::resolve_dosbox(&app));
    let patched = super::games::patch_dosbox_conf(&conf, &working_dir, None, true)?;
    super::games::ensure_dosbox_shaders(&app);

    let mut cmd = std::process::Command::new(&bin);
    cmd.current_dir(&working_dir).arg("-conf").arg(&patched);
    log::info!("Lesesaal: launching {} with {}", issue.title, conf.display());
    super::games::spawn_emulator_and_track(
        &app,
        cmd,
        &bin,
        &run_key,
        &issue.title,
        issue_run_id(&issue.key),
    )
}

fn issue_and_data_dir(db_state: &State<'_, DbState>, key: &str) -> Result<(Issue, String), String> {
    let conn = db_state.lock()?;
    let issue = reading_db::fetch_issue_by_key(&conn, key)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Issue {key} not found"))?;
    let data_dir = crate::commands::games::configured_data_dir(&conn)?;
    Ok((issue, data_dir))
}

fn runnable_launch_dir(issue: &Issue) -> Result<String, String> {
    if !issue.runnable {
        return Err(format!("{} is a document, not a disk magazine", issue.title));
    }
    issue
        .launch_dir
        .clone()
        .ok_or_else(|| format!("{} has no launcher directory", issue.title))
}

/// What `running_games` tracks a disk magazine by: its series directory, the
/// scope of the `run.bat` a launch rewrites.
fn issue_run_key(launch_dir: &str) -> String {
    format!("issue:{launch_dir}")
}

/// The id the running-game tracking and the `game-exited` event use for an
/// issue. Negative, so it can never name a games row.
fn issue_run_id(key: &str) -> i64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    -((hasher.finish() & 0x7fff_ffff) as i64) - 1
}

/// Fill `run.bat`'s placeholders for this issue. The pristine text is kept as
/// `run.exotpl` and `run.bat` is derived from it on every launch, so no state
/// has to be restored after the emulator exits (§19).
fn write_issue_launcher(dir: &Path, substitutions: Option<&str>) -> Result<(), String> {
    let Some(raw) = substitutions.filter(|s| !s.trim().is_empty()) else {
        return Ok(());
    };
    let subs: std::collections::BTreeMap<String, String> =
        serde_json::from_str(raw).map_err(|e| format!("Unreadable launcher substitutions: {e}"))?;
    if subs.is_empty() {
        return Ok(());
    }

    let bat = dir.join("run.bat");
    let template = dir.join("run.exotpl");
    if !template.is_file() {
        std::fs::copy(&bat, &template)
            .map_err(|e| format!("Failed to keep {}: {e}", template.display()))?;
    }
    let mut text = std::fs::read_to_string(&template)
        .map_err(|e| format!("Failed to read {}: {e}", template.display()))?;
    for (placeholder, value) in &subs {
        text = text.replace(placeholder, value);
    }
    std::fs::write(&bat, text).map_err(|e| format!("Failed to write {}: {e}", bat.display()))
}

/// Register a fetch job and run it in the background, so the command returns
/// the moment the transfer starts. The job's own key is what the frontend
/// polls with `get_issue_status`.
///
/// A job already in flight under this key is JOINED, not started again, and
/// the test-and-insert happens under one lock: two of them write the same
/// temporary file and rename it out from under each other.
async fn spawn_job<F, Fut>(
    jobs: &JobMap,
    key: &str,
    total_bytes: u64,
    title: String,
    run: F,
) -> ReadingStatus
where
    F: FnOnce(Arc<AtomicBool>) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let mut status = ReadingStatus::phase("fetching");
    status.total_bytes = total_bytes;
    {
        let mut guard = jobs.write().await;
        if let Some(running) = guard.get(key).filter(|job| job.status.phase == "fetching") {
            return running.status.clone();
        }
        guard.insert(
            key.to_string(),
            ReadingJob {
                status: status.clone(),
                cancel: Arc::clone(&cancel),
            },
        );
    }

    let task = run(cancel);
    let jobs = Arc::clone(jobs);
    let job_key = key.to_string();
    tauri::async_runtime::spawn(async move {
        let result = task.await;
        let mut guard = jobs.write().await;
        let Some(job) = guard.get_mut(&job_key) else { return };
        match result {
            Ok(path) => job.status = ReadingStatus::ready(path),
            Err(e) if e == CANCELLED => {
                guard.remove(&job_key);
            }
            Err(e) => {
                log::warn!("Fetching {} failed: {}", title, e);
                job.status.phase = "error".into();
                job.status.error = Some(e);
            }
        }
    });
    status
}

#[allow(clippy::too_many_arguments)]
async fn fetch_subtree(
    manager: &Arc<DownloadManager>,
    directories: &DirectoryCache,
    jobs: &JobMap,
    key: &str,
    archive: &crate::torrent::TorrentFileEntry,
    inner_zip: Option<&str>,
    prefixes: &[String],
    files: &[String],
    // Directories taken one level deep only.
    shallow: &[String],
    dest_root: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<u64, String> {
    let dir = archive_directory(manager, directories, archive, inner_zip, cancel).await?;
    let entries = &dir.entries;
    // The full-file window is the identity, so both sources read the same way.
    let stream = open_stream(manager, archive.index, cancel).await?;
    let mut stream = OffsetReader::new(stream, dir.base, dir.len);
    let ticker = spawn_progress_ticker(
        Arc::clone(jobs),
        key.to_string(),
        Arc::clone(manager),
        archive.index,
    );

    let prefixes: Vec<&str> = prefixes.iter().map(String::as_str).collect();
    let flat: Vec<&str> = shallow
        .iter()
        .flat_map(|dir| {
            let prefix = format!("{}/", dir.trim_end_matches('/'));
            entries.iter().filter_map(move |e| {
                let rest = e.name.strip_prefix(&prefix)?;
                (!rest.is_empty() && !rest.contains('/')).then_some(e.name.as_str())
            })
        })
        .collect();
    let files: Vec<&str> = files.iter().map(String::as_str).chain(flat).collect();
    let cancel_flag = Arc::clone(cancel);
    let result = tokio::time::timeout(
        ENTRY_TIMEOUT,
        zip_range::read_subtree_to_dir(
            &mut stream,
            entries,
            &prefixes,
            &files,
            dest_root,
            move |_, _| !cancel_flag.load(Ordering::Relaxed),
        ),
    )
    .await;
    ticker.abort();

    let outcome = match result {
        Ok(Ok(written)) => Ok(written),
        Ok(Err(e)) if e.to_string().contains(CANCELLED) => Err(CANCELLED.to_string()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out fetching the issue".to_string()),
    };
    let written = match outcome {
        Ok(written) => written,
        Err(message) => {
            // A timeout drops the extractor's future, so its own rollback
            // never runs; on the other paths this is a second, cheap pass.
            discard_partial_subtree(dest_root, &prefixes, shallow).await;
            return Err(message);
        }
    };
    log::info!(
        "Lesesaal: extracted {:.1} MB into {}",
        written as f64 / 1_048_576.0,
        dest_root.display()
    );
    Ok(written)
}

/// Take back what an interrupted install left in the game root: the issue's
/// own directory, plus the `.exodium-part` shards beside the series files. A
/// sibling's finished file was never this fetch's to remove (§19).
async fn discard_partial_subtree(dest_root: &Path, issue_dirs: &[&str], series_dirs: &[String]) {
    for dir in issue_dirs {
        let _ = tokio::fs::remove_dir_all(dest_root.join(dir)).await;
    }
    for dir in series_dirs {
        let Ok(mut entries) = tokio::fs::read_dir(dest_root.join(dir)).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_string_lossy().ends_with(".exodium-part") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn fetch_issue(
    manager: &Arc<DownloadManager>,
    directories: &DirectoryCache,
    jobs: &JobMap,
    key: &str,
    archive: &crate::torrent::TorrentFileEntry,
    inner_zip: Option<&str>,
    entry_path: &str,
    target: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<String, String> {
    let dir = archive_directory(manager, directories, archive, inner_zip, cancel).await?;
    let entry = dir
        .entries
        .iter()
        .find(|e| e.name == entry_path)
        .ok_or_else(|| format!("{entry_path} is not in {}", archive.path))?;

    {
        let mut guard = jobs.write().await;
        if let Some(job) = guard.get_mut(key) {
            job.status.total_bytes = entry.uncompressed_size;
        }
    }

    // The full-file window is the identity, so both sources read the same way.
    let stream = open_stream(manager, archive.index, cancel).await?;
    let mut stream = OffsetReader::new(stream, dir.base, dir.len);
    // Like the video fetch: the read parks on a whole 8 MiB piece, so the
    // honest progress is the archive's growth in the session, not the bytes
    // the reader has handed back (§14).
    let ticker = spawn_progress_ticker(
        Arc::clone(jobs),
        key.to_string(),
        Arc::clone(manager),
        archive.index,
    );

    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    // Write beside the target: a fetch killed mid-way must not leave a
    // truncated PDF that every later visit treats as cached.
    let temp = target.with_extension("part");
    let result = async {
        let mut file = tokio::fs::File::create(&temp).await.map_err(|e| e.to_string())?;
        let cancel_flag = Arc::clone(cancel);
        let written = tokio::time::timeout(
            ENTRY_TIMEOUT,
            zip_range::read_entry_to_writer(&mut stream, entry, &mut file, move |_, _| {
                !cancel_flag.load(Ordering::Relaxed)
            }),
        )
        .await
        .map_err(|_| "timed out fetching the issue".to_string())?
        .map_err(|e| {
            if e.to_string().contains(CANCELLED) {
                CANCELLED.to_string()
            } else {
                e.to_string()
            }
        })?;
        Ok::<u64, String>(written)
    }
    .await;
    ticker.abort();

    let written = match result {
        Ok(written) => written,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(e);
        }
    };
    tokio::fs::rename(&temp, target).await.map_err(|e| e.to_string())?;
    log::info!(
        "Lesesaal: {} extracted ({:.1} MB)",
        entry_path,
        written as f64 / 1_048_576.0
    );
    Ok(super::paths::path_to_fwd_slash(target))
}

/// A torrent that has just joined the session is not readable yet - librqbit
/// answers "invalid state: initializing" until its storage is open. The Media
/// Pack joins on the first issue opened (§19), so that state IS the normal
/// first experience: without this wait the first click always failed and the
/// second one worked. The wait is all it needs - the initial check finds
/// nothing selected and returns in milliseconds.
async fn open_stream(
    manager: &Arc<DownloadManager>,
    file_index: usize,
    cancel: &AtomicBool,
) -> Result<impl tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send, String> {
    let deadline = std::time::Instant::now() + INITIALIZING_TIMEOUT;
    loop {
        check_cancelled(cancel)?;
        match manager.stream_file(file_index).await {
            Ok(stream) => return Ok(stream),
            Err(e) if e.to_string().contains("initializing") && std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => return Err(format!("Could not open the archive stream: {e}")),
        }
    }
}

fn directory_key(archive_path: &str, inner: Option<&str>) -> String {
    match inner {
        Some(inner) => format!("{archive_path}#{inner}"),
        None => archive_path.to_string(),
    }
}

/// The outer entry a nested archive is read through. Only a STORED entry is a
/// byte-exact window onto its payload (§14).
fn inner_window<'a>(outer: &'a [ZipEntry], name: &str) -> Result<&'a ZipEntry, String> {
    let entry = outer
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("{name} is not in the archive"))?;
    if !entry.is_stored() {
        return Err(format!("{name} is not stored and cannot be read as a window"));
    }
    Ok(entry)
}

/// The archive's central directory and the window it lives in, read once per
/// session. 48,897 entries for the magazines archive, so this is worth
/// keeping; a nested archive also costs the outer directory to locate.
async fn archive_directory(
    manager: &Arc<DownloadManager>,
    directories: &DirectoryCache,
    archive: &crate::torrent::TorrentFileEntry,
    inner: Option<&str>,
    cancel: &AtomicBool,
) -> Result<Arc<ArchiveDirectory>, String> {
    let key = directory_key(&archive.path, inner);
    if let Some(dir) = directories.read().await.get(&key) {
        return Ok(Arc::clone(dir));
    }

    // One stream serves both directories; the outer one is only read when it
    // is not cached already.
    let mut stream = open_stream(manager, archive.index, cancel).await?;
    let outer_key = directory_key(&archive.path, None);
    let cached_outer = directories.read().await.get(&outer_key).map(Arc::clone);
    let outer = match cached_outer {
        Some(dir) => dir,
        None => {
            let entries = read_directory(&mut stream, archive.size).await?;
            check_cancelled(cancel)?;
            let dir = Arc::new(ArchiveDirectory {
                entries,
                base: 0,
                len: archive.size,
            });
            directories.write().await.insert(outer_key, Arc::clone(&dir));
            dir
        }
    };
    let Some(name) = inner else { return Ok(outer) };

    let entry = inner_window(&outer.entries, name)?;
    // One piece fetch: uncapped it waits forever (§14).
    let base = tokio::time::timeout(
        DIRECTORY_TIMEOUT,
        zip_range::entry_data_offset(&mut stream, entry),
    )
    .await
    .map_err(|_| format!("timed out locating {name}"))?
    .map_err(|e| e.to_string())?;
    check_cancelled(cancel)?;
    let len = entry.uncompressed_size;

    let entries = read_directory(&mut OffsetReader::new(stream, base, len), len).await?;
    check_cancelled(cancel)?;
    let dir = Arc::new(ArchiveDirectory { entries, base, len });
    directories.write().await.insert(key, Arc::clone(&dir));
    Ok(dir)
}

async fn read_directory<R>(reader: &mut R, len: u64) -> Result<Vec<ZipEntry>, String>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin,
{
    tokio::time::timeout(
        DIRECTORY_TIMEOUT,
        zip_range::read_central_directory(reader, len),
    )
    .await
    .map_err(|_| "timed out reading the archive index".to_string())?
    .map_err(|e| e.to_string())
}

fn spawn_progress_ticker(
    jobs: JobMap,
    key: String,
    manager: Arc<DownloadManager>,
    file_index: usize,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut baseline: Option<u64> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(progress) = manager.file_progress(file_index).await else { continue };
            let base = *baseline.get_or_insert(progress.downloaded_bytes);
            let got = progress.downloaded_bytes.saturating_sub(base);
            let mut guard = jobs.write().await;
            let Some(job) = guard.get_mut(&key) else { return };
            if job.status.phase != "fetching" || job.status.total_bytes == 0 {
                continue;
            }
            let fraction = (got as f64 / job.status.total_bytes as f64).min(0.99);
            if fraction > job.status.progress {
                job.status.progress = fraction;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_entry(name: &str, method: u16) -> ZipEntry {
        ZipEntry {
            name: name.into(),
            compressed_size: 10,
            uncompressed_size: 10,
            method,
            local_header_offset: 0,
        }
    }

    /// A nested archive is read as a byte window onto the outer one, which
    /// only holds for a STORED entry (§14).
    #[test]
    fn a_window_needs_a_stored_entry_that_exists() {
        let entries = vec![zip_entry("Content/inner.zip", 0), zip_entry("Content/packed.zip", 8)];
        assert_eq!(
            inner_window(&entries, "Content/inner.zip").unwrap().name,
            "Content/inner.zip"
        );

        let deflated = inner_window(&entries, "Content/packed.zip").unwrap_err();
        assert!(deflated.contains("not stored"), "{deflated}");
        let missing = inner_window(&entries, "Content/absent.zip").unwrap_err();
        assert!(missing.contains("Content/absent.zip"), "{missing}");
    }

    /// The nested directory describes a different archive than the outer one,
    /// so the two must never share a cache entry.
    #[test]
    fn a_nested_directory_is_keyed_apart_from_its_outer_one() {
        let outer = directory_key("Content/a.zip", None);
        let inner = directory_key("Content/a.zip", Some("Content/inner.zip"));
        assert_eq!(outer, "Content/a.zip");
        assert_ne!(outer, inner);
        assert_eq!(
            inner,
            directory_key("Content/a.zip", Some("Content/inner.zip"))
        );
    }

    #[test]
    fn cache_file_keeps_the_entrys_extension() {
        let issue = Issue {
            id: 1,
            key: "mag:a".into(),
            publication_id: 1,
            publication: "PC Gamer".into(),
            kind: "magazine".into(),
            title: "PC Gamer 1995-02".into(),
            sort_title: None,
            year: Some(1995),
            release_date: None,
            publisher: None,
            developer: None,
            notes: None,
            zip_file: "Content/DOSMagazines.zip".into(),
            entry_path: Some("eXo/Magazines/PCGamerUS/PCGamer_1995_02.PDF".into()),
            entry_kind: Some("pdf".into()),
            size_bytes: 10,
            cover_key: None,
            runnable: false,
            launch_dir: None,
            issue_dir: None,
            launch_bat: None,
            command_line: None,
            substitutions: None,
            favorited: false,
            installed: false,
            last_page: None,
            last_opened: None,
            source: "eXoMedia".into(),
            inner_zip: None,
            language: "EN".into(),
            extras_count: 0,
        };
        let path = cache_file("/data", &issue);
        assert_eq!(path.extension().unwrap(), "pdf");
        assert!(path.starts_with("/data/content/magazinecache"));
        // The name is derived from the key, not the title: two issues can
        // share a title across publications.
        assert_eq!(
            path.file_stem().unwrap().to_str().unwrap(),
            cache_stem("mag:a")
        );
    }

    /// Every launch derives `run.bat` from the pristine template, so running
    /// issue B after issue A must not inherit A's substitutions (§19).
    #[test]
    fn every_launch_rewrites_run_bat_from_the_template() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("run.bat"), "imgmount a disk\\XXX\\d1.img\nset OPT=YYY\n").unwrap();

        write_issue_launcher(dir.path(), Some(r#"{"XXX":"004","YYY":"OFF"}"#)).unwrap();
        let first = std::fs::read_to_string(dir.path().join("run.bat")).unwrap();
        assert_eq!(first, "imgmount a disk\\004\\d1.img\nset OPT=OFF\n");

        write_issue_launcher(dir.path(), Some(r#"{"XXX":"119","YYY":"ON"}"#)).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("run.bat")).unwrap(),
            "imgmount a disk\\119\\d1.img\nset OPT=ON\n"
        );
        // The template keeps the placeholders, which is what makes that work.
        assert!(std::fs::read_to_string(dir.path().join("run.exotpl")).unwrap().contains("XXX"));

        // The 49 issues eXo substitutes nothing for leave the file alone.
        write_issue_launcher(dir.path(), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("run.bat")).unwrap(),
            "imgmount a disk\\119\\d1.img\nset OPT=ON\n"
        );
    }

    fn issue(key: &str) -> Issue {
        Issue {
            id: 1,
            key: key.into(),
            publication_id: 1,
            publication: "Big Blue Disk".into(),
            kind: "magazine".into(),
            title: key.into(),
            sort_title: None,
            year: None,
            release_date: None,
            publisher: None,
            developer: None,
            notes: None,
            zip_file: "Content/DOSMagazines.zip".into(),
            entry_path: Some("eXo/Magazines/BBD/x.pdf".into()),
            entry_kind: Some("pdf".into()),
            size_bytes: 10,
            cover_key: None,
            runnable: false,
            launch_dir: None,
            issue_dir: None,
            launch_bat: None,
            command_line: None,
            substitutions: None,
            favorited: false,
            installed: false,
            last_page: None,
            last_opened: None,
            source: "eXoMedia".into(),
            inner_zip: None,
            language: "EN".into(),
            extras_count: 0,
        }
    }

    /// Nothing evicts a downloaded issue any more, but a fetch that died
    /// mid-write still must not survive as a truncated document.
    #[test]
    fn the_startup_sweep_takes_partials_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_string_lossy().to_string();
        let cache = cache_dir(&data_dir);
        std::fs::create_dir_all(&cache).unwrap();
        let body = vec![0u8; 1024];
        for name in ["one.pdf", "two.pdf"] {
            std::fs::write(cache.join(name), &body).unwrap();
        }
        std::fs::write(cache.join("half.part"), &body).unwrap();

        sweep_partial_downloads(&data_dir);
        assert!(!cache.join("half.part").exists());
        assert!(cache.join("one.pdf").exists());
        assert!(cache.join("two.pdf").exists());
    }

    #[test]
    fn removing_a_document_takes_its_cache_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_string_lossy().to_string();
        let cache = cache_dir(&data_dir);
        std::fs::create_dir_all(&cache).unwrap();
        let (a, b) = (issue("mag:a"), issue("mag:b"));
        std::fs::write(cache_file(&data_dir, &a), b"a").unwrap();
        std::fs::write(cache_file(&data_dir, &b), b"b").unwrap();

        remove_issue_files(&a, &data_dir).unwrap();
        assert!(!cache_file(&data_dir, &a).exists());
        assert!(cache_file(&data_dir, &b).exists());
        // A card can outlive its file; removing again is not an error.
        remove_issue_files(&a, &data_dir).unwrap();
    }

    /// A series keeps every issue in one directory, so only `issue_dir` is
    /// this issue's own; the loose launcher files belong to all of them.
    #[test]
    fn removing_a_series_issue_keeps_the_series_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_string_lossy().to_string();
        let root = super::super::paths::game_root(&data_dir);
        std::fs::create_dir_all(root.join("eXo/Magazines/BBD/004")).unwrap();
        std::fs::write(root.join("eXo/Magazines/BBD/dosbox.conf"), b"conf").unwrap();
        std::fs::write(root.join("eXo/Magazines/BBD/004/d1.img"), b"img").unwrap();

        let mut bbd = issue("mag:bbd#004");
        bbd.runnable = true;
        bbd.entry_path = None;
        bbd.launch_dir = Some("eXo/Magazines/BBD".into());
        bbd.issue_dir = Some("eXo/Magazines/BBD/004".into());

        remove_issue_files(&bbd, &data_dir).unwrap();
        assert!(!root.join("eXo/Magazines/BBD/004").exists());
        assert!(root.join("eXo/Magazines/BBD/dosbox.conf").exists());
    }

    /// The cache name outlives the process and the toolchain that wrote it,
    /// so it is pinned here: a changed digest orphans every downloaded issue.
    #[test]
    fn the_cache_stem_is_a_stable_digest() {
        assert_eq!(cache_stem("mag:pcgamer#1995-02"), "1c3a6db6c201a03a");
        assert_eq!(cache_stem("mag:a").len(), 16);
        assert_ne!(cache_stem("mag:a"), cache_stem("mag:b"));
    }

    /// A disk magazine's shards land in the game root, where nothing but this
    /// sweep ever looks for them.
    #[test]
    fn the_startup_sweep_also_takes_extraction_shards() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().to_string_lossy().to_string();
        let bbd = super::super::paths::game_root(&data_dir).join("eXo/Magazines/BBD/004");
        std::fs::create_dir_all(&bbd).unwrap();
        std::fs::write(bbd.join("d1.img.exodium-part"), b"half").unwrap();
        std::fs::write(bbd.join("d1.img"), b"whole").unwrap();

        sweep_partial_downloads(&data_dir);
        assert!(!bbd.join("d1.img.exodium-part").exists());
        assert!(bbd.join("d1.img").exists());
    }

    /// `run.bat` is one file per series, so the guard has to be too - two
    /// issues of one magazine cannot run at once (§19).
    #[test]
    fn the_running_guard_is_keyed_on_the_series() {
        assert_eq!(
            issue_run_key("eXo/Magazines/BBD"),
            issue_run_key("eXo/Magazines/BBD")
        );
        assert_ne!(
            issue_run_key("eXo/Magazines/BBD"),
            issue_run_key("eXo/Magazines/GameBytes")
        );
    }

    /// Poll the job map the way the frontend does, with a ceiling so a stuck
    /// job fails the test instead of hanging it.
    async fn settle(jobs: &JobMap, done: impl Fn(&HashMap<String, ReadingJob>) -> bool) {
        for _ in 0..400 {
            if done(&*jobs.read().await) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the job never settled");
    }

    /// Two clicks on one issue share a fetch: a second would write the same
    /// temporary file and rename it out from under the first.
    #[tokio::test]
    async fn a_second_call_joins_the_fetch_in_flight() {
        let jobs: JobMap = Arc::new(RwLock::new(HashMap::new()));
        let gate = Arc::new(AtomicBool::new(false));
        let held = Arc::clone(&gate);
        let first = spawn_job(&jobs, "mag:a", 10, "A".into(), move |_| async move {
            while !held.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Ok("/cache/a.pdf".to_string())
        })
        .await;
        assert_eq!(first.phase, "fetching");

        let started_again = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&started_again);
        let second = spawn_job(&jobs, "mag:a", 10, "A".into(), move |_| async move {
            marker.store(true, Ordering::Relaxed);
            Ok("/cache/second.pdf".to_string())
        })
        .await;
        assert_eq!(second.phase, "fetching");
        assert!(!started_again.load(Ordering::Relaxed));
        assert_eq!(jobs.read().await.len(), 1);

        gate.store(true, Ordering::Relaxed);
        settle(&jobs, |map| map["mag:a"].status.phase == "ready").await;
    }

    /// A cancelled fetch leaves nothing to poll: the reader would otherwise
    /// keep showing a job that is never going to finish.
    #[tokio::test]
    async fn a_cancelled_job_is_forgotten() {
        let jobs: JobMap = Arc::new(RwLock::new(HashMap::new()));
        spawn_job(&jobs, "mag:b", 10, "B".into(), |cancel| async move {
            while !cancel.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(CANCELLED.to_string())
        })
        .await;
        jobs.read().await["mag:b"].cancel.store(true, Ordering::Relaxed);
        settle(&jobs, |map| map.is_empty()).await;
    }

    #[test]
    fn a_path_outside_the_data_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();

        let mut escaping = issue("mag:evil");
        escaping.runnable = true;
        escaping.entry_path = None;
        escaping.launch_dir = Some(outside.to_string_lossy().into_owned());

        let err = remove_issue_files(&escaping, &data_dir.to_string_lossy()).unwrap_err();
        assert!(err.contains("outside the data directory"), "{err}");
        assert!(outside.join("keep.txt").exists());
    }
}
