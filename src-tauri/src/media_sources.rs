//! The eXoDOS Media Pack as a torrent source that is NOT a collection (§19).
//!
//! It carries no games, so it stays out of `COLLECTION_MAP` - every collection
//! surface is a projection of that table, and `enable_new_collections` would
//! hand a 237 GB torrent to every existing install on upgrade. The manager is
//! created on the first fetch instead, on the session the collections already
//! run, and lives for the rest of the session.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::commands::paths::bundled_torrent_path;
use crate::commands::TorrentState;
use crate::torrent::manager::DownloadManager;

pub struct MediaSource {
    pub id: &'static str,
    pub torrent_file: &'static str,
}

/// The media source's id. Every other source an issue names is a collection,
/// whose manager comes from `TorrentState` (§19).
pub const MEDIA_SOURCE: &str = "eXoMedia";

pub const MEDIA_SOURCES: &[MediaSource] = &[MediaSource {
    id: MEDIA_SOURCE,
    torrent_file: "eXoDOS Media Pack.torrent",
}];

pub fn media_source(id: &str) -> Option<&'static MediaSource> {
    MEDIA_SOURCES.iter().find(|s| s.id == id)
}

/// Lazily created media managers, keyed by source id.
#[derive(Default)]
pub struct MediaTorrentState(pub RwLock<HashMap<String, Arc<DownloadManager>>>);

impl MediaTorrentState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dropped wherever the collections' managers are: they share one session,
    /// so a media manager left behind keeps it alive and the app goes on
    /// downloading and seeding after it was told to stop (§11).
    pub async fn clear(&self) {
        self.0.write().await.clear();
    }
}

/// Give every torrent in the session the union of all their file lists as its
/// placeholder-cleanup keep-list. They all write into one root and cleanup
/// deletes whatever no torrent claims, so this is re-unioned the moment a
/// torrent joins - collections and media sources alike (§19).
pub fn apply_union_keep_paths<'a, I>(managers: I)
where
    I: Iterator<Item = &'a Arc<DownloadManager>> + Clone,
{
    let union: Arc<Vec<String>> = Arc::new(
        managers
            .clone()
            .flat_map(|m| m.index().files.iter().map(|f| f.path.clone()))
            .collect(),
    );
    for mgr in managers {
        mgr.set_cleanup_keep_paths(Arc::clone(&union));
    }
}

/// The manager for `id`, created on first use. `None` means there is no
/// session to join - offline, or setup never finished (§11).
pub async fn ensure_manager(
    torrent_state: &TorrentState,
    media_state: &MediaTorrentState,
    id: &str,
) -> Option<Arc<DownloadManager>> {
    // Every collection manager holds the same session; any of them can lend
    // it, and if there is none the app is offline by definition. Asked BEFORE
    // the cache: a cached manager outlives the session it was built on, and
    // answering from it would read as online while the engine is down.
    let collections: Vec<Arc<DownloadManager>> =
        torrent_state.0.read().await.values().cloned().collect();
    let host = collections.first()?;

    // Held across the whole creation: two issues opened at once would
    // otherwise both add the torrent and both re-union the keep list.
    let mut managers = media_state.0.write().await;
    if let Some(existing) = managers.get(id) {
        return Some(Arc::clone(existing));
    }
    let source = media_source(id)?;

    let torrent_path = bundled_torrent_path(source.torrent_file)
        .map_err(|e| log::warn!("{}: {}", source.id, e))
        .ok()?;
    let manager = DownloadManager::new_with_session(
        host.session(),
        &torrent_path,
        host.data_dir(),
        host.persistence_dir(),
    )
    .map_err(|e| log::warn!("{}: could not join the session: {}", source.id, e))
    .ok()?;
    let manager = Arc::new(manager);

    apply_union_keep_paths(
        managers
            .values()
            .chain(collections.iter())
            .chain(std::iter::once(&manager)),
    );

    manager.hydrate_from_session().await;
    log::info!("{}: joined the session ({} files)", source.id, manager.index().files.len());
    managers.insert(id.to_string(), Arc::clone(&manager));
    Some(manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No collection manager means no session, and the Media Pack has none of
    /// its own - the reading room is then offline, not broken.
    #[tokio::test]
    async fn without_a_collection_manager_there_is_nothing_to_join() {
        let torrents = TorrentState(RwLock::new(HashMap::new()));
        let media = MediaTorrentState::new();
        assert!(ensure_manager(&torrents, &media, MEDIA_SOURCE).await.is_none());
        assert!(media.0.read().await.is_empty());
    }
}
