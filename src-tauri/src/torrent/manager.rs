use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, SessionOptions,
};
use serde::Serialize;
use tokio::sync::RwLock;

use walkdir::WalkDir;

use super::TorrentIndex;

/// Remove 0-byte placeholder files created by librqbit, except those being downloaded.
fn cleanup_placeholder_files(root: &Path, keep: &HashSet<PathBuf>) -> std::io::Result<()> {
    let mut removed = 0;
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            if let Ok(meta) = path.metadata() {
                if meta.len() == 0 && !keep.contains(path)
                    && path.extension().map(|e| e == "zip").unwrap_or(false)
                {
                    let _ = std::fs::remove_file(path);
                    removed += 1;
                }
            }
        }
    }
    // Remove empty directories left behind
    for entry in WalkDir::new(root).contents_first(true).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() && path != root {
            let _ = std::fs::remove_dir(path);
        }
    }
    if removed > 0 {
        log::info!("Cleaned up {} placeholder files", removed);
    }
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
    /// Set by the command layer after checking DB — true once extracted and marked installed.
    #[serde(default)]
    pub installed: bool,
    /// Optional error/status message from the command layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadManagerStatus {
    pub active_downloads: Vec<DownloadProgress>,
    pub download_speed: Option<String>,
    pub upload_speed: Option<String>,
}

/// Manages BitTorrent downloads using librqbit with selective file support.
/// Must be Send+Sync for use in Tauri's managed state.
///
/// The torrent is only added to the session on first download request,
/// avoiding the creation of 14,000+ placeholder files at startup.
pub struct DownloadManager {
    session: Arc<Session>,
    handle: RwLock<Option<Arc<ManagedTorrent>>>,
    torrent_index: TorrentIndex,
    torrent_bytes: Arc<Vec<u8>>,
    selected_files: RwLock<HashSet<usize>>,
    data_dir: PathBuf,
}

impl DownloadManager {
    /// Create a shared librqbit session. Call once, then pass to `new_with_session`.
    pub async fn create_session(data_dir: &Path) -> anyhow::Result<Arc<Session>> {
        std::fs::create_dir_all(data_dir)?;
        let session = Session::new_with_opts(
            data_dir.to_path_buf(),
            SessionOptions {
                disable_dht: false,
                disable_dht_persistence: true,
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
    ) -> anyhow::Result<Self> {
        let torrent_bytes = Arc::new(std::fs::read(torrent_path)?);
        let torrent_index = TorrentIndex::from_file(torrent_path)
            .map_err(|e| anyhow::anyhow!("{}", e))?;

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
            data_dir: data_dir.to_path_buf(),
        })
    }

    /// Convenience: create session + manager in one call (for single-torrent use).
    pub async fn new(torrent_path: &Path, data_dir: &Path) -> anyhow::Result<Self> {
        let session = Self::create_session(data_dir).await?;
        Self::new_with_session(session, torrent_path, data_dir)
    }

    /// Get the torrent file index.
    pub fn index(&self) -> &TorrentIndex {
        &self.torrent_index
    }

    /// Returns true if the given file index has been queued for download.
    pub async fn is_file_selected(&self, file_index: usize) -> bool {
        self.selected_files.read().await.contains(&file_index)
    }

    /// Get the torrent root directory: <data_dir>/<torrent_name>/
    pub fn torrent_root(&self) -> PathBuf {
        self.data_dir.join(&self.torrent_index.name)
    }

    /// Queue file indices for download. Adds the torrent on first call.
    pub async fn download_files(&self, file_indices: Vec<usize>) -> anyhow::Result<()> {
        {
            let mut selected = self.selected_files.write().await;
            for idx in &file_indices {
                selected.insert(*idx);
            }
        }

        let mut handle_guard = self.handle.write().await;

        if let Some(ref handle) = *handle_guard {
            // Torrent already running — just update file selection
            let selected = self.selected_files.read().await;
            self.session.update_only_files(handle, &selected).await?;
            log::info!("Updated file selection, added: {:?}", file_indices);
        } else {
            // First download — add torrent to session now
            let selected = self.selected_files.read().await.clone();
            let response = self
                .session
                .add_torrent(
                    AddTorrent::from_bytes((*self.torrent_bytes).clone()),
                    Some(AddTorrentOptions {
                        only_files: Some(selected.into_iter().collect()),
                        overwrite: true,
                        ..Default::default()
                    }),
                )
                .await?;

            let handle = match response {
                AddTorrentResponse::Added(_id, h) => h,
                AddTorrentResponse::AlreadyManaged(_id, h) => h,
                AddTorrentResponse::ListOnly(_) => {
                    return Err(anyhow::anyhow!("Torrent added in list-only mode"));
                }
            };

            *handle_guard = Some(handle);
            log::info!("Torrent added, downloading files: {:?}", file_indices);

            // librqbit creates 0-byte placeholder files for all torrent entries.
            // Clean them up, but keep placeholders for files we're downloading.
            let root = self.torrent_root();
            let keep: HashSet<PathBuf> = self.selected_files.read().await.iter()
                .filter_map(|&idx| self.torrent_index.files.get(idx))
                .map(|f| root.join(&f.path))
                .collect();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if let Err(e) = cleanup_placeholder_files(&root, &keep) {
                    log::warn!("Failed to clean up placeholder files: {}", e);
                }
            });
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

        Some(DownloadProgress {
            file_index,
            file_name,
            downloaded_bytes: downloaded,
            total_bytes: total,
            progress,
            finished,
            installed: false,
            error: None,
        })
    }

    /// Get status for all active downloads.
    pub async fn status(&self) -> DownloadManagerStatus {
        let selected = self.selected_files.read().await;
        let handle_guard = self.handle.read().await;

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
                    });
                }
            }
        }

        let (download_speed, upload_speed) = handle_guard
            .as_ref()
            .map(|h| {
                let s = h.stats();
                (
                    s.live.as_ref().map(|l| l.download_speed.to_string()),
                    s.live.as_ref().map(|l| l.upload_speed.to_string()),
                )
            })
            .unwrap_or((None, None));

        DownloadManagerStatus {
            active_downloads,
            download_speed,
            upload_speed,
        }
    }

    /// Remove a file from the active selection, telling librqbit to stop prioritising it.
    /// Holds the write lock across the session update to keep selected_files and the
    /// torrent session in sync — no other caller can observe a partially-updated state.
    pub async fn deselect_file(&self, file_index: usize) {
        let mut selected = self.selected_files.write().await;
        selected.remove(&file_index);
        let handle_guard = self.handle.read().await;
        if let Some(ref handle) = *handle_guard {
            let _ = self.session.update_only_files(handle, &*selected).await;
        }
    }

    /// Check if a specific file has finished downloading.
    pub async fn is_file_complete(&self, file_index: usize) -> bool {
        self.file_progress(file_index)
            .await
            .map(|p| p.finished)
            .unwrap_or(false)
    }

    /// Wait for a specific file to complete downloading.
    pub async fn wait_for_file(&self, file_index: usize) -> anyhow::Result<()> {
        loop {
            if self.is_file_complete(file_index).await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Get the output path for a downloaded file.
    pub fn file_output_path(&self, file_index: usize) -> Option<PathBuf> {
        let entry = self.torrent_index.files.get(file_index)?;
        Some(self.torrent_root().join(&entry.path))
    }
}
