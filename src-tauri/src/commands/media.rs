//! Game preview videos and theme tracks.
//!
//! eXoDOS ships one MP4 per game inside that game's `GameData/<Title>.zip`,
//! next to the manual, and for about half the games one theme track under
//! `Music/MS-DOS/`. Those archives run from 2 MB to 1.1 GB, so playing a
//! 2.5 MB preview must not mean fetching the archive: `torrent::zip_range`
//! reads the archive's directory from its tail, then only the video's own
//! bytes, over a torrent stream that fetches pieces on demand. Measured on the
//! real catalogue: 27 MB pulled out of a 1163 MB archive.
//!
//! Fetching runs as a background job because a torrent read can block for a
//! minute waiting for peers, and the panel starts one automatically when a game
//! is opened - so it must be pollable and cancellable, exactly like downloads.
//!
//! Resolution order, cheapest first:
//!   1. the extracted cache from a previous call
//!   2. the archive already on disk (installed game, or a partial download
//!      that happens to cover the video)
//!   3. the torrent stream

use std::collections::HashMap;
use std::time::Duration;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tokio::sync::RwLock;

use crate::db::queries;
use crate::torrent::zip_range;

use super::{DbState, TorrentState};

// ── Job state ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct VideoStatus {
    /// "probing" | "fetching" | "ready" | "none" | "error"
    ///
    /// The split matters for the UI: "probing" means we are reading the
    /// archive's index and do not yet know whether a video exists at all, so
    /// there is nothing honest to announce. Only "fetching" means a video was
    /// found and its bytes are on the way.
    pub phase: String,
    /// 0..1 while fetching.
    pub progress: f64,
    /// Total bytes to transfer, so the UI can say "of 27 MB".
    pub total_bytes: u64,
    pub path: Option<String>,
    pub error: Option<String>,
}

/// The `error` token that marks a phase "none" which is NOT an inventory
/// answer: no torrent session exists (offline mode), so the archive was never
/// asked at all. "none" is otherwise permanent and the frontend caches it, so
/// this has to stay distinguishable - and it is a stable token the frontend
/// keys on (`phase == "none" && error != null`), not a message to show.
pub const OFFLINE_TOKEN: &str = "offline";

impl VideoStatus {
    fn phase(phase: &str) -> Self {
        Self {
            phase: phase.to_string(),
            progress: 0.0,
            total_bytes: 0,
            path: None,
            error: None,
        }
    }

    /// Offline is a legitimate state, not an error worth a red toast - hence
    /// phase "none" - but it must not be cached as "this archive has none".
    fn offline() -> Self {
        let mut status = Self::phase("none");
        status.error = Some(OFFLINE_TOKEN.to_string());
        status
    }
}

struct VideoJob {
    status: VideoStatus,
    cancel: Arc<AtomicBool>,
}

type JobMap = Arc<RwLock<HashMap<i64, VideoJob>>>;

/// Tauri-managed state for in-flight video fetches. The field is private
/// because `VideoJob` is - a `pub` field of a private type is an error under
/// `-D warnings`, and nothing outside this module ever needed the access.
pub struct VideoState(JobMap);

impl VideoState {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl Default for VideoState {
    fn default() -> Self {
        Self::new()
    }
}

/// In-flight theme-track fetches: the same job model as videos, in its own
/// map so a game's video and its music can be in flight at the same time.
pub struct MusicState(JobMap);

impl MusicState {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }
}

impl Default for MusicState {
    fn default() -> Self {
        Self::new()
    }
}

/// What is being pulled out of a GameData archive. The preview video and the
/// theme track sit in the same zip and travel the same road - index from the
/// archive's tail, one entry by offset, a file in the cache, a marker when
/// there is nothing - so the kind is a parameter of one pipeline rather than
/// a second copy of it. Only the finder, the cache folder and the file naming
/// differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Music,
}

impl MediaKind {
    fn label(self) -> &'static str {
        match self {
            MediaKind::Video => "video",
            MediaKind::Music => "music",
        }
    }

    fn cache_dir(self, data_dir: &str) -> PathBuf {
        // No leading dot - the asset-protocol scope glob skips hidden components,
        // which is why the first version served nothing (see setup::gallery_cache_dir).
        let name = match self {
            MediaKind::Video => "videocache",
            MediaKind::Music => "musiccache",
        };
        PathBuf::from(data_dir).join("content").join(name)
    }

    /// Marks an archive as having no entry of this kind.
    ///
    /// Whether a game has one is only knowable from the archive's own index,
    /// which sits at the end of a file in the torrent - so the answer costs a
    /// piece download (8 MB), every time, for a game that turns out to have
    /// nothing. The catalogue cannot help: its `MissingVideo` flag said "true"
    /// for 16 of the 24 sampled games that do have one, and `MissingMusic` is
    /// LaunchBox's default rather than an inventory. So the answer is written
    /// down and the question asked once per archive, ever.
    fn marker(self, data_dir: &str, collection: &str, file_index: usize) -> PathBuf {
        let ext = match self {
            MediaKind::Video => "novideo",
            MediaKind::Music => "nomusic",
        };
        self.cache_dir(data_dir).join(format!("{}_{}.{}", collection, file_index, ext))
    }

    /// Where a fetched entry is written. Videos are always `.mp4`; a track
    /// keeps its own extension, because tower-http's ServeFile derives the
    /// Content-Type from it and GStreamer picks its demuxer by that.
    fn cache_file(self, data_dir: &str, collection: &str, file_index: usize, entry_name: &str) -> PathBuf {
        let ext = match self {
            MediaKind::Video => "mp4".to_string(),
            MediaKind::Music => Path::new(entry_name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_else(|| "mp3".to_string()),
        };
        self.cache_dir(data_dir).join(format!("{}_{}.{}", collection, file_index, ext))
    }

    /// Whether a cache file is a fetched entry, as opposed to a marker or a
    /// half-written `.part`.
    fn is_payload(self, path: &Path) -> bool {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        match self {
            MediaKind::Video => ext == "mp4",
            MediaKind::Music => zip_range::is_music(&format!(".{}", ext)),
        }
    }

    /// The entry fetched on an earlier visit, if any.
    fn cached(self, data_dir: &str, collection: &str, file_index: usize) -> Option<PathBuf> {
        match self {
            MediaKind::Video => {
                let path = self.cache_file(data_dir, collection, file_index, "");
                path.is_file().then_some(path)
            }
            MediaKind::Music => {
                // The extension is the track's own, so the lookup is by stem.
                let stem = format!("{}_{}", collection, file_index);
                std::fs::read_dir(self.cache_dir(data_dir))
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| {
                        p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str())
                            && self.is_payload(p)
                            && p.is_file()
                    })
            }
        }
    }

    fn find(self, entries: &[zip_range::ZipEntry]) -> Option<&zip_range::ZipEntry> {
        match self {
            MediaKind::Video => zip_range::find_video(entries),
            MediaKind::Music => zip_range::find_music(entries),
        }
    }
}

// ── Cache ────────────────────────────────────────────────────────────────────

async fn write_cache(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    // Write beside the target first: a half-written MP4 left by a crash would
    // otherwise be served as a valid cache entry forever.
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, bytes).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, path).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Videos are 2-27 MB each, so browsing a few hundred games would otherwise
/// fill a disk quietly. Only the payload is pruned: the markers are empty
/// files that cost nothing and save a piece download each.
const VIDEO_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;
/// Themes are 1-8 MB, so the same cap keeps a few hundred tracks - enough for
/// the shuffle to become mostly free over time.
const MUSIC_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

pub fn prune_video_cache(data_dir: &str) {
    prune_media_cache(MediaKind::Video, data_dir, VIDEO_CACHE_MAX_BYTES);
}

pub fn prune_music_cache(data_dir: &str) {
    prune_media_cache(MediaKind::Music, data_dir, MUSIC_CACHE_MAX_BYTES);
}

fn prune_media_cache(kind: MediaKind, data_dir: &str, max_bytes: u64) {
    let dir = kind.cache_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !kind.is_payload(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        total += meta.len();
        files.push((path, meta.len(), meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
    }
    if total <= max_bytes {
        return;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let target = max_bytes / 5 * 4;
    let mut freed = 0u64;
    for (path, len, _) in files {
        if total - freed <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            freed += len;
        }
    }
    log::info!("{} cache pruned: {:.1} MB freed", kind.label(), freed as f64 / 1_048_576.0);
}

// ── Resolution ───────────────────────────────────────────────────────────────

/// The GameData archive that holds a game's extras: (torrent file index,
/// collection id).
///
/// Extras live in the English archive only: EVERY localized row has a NULL
/// gamedata index (DE 484/484, ES 413/413, PL 56/56), so a German selection
/// would otherwise report "nothing here" for a game that has a video and a
/// theme. The sibling lookup stays within the pack family: shortcodes are
/// unique per family, not globally, so an unqualified match would hand a
/// Win3x game the DOS game's archive when the codes collide.
pub(crate) fn resolve_gamedata(conn: &rusqlite::Connection, game: &crate::models::Game) -> (Option<i64>, String) {
    if let Some(idx) = game.gamedata_torrent_index {
        return (
            Some(idx),
            game.torrent_source.clone().unwrap_or_else(|| "eXoDOS".to_string()),
        );
    }
    let base = crate::commands::setup::collection_base_id(
        game.torrent_source.as_deref().unwrap_or("eXoDOS"),
    );
    let sibling = game.shortcode.as_deref().and_then(|sc| {
        conn.query_row(
            "SELECT g.gamedata_torrent_index, g.torrent_source FROM games g \
             WHERE g.shortcode = ?1 AND g.gamedata_torrent_index IS NOT NULL \
               AND COALESCE(g.torrent_source, 'eXoDOS') = ?2 \
             ORDER BY CASE WHEN g.language = 'EN' THEN 0 ELSE 1 END LIMIT 1",
            rusqlite::params![sc, base],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .ok()
    });
    match sibling {
        Some((idx, src)) => (idx, src.unwrap_or_else(|| "eXoDOS".to_string())),
        None => (None, "eXoDOS".to_string()),
    }
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// Kick off (or join) the fetch for a game's preview video. Returns
/// immediately; poll `get_video_status`.
#[tauri::command]
pub async fn start_game_video(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    video_state: State<'_, VideoState>,
    id: i64,
) -> Result<VideoStatus, String> {
    start_media(MediaKind::Video, &db_state, &torrent_state, &video_state.0, id).await
}

/// Same for the theme track; poll `get_music_status`.
#[tauri::command]
pub async fn start_game_music(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    music_state: State<'_, MusicState>,
    id: i64,
) -> Result<VideoStatus, String> {
    start_media(MediaKind::Music, &db_state, &torrent_state, &music_state.0, id).await
}

async fn start_media(
    kind: MediaKind,
    db_state: &DbState,
    torrent_state: &TorrentState,
    jobs: &JobMap,
    id: i64,
) -> Result<VideoStatus, String> {
    let (gamedata_idx, source, data_dir) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured")?;
        let (idx, source) = resolve_gamedata(&conn, &game);
        (idx, source, data_dir)
    };

    let Some(gamedata_idx) = gamedata_idx else {
        return Ok(VideoStatus::phase("none"));
    };
    let gamedata_idx = gamedata_idx as usize;

    // Asked before, answer was no - do not pay for the same piece twice.
    if kind.marker(&data_dir, &source, gamedata_idx).is_file() {
        return Ok(VideoStatus::phase("none"));
    }

    // Already extracted - the common case after the first visit.
    if let Some(cached) = kind.cached(&data_dir, &source, gamedata_idx) {
        let mut status = VideoStatus::phase("ready");
        status.progress = 1.0;
        status.path = Some(crate::commands::setup::path_to_fwd_slash(&cached));
        return Ok(status);
    }

    // Join an in-flight job rather than starting a second one.
    {
        let jobs = jobs.read().await;
        if let Some(job) = jobs.get(&id) {
            if job.status.phase == "probing" || job.status.phase == "fetching" {
                return Ok(job.status.clone());
            }
        }
    }

    let manager = {
        let guard = torrent_state.0.read().await;
        guard.get(&source).cloned()
    };
    let Some(manager) = manager else {
        // No manager means no session, i.e. offline. Same phase as "the archive
        // has no music/video", but carrying OFFLINE_TOKEN so the frontend does
        // not cache a temporary state as the archive's permanent answer.
        return Ok(VideoStatus::offline());
    };

    let file = manager
        .index()
        .files
        .get(gamedata_idx)
        .ok_or_else(|| format!("GameData index {} out of range", gamedata_idx))?
        .clone();

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = jobs.write().await;
        jobs.insert(
            id,
            VideoJob {
                status: VideoStatus::phase("probing"),
                cancel: Arc::clone(&cancel),
            },
        );
    }

    let jobs_arc = Arc::clone(jobs);
    let marker = kind.marker(&data_dir, &source, gamedata_idx);
    let target = FetchTarget {
        kind,
        id,
        gamedata_idx,
        local_archive: crate::commands::setup::game_root(&data_dir).join(&file.path),
        archive_len: file.size,
        archive_path: file.path.clone(),
        data_dir,
        source,
    };

    tauri::async_runtime::spawn(async move {
        let result = fetch_entry(&target, &jobs_arc, &manager, &cancel).await;

        let mut jobs = jobs_arc.write().await;
        let Some(job) = jobs.get_mut(&id) else { return };
        match result {
            Ok(Some(path)) => {
                job.status.phase = "ready".into();
                job.status.progress = 1.0;
                job.status.path = Some(path);
            }
            Ok(None) => {
                job.status.phase = "none".into();
                // A read that completed and found nothing is a real answer;
                // a timeout is not, and lands in the Err arm below.
                if let Some(parent) = marker.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                let _ = tokio::fs::write(&marker, b"").await;
            }
            Err(e) if e == "cancelled" => {
                jobs.remove(&id);
            }
            Err(e) => {
                log::warn!("{} fetch for game {} failed: {}", kind.label(), id, e);
                job.status.phase = "error".into();
                job.status.error = Some(e);
            }
        }
    });

    Ok(VideoStatus::phase("probing"))
}

/// Everything a fetch needs to know about its archive and where the result goes.
struct FetchTarget {
    kind: MediaKind,
    id: i64,
    gamedata_idx: usize,
    local_archive: PathBuf,
    archive_len: u64,
    archive_path: String,
    data_dir: String,
    source: String,
}

impl FetchTarget {
    async fn store(&self, entry_name: &str, bytes: &[u8]) -> Result<String, String> {
        let cached = self.kind.cache_file(&self.data_dir, &self.source, self.gamedata_idx, entry_name);
        write_cache(&cached, bytes).await?;
        Ok(crate::commands::setup::path_to_fwd_slash(&cached))
    }
}

async fn fetch_entry(
    target: &FetchTarget,
    jobs: &JobMap,
    manager: &Arc<crate::torrent::manager::DownloadManager>,
    cancel: &Arc<AtomicBool>,
) -> Result<Option<String>, String> {
    let kind = target.kind;
    let id = target.id;
    // 1) A local archive costs nothing: installed games keep their GameData,
    //    and even a partial download can already cover the entry.
    if target.local_archive.is_file() {
        if let Ok(mut handle) = tokio::fs::File::open(&target.local_archive).await {
            match extract(kind, &mut handle, target.archive_len, jobs, id, cancel).await {
                Ok(Some((bytes, name))) => {
                    let path = target.store(&name, &bytes).await?;
                    log::info!("{} for game {} read from the local archive", kind.label(), id);
                    return Ok(Some(path));
                }
                Ok(None) => return Ok(None),
                Err(e) if e == "cancelled" => return Err(e),
                Err(e) => log::info!("Local archive unusable for game {} ({}), streaming", id, e),
            }
        }
    }

    // 2) Stream: seeks become piece requests, so the transfer is bounded by the
    //    entry's size rather than the archive's.
    log::info!(
        "Streaming {} for game {} from {} ({:.1} MB archive)",
        kind.label(),
        id,
        target.archive_path,
        target.archive_len as f64 / 1_048_576.0
    );
    let mut stream = manager
        .stream_file(target.gamedata_idx)
        .await
        .map_err(|e| format!("Could not open the archive stream: {}", e))?;
    let Some((bytes, name)) = extract(kind, &mut stream, target.archive_len, jobs, id, cancel).await? else {
        log::info!("Archive for game {} contains no {}", id, kind.label());
        return Ok(None);
    };
    let path = target.store(&name, &bytes).await?;
    log::info!(
        "{} for game {} extracted: {:.1} MB",
        kind.label(),
        id,
        bytes.len() as f64 / 1_048_576.0
    );
    Ok(Some(path))
}

/// A stream waits for pieces indefinitely, and pieces nobody seeds never
/// arrive - one such game would otherwise hold a slot for the whole session
/// (observed: AH-3 ThunderStrike sat in "fetching" for 20 minutes). Both reads
/// get a deadline; the directory's is shorter because it is a few kilobytes
/// from the archive's tail, so slowness there means the pieces are unavailable
/// rather than large.
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(45);
const ENTRY_TIMEOUT: Duration = Duration::from_secs(300);

/// Read the archive directory, then the wanted entry, publishing progress as
/// it goes so the panel can show something other than a spinner. Returns the
/// bytes and the entry's name (the extension names the cache file).
async fn extract<R>(
    kind: MediaKind,
    reader: &mut R,
    archive_len: u64,
    jobs: &JobMap,
    id: i64,
    cancel: &Arc<AtomicBool>,
) -> Result<Option<(Vec<u8>, String)>, String>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin,
{
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let entries = tokio::time::timeout(
        DIRECTORY_TIMEOUT,
        zip_range::read_central_directory(reader, archive_len),
    )
    .await
    .map_err(|_| "timed out reading the archive index".to_string())?
    .map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let Some(entry) = kind.find(&entries) else {
        log::info!(
            "No {} in the archive for game {} ({} entries, folders: {})",
            kind.label(),
            id,
            entries.len(),
            zip_range::top_level_folders(&entries).join(", ")
        );
        return Ok(None);
    };

    let total = entry.compressed_size;
    {
        // The entry exists - from here the UI has something true to show.
        let mut guard = jobs.write().await;
        if let Some(job) = guard.get_mut(&id) {
            job.status.phase = "fetching".into();
            job.status.total_bytes = entry.uncompressed_size;
        }
    }

    // Progress is published from a blocking callback, so it uses the sync
    // try_write path - a missed tick is fine, the next chunk catches up.
    let cancel_flag = Arc::clone(cancel);
    let jobs_for_cb = Arc::clone(jobs);
    let bytes = tokio::time::timeout(ENTRY_TIMEOUT, zip_range::read_entry_with(reader, entry, move |read, _| {
        if cancel_flag.load(Ordering::Relaxed) {
            return false;
        }
        if let Ok(mut guard) = jobs_for_cb.try_write() {
            if let Some(job) = guard.get_mut(&id) {
                job.status.progress = if total > 0 { read as f64 / total as f64 } else { 0.0 };
            }
        }
        true
    }))
    .await
    .map_err(|_| format!("timed out fetching the {}", kind.label()))?
    .map_err(|e| {
        if e.to_string().contains("cancelled") { "cancelled".to_string() } else { e.to_string() }
    })?;
    Ok(Some((bytes, entry.name.clone())))
}

/// Whether mounting a `<video>` element is SAFE on this system.
///
/// On Linux the webview plays media through GStreamer, and a missing
/// `autoaudiosink` does not degrade gracefully: WebKit's pipeline setup hits a
/// NULL instance ("GStreamer element autoaudiosink not found", then
/// g_signal_connect_data assertion failures) and the WebKitWebProcess wedges -
/// the whole app freezes the moment a preview starts. The frontend asks this
/// once and simply never mounts a video when the answer is no; a fetched
/// preview nobody can watch is wasted torrent traffic anyway.
///
/// The .deb/.rpm declare the GStreamer packages as dependencies, so this
/// mainly guards the AppImage - which needs the OPPOSITE probe: linuxdeploy
/// bundles the GStreamer core (a WebKit dependency), and plugins only load
/// into the core they were built against, so the host's plugins are invisible
/// to the app's WebKit no matter what gst-inspect says. Only plugins bundled
/// next to that core (bundleMediaFramework) count there.
#[tauri::command]
pub async fn video_playback_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SUPPORTED.get_or_init(|| {
            // Two independent requirements, and each fails differently:
            // no autoaudiosink wedges the WebKit process outright, no H.264
            // decoder plays an eternally black rectangle. Both mean the
            // preview feature should stand down and say why.
            let (audio, h264) = if let Some(lib) = appimage_bundled_gst_lib() {
                let plugins = lib.join("gstreamer-1.0");
                (
                    plugins.join("libgstautodetect.so").exists(),
                    ["libgstlibav.so", "libgstopenh264.so"]
                        .iter()
                        .any(|f| plugins.join(f).exists()),
                )
            } else {
                (
                    gst_has_any(&["autoaudiosink"], &["libgstautodetect.so"]),
                    gst_has_any(
                        // Any one of these decodes our MP4s: ffmpeg's, Cisco's,
                        // VA-API or NVIDIA's. gst-libav is the note's install
                        // advice because it works without particular hardware.
                        &["avdec_h264", "openh264dec", "vah264dec", "nvh264dec"],
                        &["libgstlibav.so", "libgstopenh264.so"],
                    ),
                )
            };
            let ok = audio && h264;
            if !ok {
                log::warn!(
                    "Preview videos disabled: GStreamer audio sink present: {}, H.264 decoder present: {}",
                    audio,
                    h264
                );
            }
            ok
        })
    }
    #[cfg(not(target_os = "linux"))]
    true
}

/// The lib dir of an AppImage that carries its own GStreamer core, when
/// running inside one. APPDIR is exported by the AppRun hooks (also for an
/// extracted tree); the core check guards the day bundling stops, at which
/// point the host probe below becomes the right question again.
#[cfg(target_os = "linux")]
fn appimage_bundled_gst_lib() -> Option<std::path::PathBuf> {
    let lib = std::path::PathBuf::from(std::env::var_os("APPDIR")?).join("usr/lib");
    lib.join("libgstreamer-1.0.so.0").exists().then_some(lib)
}

/// Whether GStreamer offers any of the named elements, or - when gst-inspect
/// is not installed - whether any of the named plugin files exists in the
/// usual multiarch homes. Erring towards "no" is the safe direction: a
/// skipped preview beats a frozen app or a black box.
#[cfg(target_os = "linux")]
fn gst_has_any(elements: &[&str], plugin_files: &[&str]) -> bool {
    let mut inspect_ran = false;
    for element in elements {
        if let Ok(out) = std::process::Command::new("gst-inspect-1.0").arg(element).output() {
            inspect_ran = true;
            if out.status.success() {
                return true;
            }
        }
    }
    if inspect_ran {
        return false;
    }
    const PLUGIN_DIRS: [&str; 4] = [
        "/usr/lib/x86_64-linux-gnu/gstreamer-1.0",
        "/usr/lib/aarch64-linux-gnu/gstreamer-1.0",
        "/usr/lib64/gstreamer-1.0",
        "/usr/lib/gstreamer-1.0",
    ];
    PLUGIN_DIRS
        .iter()
        .any(|dir| plugin_files.iter().any(|f| Path::new(dir).join(f).exists()))
}

async fn status_of(jobs: &JobMap, id: i64) -> Option<VideoStatus> {
    jobs.read().await.get(&id).map(|j| j.status.clone())
}

async fn cancel_in(jobs: &JobMap, id: i64) {
    let jobs = jobs.read().await;
    if let Some(job) = jobs.get(&id) {
        job.cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub async fn get_video_status(
    video_state: State<'_, VideoState>,
    id: i64,
) -> Result<Option<VideoStatus>, String> {
    Ok(status_of(&video_state.0, id).await)
}

/// Stop an in-flight fetch - the panel calls this when the user moves on, so
/// browsing through games doesn't leave a queue of torrent reads behind.
#[tauri::command]
pub async fn cancel_game_video(video_state: State<'_, VideoState>, id: i64) -> Result<(), String> {
    cancel_in(&video_state.0, id).await;
    Ok(())
}

#[tauri::command]
pub async fn get_music_status(
    music_state: State<'_, MusicState>,
    id: i64,
) -> Result<Option<VideoStatus>, String> {
    Ok(status_of(&music_state.0, id).await)
}

#[tauri::command]
pub async fn cancel_game_music(music_state: State<'_, MusicState>, id: i64) -> Result<(), String> {
    cancel_in(&music_state.0, id).await;
    Ok(())
}

// ── Music: shuffle candidates and playback support ───────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct MusicCandidate {
    pub id: i64,
    pub title: String,
    pub torrent_source: Option<String>,
    pub thumbnail_key: Option<String>,
    pub music_file: String,
}

/// Random games whose archive is expected to hold a playable theme, for the
/// shuffle queue. The hint column is the catalogue's word; an archive that
/// answered "nothing here" before is skipped via its marker so a stale hint
/// costs one piece, ever. `gamedata_torrent_index IS NOT NULL` is what makes
/// this "the whole eXoDOS family": localized rows carry no archive of their
/// own and would only duplicate their English game.
#[tauri::command]
pub async fn music_shuffle_candidates(
    db_state: State<'_, DbState>,
    count: u32,
) -> Result<Vec<MusicCandidate>, String> {
    let count = count.clamp(1, 100) as usize;
    let (rows, data_dir) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        let sql = format!(
            "SELECT id, title, torrent_source, thumbnail_key, music_file, gamedata_torrent_index \
             FROM games \
             WHERE {} \
             ORDER BY RANDOM() LIMIT ?1",
            queries::playable_music_sql("games")
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([count * 2], |r| {
                Ok((
                    MusicCandidate {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        torrent_source: r.get(2)?,
                        thumbnail_key: r.get(3)?,
                        music_file: r.get(4)?,
                    },
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        (rows, data_dir)
    };
    let picked = rows
        .into_iter()
        .filter(|(c, idx)| {
            let source = c.torrent_source.as_deref().unwrap_or("eXoDOS");
            !MediaKind::Music.marker(&data_dir, source, *idx as usize).is_file()
        })
        .map(|(c, _)| c)
        .take(count)
        .collect();
    Ok(picked)
}

#[derive(Debug, Default, Serialize)]
pub struct MusicCacheIndex {
    /// Games whose track is already in the cache - playable with no fetch.
    pub cached: Vec<i64>,
    /// Games whose archive was read and held no playable track.
    pub none: Vec<i64>,
}

/// A cache file's `(collection, gamedata file index)` - what its name encodes.
type CacheKey = (String, i64);

/// Split a music cache directory into the keys it answers for: the tracks it
/// holds, and the archives it recorded as empty.
///
/// The stem is `<collection>_<index>` and collection ids carry underscores of
/// their own (`eXoDOS_GLP`), so only the LAST one separates the two.
fn scan_music_cache(dir: &Path) -> (Vec<CacheKey>, Vec<CacheKey>) {
    let (mut cached, mut none) = (Vec::new(), Vec::new());
    let Ok(entries) = std::fs::read_dir(dir) else {
        // No cache yet is the normal first-run state, not an error.
        return (cached, none);
    };
    for path in entries.flatten().map(|e| e.path()) {
        let Some((collection, index)) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| stem.rsplit_once('_'))
            .and_then(|(c, i)| i.parse::<i64>().ok().map(|i| (c.to_string(), i)))
        else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) == Some("nomusic") {
            none.push((collection, index));
        } else if MediaKind::Music.is_payload(&path) {
            cached.push((collection, index));
        }
        // Anything else (a half-written `.part`) answers nothing.
    }
    (cached, none)
}

/// Which games the music cache can already answer for, so the shuffle queue and
/// the panel can skip a fetch. Keyed by `(collection, gamedata index)` because
/// that is what the cache files are named after - game ids are not in them.
#[tauri::command]
pub async fn music_cache_index(db_state: State<'_, DbState>) -> Result<MusicCacheIndex, String> {
    // The directory walk happens with the mutex RELEASED - it is the slow part
    // (one stat per cached track) and every other DB command would queue behind
    // it. So: lock once for the two queries, drop, then scan.
    let (data_dir, ids) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let data_dir = queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();

        let mut ids: HashMap<CacheKey, i64> = HashMap::new();
        let mut stmt = conn
            .prepare(
                "SELECT id, torrent_source, gamedata_torrent_index FROM games \
                 WHERE gamedata_torrent_index IS NOT NULL AND music_file IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let source: Option<String> = r.get(1)?;
                Ok((
                    (source.unwrap_or_else(|| "eXoDOS".to_string()), r.get::<_, i64>(2)?),
                    r.get::<_, i64>(0)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (key, id) = row.map_err(|e| e.to_string())?;
            ids.insert(key, id);
        }
        drop(stmt);
        (data_dir, ids)
    };

    let (cached, none) = scan_music_cache(&MediaKind::Music.cache_dir(&data_dir));
    if cached.is_empty() && none.is_empty() {
        return Ok(MusicCacheIndex::default());
    }

    let resolve = |keys: Vec<CacheKey>| -> Vec<i64> {
        keys.into_iter().filter_map(|k| ids.get(&k).copied()).collect()
    };
    Ok(MusicCacheIndex { cached: resolve(cached), none: resolve(none) })
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct MusicSupport {
    pub mp3: bool,
    pub ogg: bool,
}

/// Which theme formats the webview can play. Same reasoning as
/// `video_playback_supported`: on Linux the answer is GStreamer's, and inside
/// an AppImage only the bundled plugins count. Per format rather than one
/// flag, so a missing vorbis decoder only skips the .ogg candidates instead
/// of standing the feature down.
#[tauri::command]
pub async fn music_playback_supported() -> MusicSupport {
    #[cfg(target_os = "linux")]
    {
        static SUPPORTED: std::sync::OnceLock<MusicSupport> = std::sync::OnceLock::new();
        *SUPPORTED.get_or_init(|| {
            let support = if let Some(lib) = appimage_bundled_gst_lib() {
                let plugins = lib.join("gstreamer-1.0");
                let has = |f: &str| plugins.join(f).exists();
                let audio = has("libgstautodetect.so");
                // A decoder without the parser in front of it is a silent
                // pipeline: raw MP3 needs mpegaudioparse (plugins-good).
                let mp3 = (has("libgstmpg123.so") || has("libgstlibav.so")) && has("libgstaudioparsers.so");
                let ogg = has("libgstogg.so") && has("libgstvorbis.so");
                MusicSupport { mp3: audio && mp3, ogg: audio && ogg }
            } else {
                let audio = gst_has_any(&["autoaudiosink"], &["libgstautodetect.so"]);
                let mp3 = gst_has_any(&["mpg123audiodec", "avdec_mp3"], &["libgstmpg123.so", "libgstlibav.so"])
                    && gst_has_any(&["mpegaudioparse"], &["libgstaudioparsers.so"]);
                let ogg = gst_has_any(&["oggdemux"], &["libgstogg.so"])
                    && gst_has_any(&["vorbisdec"], &["libgstvorbis.so"]);
                MusicSupport { mp3: audio && mp3, ogg: audio && ogg }
            };
            if !support.mp3 || !support.ogg {
                log::warn!(
                    "Theme playback limited: mp3 supported: {}, ogg supported: {}",
                    support.mp3,
                    support.ogg
                );
            }
            support
        })
    }
    #[cfg(not(target_os = "linux"))]
    MusicSupport { mp3: true, ogg: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::SeekFrom;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

    /// A torrent stream whose pieces never arrive: reads park forever. This is
    /// what an unseeded region looks like from the reader's side, and what made
    /// one game hold a fetch slot for 20 minutes.
    struct StalledReader;

    impl AsyncRead for StalledReader {
        fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }

    impl AsyncSeek for StalledReader {
        fn start_seek(self: Pin<&mut Self>, _: SeekFrom) -> std::io::Result<()> {
            Ok(())
        }
        fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
            Poll::Ready(Ok(0))
        }
    }

    /// The offline answer shares its phase with "the archive has none", which
    /// the frontend caches forever - so it has to stay tellable apart by its
    /// token, or an offline visit poisons the game for the rest of the install.
    #[test]
    fn offline_is_a_none_that_carries_a_token() {
        let offline = VideoStatus::offline();
        let empty = VideoStatus::phase("none");
        assert_eq!(offline.phase, empty.phase);
        assert_eq!(offline.error.as_deref(), Some("offline"));
        assert_eq!(empty.error, None);
    }

    /// Markers are the reason a game with no video is asked about once rather
    /// than on every visit - pruning must never take them.
    #[test]
    fn pruning_keeps_the_no_video_markers() {
        let dir = std::env::temp_dir().join(format!("exodium_vidprune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("content").join("videocache");
        std::fs::create_dir_all(&cache).unwrap();

        std::fs::write(cache.join("eXoDOS_1.novideo"), b"").unwrap();
        for i in 0..3 {
            std::fs::write(cache.join(format!("eXoDOS_{}.mp4", 10 + i)), vec![0u8; 4096]).unwrap();
        }

        prune_video_cache(dir.to_str().unwrap());

        // Well under the cap: nothing should go.
        assert!(cache.join("eXoDOS_1.novideo").exists());
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_keeps_the_no_music_markers() {
        let dir = std::env::temp_dir().join(format!("exodium_musprune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("content").join("musiccache");
        std::fs::create_dir_all(&cache).unwrap();

        std::fs::write(cache.join("eXoDOS_1.nomusic"), b"").unwrap();
        std::fs::write(cache.join("eXoDOS_2.mp3"), vec![0u8; 4096]).unwrap();
        std::fs::write(cache.join("eXoDOS_3.ogg"), vec![0u8; 4096]).unwrap();
        std::fs::write(cache.join("eXoDOS_4.part"), vec![0u8; 4096]).unwrap();

        // A cap of zero forces a prune; only the tracks may go.
        prune_media_cache(MediaKind::Music, dir.to_str().unwrap(), 0);

        assert!(cache.join("eXoDOS_1.nomusic").exists());
        assert!(cache.join("eXoDOS_4.part").exists());
        assert!(!cache.join("eXoDOS_2.mp3").exists());
        assert!(!cache.join("eXoDOS_3.ogg").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The track keeps its own extension, so the cache lookup is by stem -
    /// and must not mistake a marker or a half-written file for a track.
    #[test]
    fn cached_music_matches_any_playable_extension() {
        let dir = std::env::temp_dir().join(format!("exodium_muscache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("content").join("musiccache");
        std::fs::create_dir_all(&cache).unwrap();
        let data_dir = dir.to_str().unwrap();

        assert!(MediaKind::Music.cached(data_dir, "eXoDOS", 7).is_none());
        std::fs::write(cache.join("eXoDOS_7.nomusic"), b"").unwrap();
        std::fs::write(cache.join("eXoDOS_7.part"), b"x").unwrap();
        assert!(MediaKind::Music.cached(data_dir, "eXoDOS", 7).is_none());

        std::fs::write(cache.join("eXoDOS_7.ogg"), b"OggS").unwrap();
        let found = MediaKind::Music.cached(data_dir, "eXoDOS", 7).expect("the track");
        assert_eq!(found.file_name().unwrap().to_str().unwrap(), "eXoDOS_7.ogg");
        // A different index is a different game.
        assert!(MediaKind::Music.cached(data_dir, "eXoDOS", 77).is_none());

        assert_eq!(
            MediaKind::Music.cache_file(data_dir, "eXoDOS", 8, "Music/MS-DOS/X (1990).MP3").file_name().unwrap(),
            "eXoDOS_8.mp3"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The stem splits at the LAST underscore, or `eXoDOS_GLP_3` would read as
    /// collection "eXoDOS" and a bad index.
    #[test]
    fn music_cache_stems_split_at_the_last_underscore() {
        let dir = std::env::temp_dir().join(format!("exodium_muscidx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["eXoDOS_8.mp3", "eXoDOS_9.nomusic", "eXoDOS_GLP_3.ogg", "eXoDOS_4.part"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }

        let (mut cached, none) = scan_music_cache(&dir);
        cached.sort();
        assert_eq!(cached, vec![("eXoDOS".to_string(), 8), ("eXoDOS_GLP".to_string(), 3)]);
        assert_eq!(none, vec![("eXoDOS".to_string(), 9)]);

        // A directory that was never created is the first-run state.
        assert_eq!(scan_music_cache(&dir.join("nope")), (Vec::new(), Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn memory_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn
    }

    fn insert(conn: &rusqlite::Connection, title: &str, lang: &str, code: &str, source: &str, gd: Option<i64>) -> i64 {
        conn.execute(
            "INSERT INTO games (title, language, shortcode, torrent_source, gamedata_torrent_index) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![title, lang, code, source, gd],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// A localized row has no archive of its own and borrows its English
    /// game's - but only inside its own pack family, never a same-coded game
    /// from another pack.
    #[test]
    fn resolve_gamedata_borrows_the_english_archive_within_the_family() {
        let conn = memory_db();
        insert(&conn, "Earthquest", "EN", "EarthQue", "eXoDOS", Some(41));
        let de = insert(&conn, "Earthquest DE", "DE", "EarthQue", "eXoDOS_GLP", None);
        insert(&conn, "Earth Quest VR", "EN", "EarthQue", "eXoWin3x", Some(9));
        let win3x_only = insert(&conn, "Lonely", "EN", "Lonely", "eXoWin3x", None);

        let game = queries::fetch_game_by_id(&conn, de).unwrap().unwrap();
        assert_eq!(resolve_gamedata(&conn, &game), (Some(41), "eXoDOS".to_string()));

        let game = queries::fetch_game_by_id(&conn, win3x_only).unwrap().unwrap();
        assert_eq!(resolve_gamedata(&conn, &game), (None, "eXoDOS".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_stream_gives_up_instead_of_holding_the_slot() {
        let jobs: Arc<RwLock<HashMap<i64, VideoJob>>> = Arc::new(RwLock::new(HashMap::new()));
        jobs.write().await.insert(
            1,
            VideoJob {
                status: VideoStatus::phase("fetching"),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        let cancel = Arc::new(AtomicBool::new(false));

        let err = extract(MediaKind::Video, &mut StalledReader, 10_000_000, &jobs, 1, &cancel)
            .await
            .expect_err("a stream that never delivers must fail");

        assert!(err.contains("timed out"), "unexpected error: {}", err);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_is_reported_as_such_not_as_a_failure() {
        let jobs: Arc<RwLock<HashMap<i64, VideoJob>>> = Arc::new(RwLock::new(HashMap::new()));
        let cancel = Arc::new(AtomicBool::new(true));
        let err = extract(MediaKind::Video, &mut StalledReader, 10_000_000, &jobs, 2, &cancel)
            .await
            .expect_err("cancelled");
        // Cancellation is a user action, so it must never surface as an error
        // phase - the caller distinguishes on this exact string. It must also
        // cost no I/O: a job cancelled while queued should not read anything.
        assert_eq!(err, "cancelled");
    }
}

// ── Localhost media server (Linux) ───────────────────────────────────────────
//
// WebKitGTK's media player cannot pull media out of a custom URI scheme
// handler: a <video> whose src is served through one ends with
// MEDIA_ERR_SRC_NOT_SUPPORTED / networkState NO_SOURCE (measured on WebKitGTK
// 2.52 with a minimal harness - the same file plays fine from file://).
// Images are unaffected, so the asset protocol stays for those; only <video>
// sources go through this 127.0.0.1 HTTP server, whose responses tower-http's
// ServeFile answers with proper Range support (GStreamer seeks).
//
// URLs carry an opaque per-session token instead of a path: the HTTP side
// never parses paths, an unknown token is a 404, and only files the backend
// itself registered (after the same under-the-data-dir check the asset scope
// enforces) are reachable. Bound to 127.0.0.1; other local processes can
// fetch registered previews, which is the same exposure any local media
// server has.

/// Token -> file map plus the lazily-started server's port. The map sits
/// behind its own Arc because the axum router holds a clone of it.
pub struct MediaServerState {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    port: std::sync::Mutex<Option<u16>>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    tokens: std::sync::Arc<std::sync::Mutex<HashMap<String, PathBuf>>>,
}

impl MediaServerState {
    pub fn new() -> Self {
        Self {
            port: std::sync::Mutex::new(None),
            tokens: std::sync::Arc::default(),
        }
    }
}

impl Default for MediaServerState {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate an absolute media path into a playable URL, or None where the
/// asset protocol already plays media fine (macOS/Windows) - the frontend
/// falls back to convertFileSrc then.
#[tauri::command]
pub async fn media_url(
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] db_state: State<
        '_,
        crate::DbState,
    >,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] server: State<
        '_,
        MediaServerState,
    >,
    #[cfg_attr(not(target_os = "linux"), allow(unused_variables))] path: String,
) -> Result<Option<String>, String> {
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
    #[cfg(target_os = "linux")]
    {
        let data_dir = {
            let conn = db_state.0.lock().map_err(|e| e.to_string())?;
            queries::get_config(&conn, "data_dir")
                .map_err(|e| e.to_string())?
                .ok_or("Data directory not configured")?
        };
        // Same containment rule as the asset-protocol scope: only files under
        // the user's data dir are servable. Canonicalize both sides so a
        // symlinked data dir still matches and `..` segments can't escape.
        let canon_file = std::fs::canonicalize(&path).map_err(|e| format!("{path}: {e}"))?;
        let canon_root =
            std::fs::canonicalize(&data_dir).map_err(|e| format!("{data_dir}: {e}"))?;
        if !canon_file.starts_with(&canon_root) {
            return Err(format!("{} is outside the data directory", canon_file.display()));
        }
        if !canon_file.is_file() {
            return Err(format!("{} is not a file", canon_file.display()));
        }

        let port = {
            // Hold the port lock across server startup so two concurrent
            // calls can't both bind a listener.
            let mut port = server.port.lock().map_err(|e| e.to_string())?;
            match *port {
                Some(p) => p,
                None => {
                    let p = start_media_server(server.tokens.clone())?;
                    *port = Some(p);
                    p
                }
            }
        };
        let token = media_token(&canon_file);
        server
            .tokens
            .lock()
            .map_err(|e| e.to_string())?
            .insert(token.clone(), canon_file);
        Ok(Some(format!("http://127.0.0.1:{port}/m/{token}")))
    }
}

/// Opaque, non-guessable token: file path hashed with a per-process salt.
#[cfg(target_os = "linux")]
fn media_token(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    use std::sync::OnceLock;
    static SALT: OnceLock<u128> = OnceLock::new();
    let salt = *SALT.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ (std::process::id() as u128) << 64
    });
    let mut h = std::collections::hash_map::DefaultHasher::new();
    salt.hash(&mut h);
    path.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(target_os = "linux")]
fn start_media_server(
    tokens: std::sync::Arc<std::sync::Mutex<HashMap<String, PathBuf>>>,
) -> Result<u16, String> {
    use axum::body::Body;
    use axum::extract::{Path as AxPath, State as AxState};
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use tower::ServiceExt;

    async fn serve(
        AxState(tokens): AxState<std::sync::Arc<std::sync::Mutex<HashMap<String, PathBuf>>>>,
        AxPath(token): AxPath<String>,
        req: Request<Body>,
    ) -> Result<Response, StatusCode> {
        let file = tokens
            .lock()
            .ok()
            .and_then(|m| m.get(&token).cloned())
            .ok_or(StatusCode::NOT_FOUND)?;
        tower_http::services::ServeFile::new(file)
            .oneshot(req)
            .await
            .map(|res| res.map(Body::new))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }

    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;

    let router = axum::Router::new()
        .route("/m/{token}", axum::routing::get(serve))
        .with_state(tokens);

    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                log::error!("media server: listener conversion failed: {e}");
                return;
            }
        };
        if let Err(e) = axum::serve(listener, router).await {
            log::error!("media server exited: {e}");
        }
    });

    log::info!("media server listening on 127.0.0.1:{port}");
    Ok(port)
}
