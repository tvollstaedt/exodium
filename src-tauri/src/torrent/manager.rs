use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions,
    SessionPersistenceConfig,
};
use serde::Serialize;
use tokio::sync::RwLock;

use walkdir::WalkDir;

/// Upload cap applied when sharing is off. Not zero: librqbit throttles rather
/// than blocks, and a hard zero would stall the handshakes that keep downloads
/// working.
pub const SEEDING_OFF_BPS: u32 = 1024;

/// The ONE place that sets the session's transfer caps: seeding and the
/// user's upload limit write the same knob, and seeding-off (1 KB/s) wins.
/// Called for a running session (`apply_limits`) and a fresh one (§11).
pub fn apply_session_limits(
    session: &librqbit::Session,
    seeding: bool,
    up_kbps: Option<u32>,
    down_kbps: Option<u32>,
) {
    let to_bps = |kbps: u32| std::num::NonZeroU32::new(kbps.saturating_mul(1024));
    // Seeding off wins over any user limit: librqbit has no upload
    // kill-switch, so 1 KB/s is as close to off as it gets while leaving
    // enough headroom for the handshakes downloads depend on.
    let up = if seeding {
        up_kbps.and_then(to_bps)
    } else {
        std::num::NonZeroU32::new(SEEDING_OFF_BPS)
    };
    let down = down_kbps.and_then(to_bps);
    session.ratelimits.set_upload_bps(up);
    session.ratelimits.set_download_bps(down);
    log::info!(
        "Transfer limits: seeding={} up={} down={}",
        seeding,
        up.map_or("unlimited".to_string(), |b| format!("{} B/s", b.get())),
        down.map_or("unlimited".to_string(), |b| format!("{} B/s", b.get())),
    );
}

use anyhow::Context;
use super::TorrentIndex;

/// Clear the ledger bits of every piece overlapping the byte range
/// [offset, offset+size) - MSB0 order, matching librqbit's .bitv layout
/// (bit i of the file, high-bit-first per byte, is piece i).
fn clear_file_pieces(bytes: &mut [u8], offset: u64, size: u64, piece_len: u64) {
    if size == 0 || piece_len == 0 {
        return;
    }
    let start = offset / piece_len;
    let end = (offset + size - 1) / piece_len;
    for p in start..=end {
        if let Some(b) = bytes.get_mut((p / 8) as usize) {
            *b &= !(0x80u8 >> (p % 8));
        }
    }
}

/// NT extended-length form (`\\?\C:\...`, `\\?\UNC\...`): lifts the
/// MAX_PATH limit for every file librqbit opens under the folder.
#[cfg(target_os = "windows")]
fn to_long_path(p: &Path) -> String {
    // \\?\ disables path normalization, so we must hand it backslash-only paths.
    // Tauri's dialog and PathBuf::join sometimes leave forward slashes from
    // user-provided strings; normalize before prefixing.
    let s = p.to_string_lossy().replace('/', r"\");
    if s.starts_with(r"\\?\") {
        return s;
    }
    if !p.is_absolute() {
        return s;
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        // UNC path: \\server\share\... -> \\?\UNC\server\share\...
        return format!(r"\\?\UNC\{}", rest);
    }
    format!(r"\\?\{}", s)
}

#[cfg(not(target_os = "windows"))]
fn to_long_path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Remove 0-byte zips under `root` that no enabled torrent declares.
/// `keep_paths` must be the FULL file list of every collection sharing the
/// root, not the selection: librqbit creates a placeholder per declared file
/// and its fastresume ledger marks shared pieces "have", so deleting a
/// tracked placeholder leaves a file at "100%" that never appears on disk.
fn cleanup_placeholder_files(root: &Path, keep_paths: &[String]) -> std::io::Result<()> {
    let mut removed = 0;
    let mut kept = 0;
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let meta = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() != 0 {
            continue;
        }
        if path.extension().map(|e| e != "zip").unwrap_or(true) {
            continue;
        }
        // Forward-slash form of the absolute path on disk. `keep_paths`
        // entries are torrent-relative ("eXoDOS/Content/.../Foo.zip"), so a
        // suffix match is enough - and it is slash-direction-agnostic now.
        let path_fwd = path.to_string_lossy().replace('\\', "/");
        let in_torrent = keep_paths.iter().any(|sp| path_fwd.ends_with(sp));
        if in_torrent {
            // No per-file logging: this fires ~14k times per torrent add
            // (observed 14,616 lines in one field session) - the summary
            // line below carries the counts.
            kept += 1;
            continue;
        }
        log::info!("Cleanup: deleting orphan 0-byte placeholder {}", path.display());
        let _ = std::fs::remove_file(path);
        removed += 1;
    }
    // Remove empty directories left behind
    for entry in WalkDir::new(root).contents_first(true).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() && path != root {
            let _ = std::fs::remove_dir(path);
        }
    }
    log::info!(
        "Cleanup: deleted {} orphan placeholder(s), kept {} torrent-tracked (torrent size {})",
        removed, kept, keep_paths.len()
    );
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub file_index: usize,
    pub file_name: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub progress: f64,
    pub finished: bool,
    /// Set by the command layer after checking DB - true once extracted and marked installed.
    #[serde(default)]
    pub installed: bool,
    /// Optional error/status message from the command layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The archive is here but the install waits on another game: the
    /// English base an overlay language variant is unpacked onto (§10a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    /// librqbit's torrent state. `initializing` (hash check, minutes on
    /// Windows) keeps `progress` at 0; the UI shows "Validating…" instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_state: Option<String>,
    /// Whole-torrent validation/download progress (0.0..1.0). During init this
    /// reflects librqbit's hash-check progress; once live, it tracks downloaded
    /// bytes across all selected files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_progress: Option<f64>,
    /// Progress of the game's extras (GameData), which keep downloading after
    /// the game is playable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras_progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras_done: Option<bool>,
}

/// Live transfer figures for the shared session.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SessionTransfer {
    pub download_bps: u64,
    pub upload_bps: u64,
    /// Uploaded since the session started - librqbit keeps no lifetime total.
    pub uploaded_bytes: u64,
    /// Peers currently connected. The readout that answers "is anything
    /// happening" while the rates sit at zero: connections are a standing
    /// state, transfer is event-driven.
    pub peers: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadManagerStatus {
    pub active_downloads: Vec<DownloadProgress>,
    /// Whether this manager's torrent is live (not merely added, checking or
    /// paused). "Nothing running" and "running at 0 B/s" are different things
    /// and the UI shows them differently, so don't collapse them.
    pub live: bool,
}

/// One collection's torrent in the shared librqbit session, added on the
/// first download (adding creates 14,000+ placeholder files).
///
/// Lock order: `selection_apply`, then `handle`, then `selected_files`.
/// tokio's RwLock is write-preferring, so a mixed order deadlocks.
pub struct DownloadManager {
    session: Arc<Session>,
    handle: RwLock<Option<Arc<ManagedTorrent>>>,
    torrent_index: TorrentIndex,
    torrent_bytes: Arc<Vec<u8>>,
    selected_files: RwLock<HashSet<usize>>,
    /// Serializes every selection push into librqbit: two concurrent
    /// `update_only_files`, or one racing a re-check, wedge its checking
    /// task until even `stats()` blocks.
    selection_apply: tokio::sync::Mutex<()>,
    data_dir: PathBuf,
    /// Hex SHA1 info-hash of this manager's torrent, for finding it among
    /// the session's persisted (auto-resumed) torrents.
    info_hash_hex: Option<String>,
    /// Where librqbit keeps <hash>.bitv piece-ledger files - needed for the
    /// surgical ledger patch on uninstall.
    persistence_dir: PathBuf,
    /// Union of every enabled collection's file list, since all share one
    /// root (`cleanup_placeholder_files`). This torrent's own list when unset.
    cleanup_keep_paths: std::sync::RwLock<Option<Arc<Vec<String>>>>,
}

/// Default location for librqbit's fastresume persistence (`<info_hash>.bitv`,
/// `<info_hash>.torrent`, `session.json`). Co-located with the session dir so
/// it shares the same lifecycle (cleared by factory_reset).
pub(crate) fn fastresume_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("librqbit-fastresume")
}

impl DownloadManager {
    /// The shared librqbit session. `session_dir` is the app config dir (not
    /// the data dir); `persistence_dir` holds the fastresume bitfields and
    /// session.json - a pre-seeded `<info_hash>.bitv` skips the initial check.
    pub async fn create_session(
        session_dir: &Path,
        persistence_dir: &Path,
    ) -> anyhow::Result<Arc<Session>> {
        std::fs::create_dir_all(session_dir)?;
        std::fs::create_dir_all(persistence_dir)?;
        let session = Session::new_with_opts(
            session_dir.to_path_buf(),
            SessionOptions {
                // DHT on, DHT persistence off (unchanged semantics from the
                // pre-rc API's disable_dht=false + disable_dht_persistence).
                dht: Some(librqbit::DhtSessionConfig {
                    persistence: None,
                    ..Default::default()
                }),
                // The persisted bitfield lets later adds skip the initial
                // check (minutes on Windows).
                fastresume: true,
                persistence: Some(SessionPersistenceConfig::Json {
                    folder: Some(persistence_dir.to_path_buf()),
                }),
                ..Default::default()
            },
        )
        .await?;
        Ok(session)
    }

    /// Initialize a download manager using a shared session.
    pub fn new_with_session(
        session: Arc<Session>,
        torrent_path: &Path,
        data_dir: &Path,
        persistence_dir: &Path,
    ) -> anyhow::Result<Self> {
        let torrent_bytes = Arc::new(std::fs::read(torrent_path)?);
        let torrent_index = TorrentIndex::from_file(torrent_path)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let info_hash_hex = TorrentIndex::infohash(torrent_path).ok();

        log::info!(
            "Download manager initialized: {} files in torrent, data dir: {}",
            torrent_index.files.len(),
            data_dir.display()
        );

        Ok(Self {
            session,
            handle: RwLock::new(None),
            torrent_index,
            torrent_bytes,
            selected_files: RwLock::new(HashSet::new()),
            selection_apply: tokio::sync::Mutex::new(()),
            data_dir: data_dir.to_path_buf(),
            info_hash_hex,
            persistence_dir: persistence_dir.to_path_buf(),
            cleanup_keep_paths: std::sync::RwLock::new(None),
        })
    }

    /// Adopt a torrent the session auto-resumed from persistence; otherwise a
    /// download in flight at shutdown continues invisibly and the next add
    /// would deselect it. True when adopted.
    pub async fn hydrate_from_session(&self) -> bool {
        if self.handle.read().await.is_some() {
            return true;
        }
        let Some(ref my_hash) = self.info_hash_hex else {
            return false;
        };
        let found = self.session.with_torrents(|iter| {
            for (_, t) in iter {
                if t.info_hash().as_string().eq_ignore_ascii_case(my_hash) {
                    return Some(Arc::clone(t));
                }
            }
            None
        });
        let Some(handle) = found else {
            return false;
        };
        let resumed = handle.only_files().unwrap_or_default();
        let mut handle_guard = self.handle.write().await;
        {
            let mut selected = self.selected_files.write().await;
            selected.extend(resumed.iter().copied());
        }
        *handle_guard = Some(handle);
        log::info!(
            "Hydrated torrent from persisted session ({} previously selected files)",
            resumed.len()
        );
        true
    }

    /// Set the union keep-list for placeholder cleanup (see field docs).
    pub fn set_cleanup_keep_paths(&self, paths: Arc<Vec<String>>) {
        if let Ok(mut guard) = self.cleanup_keep_paths.write() {
            *guard = Some(paths);
        }
    }


    /// Get the torrent file index.
    pub fn index(&self) -> &TorrentIndex {
        &self.torrent_index
    }

    /// Session-wide rates: one read, not a sum over torrents (`stats()` copies
    /// a 15,000-entry file-progress vector per call).
    pub fn session_transfer(&self) -> SessionTransfer {
        let snap = self.session.stats_snapshot();
        SessionTransfer {
            download_bps: snap.download_speed.as_bytes(),
            upload_bps: snap.upload_speed.as_bytes(),
            uploaded_bytes: snap.counters.uploaded_bytes,
            peers: snap.peers.live,
        }
    }

    /// Caps in KB/s, `None` = unlimited; session-wide. See `apply_session_limits`.
    pub fn apply_limits(&self, seeding: bool, up_kbps: Option<u32>, down_kbps: Option<u32>) {
        apply_session_limits(&self.session, seeding, up_kbps, down_kbps);
    }

    /// Stop the shared session (all collections) and flush persistence, so a
    /// caller can delete data files without a writer re-creating them.
    pub async fn shutdown_session(&self) {
        self.session.stop().await;
    }

    /// Returns true if the given file index has been queued for download.
    pub async fn is_file_selected(&self, file_index: usize) -> bool {
        self.selected_files.read().await.contains(&file_index)
    }

    /// The single game root every collection writes into (§2), never
    /// `<data_dir>/<torrent name>`.
    pub fn torrent_root(&self) -> PathBuf {
        crate::commands::paths::game_root(&self.data_dir.to_string_lossy())
    }

    /// Wait out an initial check, then push the CURRENT selection (re-read
    /// under `selection_apply`, so concurrent waiters converge). A push into
    /// a checking torrent wedges librqbit. Never call with the handle lock
    /// held: a re-check takes minutes and progress polling reads that lock.
    async fn wait_ready_then_apply_selection(
        &self,
        handle: &Arc<ManagedTorrent>,
    ) -> anyhow::Result<()> {
        let _apply_guard = self.selection_apply.lock().await;
        const MAX_WAIT_SECS: u64 = 1800; // full check of a large selection on slow disks
        let start = std::time::Instant::now();
        let mut last_log = 0u64;
        while handle.stats().state.to_string() == "initializing" {
            let waited = start.elapsed().as_secs();
            if waited >= MAX_WAIT_SECS {
                anyhow::bail!("torrent still initializing after {}s", waited);
            }
            if waited / 30 > last_log {
                last_log = waited / 30;
                log::info!("Selection update waiting for initial check ({}s)", waited);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // Retry tail for the init->live race. The `selected_files` read lock
        // is held across the push, so a concurrent deselect serializes after
        // us and re-applies its smaller set.
        const MAX_ATTEMPTS: u32 = 10;
        for attempt in 0..MAX_ATTEMPTS {
            let result = {
                let selected = self.selected_files.read().await;
                self.session.update_only_files(handle, &selected).await
            };
            match result {
                Ok(_) => return Ok(()),
                Err(e) if e.to_string().contains("initializing") && attempt + 1 < MAX_ATTEMPTS => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// A seekable reader over one file WITHOUT selecting it: the read position
    /// drives which pieces librqbit fetches (§14). Adds the torrent with an
    /// empty selection when needed. Returned by capability because librqbit's
    /// `FileStream` type is private.
    pub async fn stream_file(
        &self,
        file_index: usize,
    ) -> anyhow::Result<impl tokio::io::AsyncRead + tokio::io::AsyncSeek + Unpin + Send + use<>> {
        if let Some(handle) = self.handle.read().await.as_ref().map(Arc::clone) {
            return handle.stream(file_index).await;
        }
        self.download_files(Vec::new()).await?;
        let handle = self
            .handle
            .read()
            .await
            .as_ref()
            .map(Arc::clone)
            .context("torrent handle missing after add")?;
        handle.stream(file_index).await
    }

    /// Queue file indices for download. Adds the torrent on first call.
    pub async fn download_files(&self, file_indices: Vec<usize>) -> anyhow::Result<()> {
        {
            let mut selected = self.selected_files.write().await;
            for idx in &file_indices {
                selected.insert(*idx);
            }
        }

        // The already-running path may wait MINUTES for an initial check
        // before it can apply the selection - clone the Arc and release the
        // lock first, or every progress poll would block on it.
        let existing: Option<Arc<ManagedTorrent>> =
            { self.handle.read().await.as_ref().map(Arc::clone) };
        if let Some(handle) = existing {
            self.wait_ready_then_apply_selection(&handle).await?;
            log::info!("Updated file selection, added: {:?}", file_indices);
            return Ok(());
        }

        let mut handle_guard = self.handle.write().await;

        if let Some(ref handle) = *handle_guard {
            // Raced another add between our read-check and this write lock -
            // it owns the torrent now; just apply the selection.
            let handle = Arc::clone(handle);
            drop(handle_guard);
            self.wait_ready_then_apply_selection(&handle).await?;
            log::info!("Updated file selection, added: {:?}", file_indices);
        } else {
            // First download - add torrent to session now
            let selected = self.selected_files.read().await.clone();
            // Explicit output folder (the session default is the app data
            // dir); long-path form on Windows or nested entries fail to open.
            let raw_root = self.torrent_root();
            let output_folder = to_long_path(&raw_root);

            // Diagnostic: surface exactly what we are handing to librqbit. On
            // Windows the `\\?\` prefix should appear here. If it doesn't, the
            // long-path fix isn't being applied for this run.
            log::info!(
                "Torrent add: output_folder={:?} (torrent_root={}, exists={}, is_dir={})",
                output_folder,
                raw_root.display(),
                raw_root.exists(),
                raw_root.is_dir()
            );
            log::info!(
                "Torrent add: selected file_indices={:?} (total={} of {})",
                file_indices, selected.len(), self.torrent_index.files.len()
            );

            let response = self
                .session
                .add_torrent(
                    AddTorrent::from_bytes((*self.torrent_bytes).clone()),
                    Some(AddTorrentOptions {
                        only_files: Some(selected.into_iter().collect()),
                        overwrite: true,
                        output_folder: Some(output_folder),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|e| {
                    log::error!("session.add_torrent failed: {}", e);
                    e
                })?;

            let (handle, already_managed) = match response {
                AddTorrentResponse::Added(_id, h) => {
                    log::info!("Torrent add: response=Added");
                    (h, false)
                }
                AddTorrentResponse::AlreadyManaged(_id, h) => {
                    log::info!("Torrent add: response=AlreadyManaged");
                    (h, true)
                }
                AddTorrentResponse::ListOnly(_) => {
                    log::error!("Torrent add: response=ListOnly (unexpected - file selection ignored)");
                    return Err(anyhow::anyhow!("Torrent added in list-only mode"));
                }
            };

            *handle_guard = Some(Arc::clone(&handle));
            log::info!("Torrent added (already_managed={already_managed}), downloading files: {:?}", file_indices);

            // Already in the session: merge and re-apply the selection after
            // the handle lock is dropped (`pending_selection_update`).
            if already_managed {
                // Merge, don't replace: auto-resumed files from a previous
                // run would otherwise be deselected mid-download.
                let session_selection = handle.only_files().unwrap_or_default();
                let mut selected = self.selected_files.write().await;
                selected.extend(session_selection.iter().copied());
            }

            // Diagnostic stats every 2 s for 60 s after an add: tells a peer
            // problem (peers=0) from a disk one (peers>0, speed=0).
            let stats_handle = Arc::clone(&handle);
            let watched_files: Vec<(usize, String, u64)> = file_indices.iter()
                .filter_map(|&idx| {
                    self.torrent_index.files.get(idx).map(|f| (idx, f.path.clone(), f.size))
                })
                .collect();
            tokio::spawn(async move {
                let start = std::time::Instant::now();
                while start.elapsed() < Duration::from_secs(60) {
                    let s = stats_handle.stats();
                    let per_file: Vec<String> = watched_files.iter().map(|(idx, name, size)| {
                        let dl = s.file_progress.get(*idx).copied().unwrap_or(0);
                        let pct = if *size > 0 { (dl as f64 / *size as f64) * 100.0 } else { 0.0 };
                        format!("[{}]={}/{} ({:.1}%) {}", idx, dl, size, pct, name)
                    }).collect();
                    if let Some(ref err) = s.error {
                        // Also after an uninstall removed the torrent: log
                        // once, stop.
                        log::error!("[stats] state={} error={:?}", s.state, err);
                        break;
                    }
                    log::info!(
                        "[stats] {} | live={} | files: {}",
                        s, s.live.is_some(), per_file.join(" ")
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                log::debug!("[stats] periodic logger finished after 60 s");
            });

            // Orphan cleanup with the union keep-list (see
            // `cleanup_placeholder_files` for why the full list matters).
            let root = self.torrent_root();
            let keep_paths: Arc<Vec<String>> = self
                .cleanup_keep_paths
                .read()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_else(|| {
                    Arc::new(
                        self.torrent_index
                            .files
                            .iter()
                            .map(|f| f.path.clone())
                            .collect(),
                    )
                });
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                if let Err(e) = cleanup_placeholder_files(&root, &keep_paths) {
                    log::warn!("Failed to clean up placeholder files: {}", e);
                }
            });

            // AlreadyManaged: apply the merged selection - AFTER releasing
            // the handle lock, because this may wait out a full initial
            // check and progress polling needs that lock.
            drop(handle_guard);
            if already_managed {
                self.wait_ready_then_apply_selection(&handle).await?;
            }
        }

        Ok(())
    }

    /// Get download progress for a specific file index.
    /// Returns None if the torrent hasn't been added yet.
    pub async fn file_progress(&self, file_index: usize) -> Option<DownloadProgress> {
        let handle_guard = self.handle.read().await;
        let handle = handle_guard.as_ref()?;
        let stats = handle.stats();

        let downloaded = stats.file_progress.get(file_index).copied().unwrap_or(0);
        let total = self.torrent_index.files.get(file_index)?.size;
        let finished = total > 0 && downloaded >= total;
        let progress = if total > 0 {
            (downloaded as f64 / total as f64).min(1.0)
        } else {
            0.0
        };

        let file_name = self.torrent_index.files.get(file_index)?.path.clone();

        // Whole-torrent progress mirrors librqbit's view: during `initializing`
        // it is the validation-pass progress; once live, the cumulative download.
        let torrent_progress = if stats.total_bytes > 0 {
            Some((stats.progress_bytes as f64 / stats.total_bytes as f64).min(1.0))
        } else {
            None
        };
        let torrent_state = Some(stats.state.to_string());

        Some(DownloadProgress {
            waiting_for: None,
            file_index,
            file_name,
            downloaded_bytes: downloaded,
            total_bytes: total,
            progress,
            finished,
            installed: false,
            error: None,
            torrent_state,
            torrent_progress,
            extras_progress: None,
            extras_done: None,
        })
    }

    /// Get status for all active downloads.
    pub async fn status(&self) -> DownloadManagerStatus {
        // Lock order: handle before selected_files (see struct docs).
        let handle_guard = self.handle.read().await;
        let selected = self.selected_files.read().await;

        let mut active_downloads = Vec::new();

        if let Some(ref handle) = *handle_guard {
            let stats = handle.stats();
            for &idx in selected.iter() {
                if let Some(entry) = self.torrent_index.files.get(idx) {
                    let downloaded = stats.file_progress.get(idx).copied().unwrap_or(0);
                    let total = entry.size;
                    let finished = total > 0 && downloaded >= total;
                    let progress = if total > 0 {
                        (downloaded as f64 / total as f64).min(1.0)
                    } else {
                        0.0
                    };
                    active_downloads.push(DownloadProgress {
            waiting_for: None,
                        file_index: idx,
                        file_name: entry.path.clone(),
                        downloaded_bytes: downloaded,
                        total_bytes: total,
                        progress,
                        finished,
                        installed: false,
                        error: None,
                        extras_progress: None,
                        extras_done: None,
                        torrent_state: Some(stats.state.to_string()),
                        torrent_progress: if stats.total_bytes > 0 {
                            Some((stats.progress_bytes as f64 / stats.total_bytes as f64).min(1.0))
                        } else {
                            None
                        },
                    });
                }
            }
        }

        // `live()` is an Arc clone; `stats()` would copy a 15,000-entry
        // file-progress vector just to answer this.
        let live = handle_guard.as_ref().is_some_and(|h| h.live().is_some());

        DownloadManagerStatus {
            active_downloads,
            live,
        }
    }

    /// Drop a file from the selection now and push the change through the
    /// serialized applier in the background, so cancel is instant even during
    /// a long check and cannot race one.
    pub async fn deselect_file(self: &Arc<Self>, file_index: usize) {
        {
            let mut selected = self.selected_files.write().await;
            selected.remove(&file_index);
        }
        let handle = { self.handle.read().await.as_ref().map(Arc::clone) };
        if let Some(handle) = handle {
            let mgr = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                if let Err(e) = mgr.wait_ready_then_apply_selection(&handle).await {
                    log::warn!("deselect apply failed: {}", e);
                }
            });
        }
    }

    /// Check if a specific file has finished downloading.
    pub async fn is_file_complete(&self, file_index: usize) -> bool {
        self.file_progress(file_index)
            .await
            .map(|p| p.finished)
            .unwrap_or(false)
    }

    /// Get the output path for a downloaded file.
    pub fn file_output_path(&self, file_index: usize) -> Option<PathBuf> {
        let entry = self.torrent_index.files.get(file_index)?;
        Some(self.torrent_root().join(&entry.path))
    }

    /// The on-disk piece ledger with every piece of the given files cleared:
    /// (path, bytes), or None on any format surprise (callers then take the
    /// full re-check). The snapshot may lag the in-memory bitfield by one
    /// flush; those pieces re-download.
    fn patched_bitv_without(&self, drop_indices: &[usize]) -> Option<(PathBuf, Vec<u8>)> {
        let hash = self.info_hash_hex.as_ref()?;
        let path = self.persistence_dir.join(format!("{}.bitv", hash));
        let mut bytes = std::fs::read(&path).ok()?;
        let piece_len = self.torrent_index.piece_length;
        if piece_len == 0 {
            return None;
        }
        let total_pieces = self.torrent_index.total_size.div_ceil(piece_len);
        if (bytes.len() as u64) * 8 < total_pieces {
            log::warn!(
                "Piece ledger smaller than expected ({} bytes for {} pieces) - falling back to full re-check",
                bytes.len(), total_pieces
            );
            return None;
        }
        for &idx in drop_indices {
            let f = self.torrent_index.files.get(idx)?;
            clear_file_pieces(&mut bytes, f.offset, f.size, piece_len);
        }
        Some((path, bytes))
    }

    /// Reset librqbit's piece bookkeeping after files were deleted (uninstall):
    /// the ledger still claims their pieces, so a re-download would report
    /// 100% with nothing on disk. Drops the torrent from the session; the
    /// next download re-adds it. `drop_indices` leave the selection,
    /// `deleted_indices` (the subset really gone) lose their ledger bits - a
    /// deselected-but-present GameData zip must not re-download.
    pub async fn invalidate_after_file_delete(
        &self,
        drop_indices: &[usize],
        deleted_indices: &[usize],
    ) -> anyhow::Result<()> {
        // Lock order: handle before selected_files (see struct docs).
        let mut handle_guard = self.handle.write().await;
        let remaining: Vec<usize> = {
            let mut selected = self.selected_files.write().await;
            for idx in drop_indices {
                selected.remove(idx);
            }
            selected.iter().copied().collect()
        };
        let Some(handle) = handle_guard.take() else {
            // Torrent not in the session this run (and hydrate_from_session
            // would have adopted a persisted one) - nothing to invalidate.
            return Ok(());
        };
        // Patched ledger restored after session.delete erases it: the next
        // add skips the full re-check (15-30 min in the field).
        let patched_bitv = self.patched_bitv_without(deleted_indices);
        self.session
            .delete(librqbit::api::TorrentIdOrHash::Hash(handle.info_hash()), false)
            .await?;
        if let Some((path, bytes)) = patched_bitv {
            // The flusher may still hold the old handle (delete-pending on
            // exFAT/SMB): write a temp name and rename with a short retry.
            let tmp = path.with_extension("bitv.tmp");
            let mut restored = false;
            for attempt in 0..10u32 {
                let result = std::fs::write(&tmp, &bytes)
                    .and_then(|_| std::fs::rename(&tmp, &path));
                match result {
                    Ok(()) => {
                        restored = true;
                        break;
                    }
                    Err(e) if attempt < 9 => {
                        log::debug!("piece-ledger restore attempt {} failed: {}", attempt + 1, e);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    Err(e) => log::error!(
                        "Could not restore patched piece ledger ({}); next download will run a full re-check",
                        e
                    ),
                }
            }
            if restored {
                log::info!(
                    "Restored piece ledger with {} file(s) invalidated - next add skips the full re-check",
                    deleted_indices.len()
                );
            }
        }
        drop(handle_guard);
        log::info!(
            "Dropped torrent from session after uninstall ({} files still selected)",
            remaining.len()
        );
        if !remaining.is_empty() {
            // Re-add immediately so other in-flight downloads continue.
            self.download_files(remaining).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::clear_file_pieces;

    #[test]
    fn clear_file_pieces_clears_exactly_the_overlapping_range() {
        // 32 pieces, all "have". File spans bytes 1000..5000 with 1024-byte
        // pieces -> pieces 0..=4 inclusive (offset 1000 is in piece 0,
        // last byte 4999 is in piece 4).
        let mut bytes = vec![0xFFu8; 4];
        clear_file_pieces(&mut bytes, 1000, 4000, 1024);
        assert_eq!(bytes[0], 0b0000_0111, "pieces 0-4 cleared (MSB0)");
        assert_eq!(&bytes[1..], &[0xFF, 0xFF, 0xFF], "later pieces untouched");
    }

    #[test]
    fn clear_file_pieces_handles_byte_boundaries_and_empty() {
        let mut bytes = vec![0xFFu8; 2];
        // File exactly covering pieces 7..=8 (crosses the byte boundary).
        clear_file_pieces(&mut bytes, 7 * 64, 2 * 64, 64);
        assert_eq!(bytes[0], 0b1111_1110);
        assert_eq!(bytes[1], 0b0111_1111);
        // Zero-size file: no-op.
        let mut untouched = vec![0xFFu8; 2];
        clear_file_pieces(&mut untouched, 100, 0, 64);
        assert_eq!(untouched, vec![0xFF, 0xFF]);
    }
}
