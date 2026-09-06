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

/// Apply the effective transfer caps to a session.
///
/// Free-standing because two callers need it: `DownloadManager::apply_limits`
/// for a running session, and `init_download_manager` for one that was just
/// created (a new session starts unlimited in both directions). Both MUST go
/// through here - seeding and the user's upload limit write the same knob, and
/// a second copy of this rule would drift from the first.
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

/// Convert a path to its NT extended-length form on Windows (`\\?\C:\...` or
/// `\\?\UNC\server\share\...`). On other platforms this is a no-op.
///
/// The prefix tells the Win32 API to skip path normalization and the
/// MAX_PATH (260) check, allowing paths up to 32 767 characters. librqbit
/// passes the output folder verbatim to the file writer, so prefixing it
/// here is enough - every file it later opens inherits the long-path mode.
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

/// Remove 0-byte zip files in `root` that are NOT part of the current torrent
/// - i.e. true orphans from a previous run or unrelated user files.
///
/// `keep_paths` must contain the **full set** of the torrent's file paths
/// (forward-slashed, as produced by `TorrentIndex::from_file`), not just the
/// user's current selection. librqbit's `init()` creates a 0-byte placeholder
/// for **every** file declared by the torrent, regardless of `only_files`.
/// With fastresume's piece-cache (v0.6.4+), pieces shared between files get
/// marked "had" once any selected file's pieces arrive - and librqbit will
/// then refuse to re-download those pieces even if some of their target files
/// were deleted. Deleting a tracked placeholder therefore puts librqbit's
/// in-memory state at odds with disk: `file_progress` reports 100% complete
/// while `<file>.zip` is gone, and the user is stuck in an extraction loop
/// that never resolves (observed v0.6.6 with Dominium 762/762 bytes "100%"
/// but the zip never on disk).
///
/// To make the match work on Windows - where `WalkDir` yields backslash-
/// separated paths - we normalize each on-disk entry's string form to forward
/// slashes before comparing.
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
    /// Torrent lifecycle state from librqbit. During the `initializing` phase
    /// librqbit hashes the entire torrent's existing on-disk content before
    /// any peer pieces are requested - on Windows with thousands of placeholder
    /// files this can take several minutes, and per-file `progress` will stay
    /// at 0 the whole time. The frontend uses this to show a meaningful
    /// "Validating…" status instead of a frozen 0%.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_state: Option<String>,
    /// Whole-torrent validation/download progress (0.0..1.0). During init this
    /// reflects librqbit's hash-check progress; once live, it tracks downloaded
    /// bytes across all selected files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_progress: Option<f64>,
    /// Progress (0.0..1.0) of the game's extras (GameData: manuals, videos,
    /// music). Downloads continue after the game itself is installed and
    /// playable - surfaced so the UI can show the second phase instead of
    /// letting it finish invisibly.
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

/// Manages BitTorrent downloads using librqbit with selective file support.
/// Must be Send+Sync for use in Tauri's managed state.
///
/// The torrent is only added to the session on first download request,
/// avoiding the creation of 14,000+ placeholder files at startup.
///
/// Lock-order convention: `selection_apply` before `handle` before
/// `selected_files`, always. Mixed order deadlocks - tokio's
/// write-preferring RwLock blocks new readers while a writer waits, so two
/// tasks holding one lock each and waiting on the other freeze every
/// download.
pub struct DownloadManager {
    session: Arc<Session>,
    handle: RwLock<Option<Arc<ManagedTorrent>>>,
    torrent_index: TorrentIndex,
    torrent_bytes: Arc<Vec<u8>>,
    selected_files: RwLock<HashSet<usize>>,
    /// Serializes every `update_only_files` push into librqbit (including
    /// its wait-for-check preamble). Two concurrent selection updates - or
    /// one racing the re-check a previous update triggered - wedge
    /// librqbit's checking task so hard that even `stats()` blocks forever,
    /// freezing every progress poll (field report: three first downloads
    /// clicked in quick succession on Linux 0.8.7, UI stuck on "Starting
    /// download..." with a silent log).
    selection_apply: tokio::sync::Mutex<()>,
    data_dir: PathBuf,
    /// Hex SHA1 info-hash of this manager's torrent, for finding it among
    /// the session's persisted (auto-resumed) torrents.
    info_hash_hex: Option<String>,
    /// Where librqbit keeps <hash>.bitv piece-ledger files - needed for the
    /// surgical ledger patch on uninstall.
    persistence_dir: PathBuf,
    /// Torrent-relative paths that placeholder cleanup must never delete.
    /// All four eXoDOS torrents overlay into the same root, so this must be
    /// the UNION of every enabled collection's file list (set after all
    /// managers are built) - cleaning with only this torrent's list deletes
    /// placeholders that sibling torrents still track (cross-collection
    /// variant of the v0.6.6 stuck-download bug). Falls back to this
    /// torrent's own list when unset.
    cleanup_keep_paths: std::sync::RwLock<Option<Arc<Vec<String>>>>,
}

/// Default location for librqbit's fastresume persistence (`<info_hash>.bitv`,
/// `<info_hash>.torrent`, `session.json`). Co-located with the session dir so
/// it shares the same lifecycle (cleared by factory_reset).
pub(crate) fn fastresume_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("librqbit-fastresume")
}

impl DownloadManager {
    /// Create a shared librqbit session. Call once, then pass to `new_with_session`.
    /// `session_dir` is where librqbit stores its internal state (.librqbit/).
    /// This should be the app config directory, NOT the game data directory.
    ///
    /// `persistence_dir` is where fastresume bitfields, per-torrent .torrent
    /// copies and session.json live. Pre-seeding `<info_hash>.bitv` files in
    /// here before this call lets librqbit skip its initial checksum pass on
    /// fresh installs - see `setup::seed_fastresume_bitvs`.
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
                // fastresume + JSON persistence: librqbit caches the per-torrent
                // have-pieces bitfield to `<persistence_dir>/<info_hash>.bitv`.
                // On subsequent adds (or after we plant an empty bitfield for a
                // fresh install) librqbit skips the initial_check pass entirely
                // - turning a 5-10 minute Windows wait into seconds.
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

    /// Adopt this manager's torrent if the session auto-resumed it from JSON
    /// persistence. Without this, a download in flight at last shutdown keeps
    /// downloading inside librqbit after restart, but the manager reports no
    /// progress (handle = None) and the next add_torrent would apply a fresh
    /// selection that silently deselects the resumed files. Returns true when
    /// a session torrent was adopted.
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

    /// The one place that sets the session's transfer caps.
    ///
    /// Seeding and the user's upload limit write the same knob, so they cannot
    /// be applied independently - whoever ran last would win, and turning
    /// sharing off after setting a limit would silently lift the cap. Seeding
    /// off always wins: librqbit has no runtime upload kill-switch, and 1 KB/s
    /// keeps protocol handshakes working while making uploads negligible.
    ///
    /// Limits are in KB/s, `None` meaning unlimited. The session is shared
    /// across collections, so calling this on any one manager affects all.
    /// Live transfer rates for the whole session.
    ///
    /// Session-wide on purpose: all collections share one session, so this is
    /// one cheap read instead of summing per-torrent stats - and `stats()`
    /// copies a 15,000-entry file-progress vector plus a path String per
    /// selected file, which is a lot of work to poll for three numbers.
    pub fn session_transfer(&self) -> SessionTransfer {
        let snap = self.session.stats_snapshot();
        SessionTransfer {
            download_bps: snap.download_speed.as_bytes(),
            upload_bps: snap.upload_speed.as_bytes(),
            uploaded_bytes: snap.counters.uploaded_bytes,
            peers: snap.peers.live,
        }
    }

    pub fn apply_limits(&self, seeding: bool, up_kbps: Option<u32>, down_kbps: Option<u32>) {
        apply_session_limits(&self.session, seeding, up_kbps, down_kbps);
    }

    /// Stop the shared librqbit session: aborts live torrents and flushes
    /// persistence, so callers can delete data files without a writer task
    /// racing the delete and re-creating them. The session is shared across
    /// collections - stopping via any one manager stops them all.
    pub async fn shutdown_session(&self) {
        self.session.stop().await;
    }

    /// Returns true if the given file index has been queued for download.
    pub async fn is_file_selected(&self, file_index: usize) -> bool {
        self.selected_files.read().await.contains(&file_index)
    }

    /// The directory this torrent's files live in.
    ///
    /// ONE root for every collection, not `<data_dir>/<torrent name>`: eXo's
    /// packs are separate torrents but a single installation (their
    /// Setup/eXoMerge bats merge `eXo\` and `Content\` into one folder), and
    /// their file paths - `eXo/eXoDOS/…`, `eXo/eXoWin9x/…` - are built to sit
    /// side by side. Following librqbit's naming instead gave every pack its
    /// own tree, which no eXo tool produces and which made an imported
    /// installation look half-empty.
    pub fn torrent_root(&self) -> PathBuf {
        crate::commands::setup::game_root(&self.data_dir.to_string_lossy())
    }

    /// Wait out an in-progress initial check, then apply the CURRENT
    /// selection (re-read at apply time, so concurrent waiters converge on
    /// the same final set).
    ///
    /// Pushing update_only_files into an initializing torrent has wedged
    /// librqbit's checking task in the field (Windows, uninstall -> re-add,
    /// "Validating N%" frozen forever) - twice, both times when a selection
    /// update raced the check. So: wait for the check to finish first. Must
    /// NOT be called while holding the handle lock - a full re-check takes
    /// minutes and progress polling reads that lock.
    ///
    /// The `selection_apply` mutex makes appliers mutually exclusive: an
    /// update can also race the re-check that a PREVIOUS update triggered
    /// (three games queued back-to-back on a fresh collection), which
    /// wedges librqbit just the same. Each applier therefore re-runs the
    /// wait loop while already holding the mutex.
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
        // Small retry tail for the init->live transition race. The
        // selected_files read lock is HELD across the apply so a concurrent
        // deselect_file (write lock) strictly serializes after us and
        // re-applies the newer, smaller set - without this a cancelled file
        // could be resurrected inside librqbit by our in-flight apply.
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

    /// Open a streaming reader over ONE file without selecting it for
    /// download. The stream's own read position drives which pieces librqbit
    /// fetches, which is what lets a 2.5 MB video come out of a 1.1 GB archive
    /// without pulling the archive.
    ///
    /// Adds the torrent if it isn't running yet, reusing `download_files` with
    /// an empty index list rather than duplicating that (delicate) add path.
    /// librqbit's `FileStream` type lives in a private module and cannot be
    /// named from here, so the reader is returned by its capabilities - which
    /// is all `zip_range` needs anyway.
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
            // Explicitly set output_folder to torrent_root so downloads land in data_dir,
            // not in the session's default output folder (which is app_data_dir).
            // On Windows, prefix with \\?\ so file writes use the NT extended-length
            // path API (32 768-char limit) instead of the legacy MAX_PATH (260).
            // Without this, deeply nested torrent entries silently fail to open and
            // the download appears stuck at 0%.
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

            // If the session already had this torrent, apply our file selection.
            // Waits out any in-progress initial check first (see
            // wait_ready_then_apply_selection) - done AFTER dropping the
            // handle lock below, via `pending_selection_update`.
            if already_managed {
                // Merge librqbit's current selection first: the session may
                // have auto-resumed files from a previous run that this
                // manager doesn't know about - replacing the selection
                // outright would silently deselect them mid-download.
                let session_selection = handle.only_files().unwrap_or_default();
                let mut selected = self.selected_files.write().await;
                selected.extend(session_selection.iter().copied());
            }

            // Periodic diagnostic stats: log peer count, live state, and
            // download speed every 2 s for 60 s after a fresh add. This is
            // the window during which Windows-stuck-at-0% manifests; without
            // these snapshots we have no signal to tell network/peer issues
            // (peers=0) apart from disk/librqbit issues (peers>0, speed=0).
            // Capture file paths up front so the spawned task does not need
            // a reference to self.
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
                    // The Display impl gives us state + progress + (when live)
                    // download/upload speeds - the most diagnostic-dense
                    // single line we can emit. Augment with the per-file
                    // breakdown so we can tell partial progress apart.
                    let per_file: Vec<String> = watched_files.iter().map(|(idx, name, size)| {
                        let dl = s.file_progress.get(*idx).copied().unwrap_or(0);
                        let pct = if *size > 0 { (dl as f64 / *size as f64) * 100.0 } else { 0.0 };
                        format!("[{}]={}/{} ({:.1}%) {}", idx, dl, size, pct, name)
                    }).collect();
                    if let Some(ref err) = s.error {
                        // Also fires when the torrent was removed from the
                        // session (uninstall invalidation) - stats() then
                        // reports a broken "None" state every poll. Log once
                        // and stop instead of spamming for the full 60 s.
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

            // Cleanup: removes 0-byte zip files that are NOT in the torrent's
            // file list (true orphans from a previous run / unrelated files).
            //
            // Critical: pass the FULL torrent file list, not just the user's
            // current selection. librqbit's `init()` opens (creates) every
            // file declared by the torrent, so all 14k+ slots exist as 0-byte
            // sparse files immediately after add. With fastresume enabled
            // (v0.6.4+), pieces shared between files get marked "have" once
            // any selected file's pieces arrive - and librqbit then refuses
            // to re-download those pieces, even if some target files were
            // deleted. Deleting a tracked placeholder therefore makes
            // librqbit's in-memory state lie about disk state, leaving the
            // user stuck in a "100% but zip missing" loop on subsequent
            // downloads (observed v0.6.4-v0.6.6).
            // ... and because all collections share one overlay root, prefer
            // the union keep-list over this torrent's own file list.
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

    /// Remove a file from the active selection, telling librqbit to stop prioritising it.
    /// Mutates the set immediately, then pushes it to the session via the
    /// serialized applier in the background: a direct update_only_files here
    /// was the second unserialized caller (besides queueing) able to race a
    /// check and wedge librqbit. The applier reads the CURRENT set at apply
    /// time, so this reduced selection wins over any apply already in
    /// flight, and cancel stays instant even while a long check runs.
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

    /// Read this torrent's on-disk piece ledger and clear the bits of every
    /// piece overlapping the given files. The on-disk snapshot can lag the
    /// in-memory bitfield by one flush interval, so pieces downloaded in the
    /// final seconds before an uninstall may re-download - bounded, harmless. Returns (path, patched bytes), or
    /// None when anything looks unexpected (missing ledger, zero piece
    /// length, size mismatch) - callers then fall back to the full re-check.
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

    /// Reset librqbit's piece bookkeeping after a tracked file was deleted
    /// from disk (uninstall). The persisted fastresume bitfield still claims
    /// the deleted file's pieces exist, so a re-download would instantly
    /// report 100% with no file on disk - the unrecoverable "stuck at 100%"
    /// loop. Dropping the torrent from the session deletes its .bitv +
    /// session entry; the next download re-adds it and re-derives state from
    /// what is actually on disk (an initial hash-check pass, so still-selected
    /// downloads keep their completed pieces).
    ///
    /// `drop_indices`: file indices to remove from the selection first
    /// (the uninstalled game's own files), so the re-add doesn't fetch them.
    /// `deleted_indices`: the subset whose files were actually DELETED from
    /// disk - only their pieces are cleared in the ledger. Clearing the bits
    /// of merely-deselected files (e.g. a shared GameData ZIP still on disk)
    /// would force a pointless multi-GB re-download on the next install.
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
        // Surgical ledger patch: snapshot the piece bitfield, clear only the
        // deleted files' pieces, and restore it after session.delete (which
        // erases the file). The next add loads it via fastresume and skips
        // the full re-check - which took 15-30 min in the field. Boundary
        // pieces shared with neighbors are cleared too (they just get
        // re-fetched); on any format surprise we fall back to the full check.
        let patched_bitv = self.patched_bitv_without(deleted_indices);
        self.session
            .delete(librqbit::api::TorrentIdOrHash::Hash(handle.info_hash()), false)
            .await?;
        if let Some((path, bytes)) = patched_bitv {
            // librqbit's async bitv flusher may hold the old file's handle
            // for a moment after session.delete; on filesystems without
            // POSIX delete semantics (exFAT/SMB/older NTFS) that leaves the
            // name delete-pending and a write fails transiently. Write to a
            // temp name and rename with a short retry so the optimization
            // doesn't silently degrade to the full re-check there.
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
