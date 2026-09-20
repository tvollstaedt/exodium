//! Whether a game's archive outlives its extraction, and what a game costs
//! on disk (§5).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::db::queries;
use crate::models::Game;
use crate::torrent::manager::DownloadManager;

use super::collections::{collection_rel_game_dir, collection_rel_zip};
use super::games::{configured_data_dir, game_op_lock};
use super::library::game_name_from_app_path;
use super::lp_overlay::dir_size;
use super::paths::game_root;
use super::user_data::PristineIndex;
use super::{DbState, TorrentState};

pub(crate) const KEEP_ARCHIVES_KEY: &str = "keep_archives";

/// Unset means keep: Reset and seeding need the file.
pub(crate) fn keep_archives(conn: &rusqlite::Connection) -> bool {
    queries::get_config(conn, KEEP_ARCHIVES_KEY).ok().flatten().as_deref() != Some("0")
}

/// Where a dropped archive's manifest lives; `content/` goes with a factory
/// reset, exactly like the games it describes.
pub(crate) fn manifest_path(data_dir: &Path, source: &str, file_index: usize) -> PathBuf {
    data_dir.join("content").join("pristine").join(format!("{source}_{file_index}.idx"))
}

pub(crate) fn zip_path_of(torrent_root: &Path, game: &Game) -> PathBuf {
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let app_path = game.application_path.as_deref();
    let name = app_path
        .and_then(game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());
    torrent_root.join(collection_rel_zip(source, &name, app_path))
}

/// The archive while it is on disk, else the manifest written when it was
/// dropped; None when neither exists (the uninstall then keeps everything).
pub(crate) fn pristine_for(data_dir: &Path, torrent_root: &Path, game: &Game) -> Option<PristineIndex> {
    if let Some(index) = PristineIndex::from_zip(&zip_path_of(torrent_root, game)) {
        return Some(index);
    }
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let file_index = usize::try_from(game.game_torrent_index?).ok()?;
    PristineIndex::from_manifest(&manifest_path(data_dir, source, file_index))
}

/// Manifest first, then the file. The caller invalidates the torrent's
/// ledger afterwards (`invalidate_after_file_delete`), or the session goes
/// on believing it has the pieces.
pub(crate) fn unlink_archive(zip: &Path, manifest: &Path) -> Result<u64, String> {
    let index = PristineIndex::from_zip(zip)
        .ok_or_else(|| format!("{} is not an archive", zip.display()))?;
    index
        .write_manifest(manifest)
        .map_err(|e| format!("cannot write {}: {e}", manifest.display()))?;
    let bytes = std::fs::metadata(zip).map(|m| m.len()).unwrap_or(0);
    std::fs::remove_file(zip).map_err(|e| format!("cannot remove {}: {e}", zip.display()))?;
    Ok(bytes)
}

/// One installed game's archive after extraction, when the setting says so.
pub(crate) async fn drop_archive_after_install(
    mgr: &Arc<DownloadManager>,
    source: &str,
    file_index: usize,
    zip: &Path,
    title: &str,
) {
    let manifest = manifest_path(mgr.data_dir(), source, file_index);
    let zip = zip.to_path_buf();
    let unlinked = tauri::async_runtime::spawn_blocking(move || unlink_archive(&zip, &manifest))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match unlinked {
        Ok(bytes) => {
            log::info!("Dropped archive of {title} after install ({bytes} bytes)");
            mgr.schedule_invalidation(file_index);
        }
        Err(e) => log::warn!("Archive of {title} kept: {e}"),
    }
}

#[derive(Serialize, Debug, Default, Clone, Copy)]
pub struct ArchiveUsage {
    pub count: usize,
    pub bytes: u64,
}

struct InstalledArchive {
    id: i64,
    source: String,
    file_index: usize,
    zip: PathBuf,
    bytes: u64,
}

/// Archives of games that are unpacked and on disk as real archives. Bytes
/// are allocated size: a sparse placeholder costs nothing and says so.
fn installed_archives(conn: &rusqlite::Connection, torrent_root: &Path) -> Vec<InstalledArchive> {
    let mut out = Vec::new();
    for game in queries::fetch_installed_rows(conn).unwrap_or_default() {
        let (Some(id), Some(idx), Some(shortcode)) = (game.id, game.game_torrent_index, game.shortcode.as_deref()) else { continue };
        let source = game.torrent_source.clone().unwrap_or_else(|| "eXoDOS".into());
        let dir = torrent_root.join(collection_rel_game_dir(&source, shortcode, game.application_path.as_deref()));
        if !dir.is_dir() {
            continue;
        }
        let zip = zip_path_of(torrent_root, &game);
        let Some(bytes) = real_archive_bytes(&zip) else { continue };
        out.push(InstalledArchive { id, source, file_index: idx as usize, zip, bytes });
    }
    out
}

/// Allocated bytes of a file that opens as a zip; None otherwise.
pub(crate) fn real_archive_bytes(zip: &Path) -> Option<u64> {
    use fs4::FileExt;
    let file = std::fs::File::open(zip).ok()?;
    let allocated = file.allocated_size().ok()?;
    zip::ZipArchive::new(file).ok()?;
    Some(allocated)
}

#[tauri::command]
pub async fn archive_usage(db_state: State<'_, DbState>) -> Result<ArchiveUsage, String> {
    let (db_path, root) = {
        let conn = db_state.lock()?;
        let data_dir = configured_data_dir(&conn)?;
        let db_path = conn.path().map(PathBuf::from).ok_or("Cannot determine database path")?;
        (db_path, game_root(&data_dir))
    };
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open(&db_path).map_err(|e| e.to_string())?;
        let list = installed_archives(&conn, &root);
        Ok(ArchiveUsage { count: list.len(), bytes: list.iter().map(|a| a.bytes).sum() })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Removes the archives of every installed game. A game with an operation in
/// flight is skipped; one ledger update per torrent. Only sources with a
/// live manager: a deleted file whose pieces the ledger still claims would
/// be seeded as garbage and re-download as "100% with nothing on disk".
#[tauri::command]
pub async fn remove_installed_archives(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<ArchiveUsage, String> {
    let (list, data_dir) = {
        let conn = db_state.lock()?;
        let data_dir = configured_data_dir(&conn)?;
        (installed_archives(&conn, &game_root(&data_dir)), data_dir)
    };
    let managers: Vec<(String, Arc<DownloadManager>)> = {
        let guard = torrent_state.0.read().await;
        guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    if managers.is_empty() {
        return Err("Archives can only be removed while online: the download client has to forget them.".into());
    }
    let mut guards = Vec::new();
    let mut removed: Vec<(String, usize, u64)> = Vec::new();
    for entry in list {
        if !managers.iter().any(|(id, _)| *id == entry.source) {
            continue;
        }
        let lock = game_op_lock(entry.id);
        let Ok(guard) = lock.clone().try_lock_owned() else { continue };
        let manifest = manifest_path(Path::new(&data_dir), &entry.source, entry.file_index);
        let zip = entry.zip.clone();
        let result = tauri::async_runtime::spawn_blocking(move || unlink_archive(&zip, &manifest))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        match result {
            Ok(_) => removed.push((entry.source, entry.file_index, entry.bytes)),
            Err(e) => log::warn!("remove_installed_archives: {e}"),
        }
        guards.push(guard);
    }
    for (col_id, mgr) in managers {
        let indices: Vec<usize> = removed.iter().filter(|(s, _, _)| *s == col_id).map(|(_, i, _)| *i).collect();
        if indices.is_empty() {
            continue;
        }
        if let Err(e) = mgr.invalidate_after_file_delete(&indices, &indices).await {
            log::warn!("Ledger of {col_id} not updated after removing archives: {e}");
        }
    }
    drop(guards);
    Ok(ArchiveUsage { count: removed.len(), bytes: removed.iter().map(|(_, _, b)| *b).sum() })
}

#[derive(Serialize, Debug, Default, Clone, Copy)]
pub struct GameDiskUsage {
    /// The unpacked game directory.
    pub game_bytes: u64,
    /// The archive, allocated size; 0 for a placeholder or a dropped one.
    pub archive_bytes: u64,
    /// The `!save` backup an uninstall left behind.
    pub save_bytes: u64,
}

#[tauri::command]
pub async fn game_disk_usage(db_state: State<'_, DbState>, id: i64) -> Result<GameDiskUsage, String> {
    let (game, root) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {id} not found"))?;
        let data_dir = configured_data_dir(&conn)?;
        (game, game_root(&data_dir))
    };
    tauri::async_runtime::spawn_blocking(move || {
        let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
        let Some(shortcode) = game.shortcode.as_deref() else { return GameDiskUsage::default() };
        let rel_dir = collection_rel_game_dir(source, shortcode, game.application_path.as_deref());
        let game_dir = root.join(&rel_dir);
        let save_dir = match rel_dir.rsplit_once('/') {
            Some((parent, leaf)) => root.join(parent).join("!save").join(leaf),
            None => root.join("!save").join(&rel_dir),
        };
        let measure = |d: &Path| if d.is_dir() { dir_size(d) } else { 0 };
        GameDiskUsage {
            game_bytes: measure(&game_dir),
            archive_bytes: real_archive_bytes(&zip_path_of(&root, &game)).unwrap_or(0),
            save_bytes: measure(&save_dir),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest answers once the archive is gone - the difference between
    /// a two-file save backup and a whole game folder.
    #[test]
    fn pristine_falls_back_to_the_manifest_when_the_archive_is_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let root = data_dir.join("eXoDOS");
        std::fs::create_dir_all(root.join("eXo/eXoDOS")).unwrap();
        let game = Game {
            title: "Space Quest V".into(),
            shortcode: Some("SQ5".into()),
            torrent_source: Some("eXoDOS".into()),
            application_path: Some(r"eXo\eXoDOS\!dos\SQ5\Space Quest V (1993).bat".into()),
            game_torrent_index: Some(7),
            ..Game::default()
        };
        let zip = zip_path_of(&root, &game);
        {
            let file = std::fs::File::create(&zip).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            w.start_file("SQ5/SQ5.EXE", opts).unwrap();
            std::io::Write::write_all(&mut w, b"exe").unwrap();
            w.finish().unwrap();
        }
        assert!(pristine_for(data_dir, &root, &game).is_some(), "from the archive");
        unlink_archive(&zip, &manifest_path(data_dir, "eXoDOS", 7)).unwrap();
        assert!(!zip.exists());
        assert!(pristine_for(data_dir, &root, &game).is_some(), "from the manifest");
    }
}
