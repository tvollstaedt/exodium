//! Folding pre-single-root installs into the one game root (§2).

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::db::queries;
use crate::torrent::manager::fastresume_dir;

use super::collections::COLLECTION_MAP;

use super::paths::{game_root, is_os_metadata, load_root_folder};
use super::{DbState, TorrentState};

/// Old per-torrent roots still sitting next to the real one.
///
/// Exodium used to give each pack the folder librqbit names after the torrent
/// (`<data>/eXoWin3x/`, `<data>/eXoWin9x/`), which is not a layout eXo
/// produces. Everything lives in one root now, so an install made before that
/// has to be merged - with the user's consent, since it moves their files.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutMigration {
    /// Folder names still holding games, relative to the data dir.
    pub folders: Vec<String>,
    /// Rough size, so the dialog can say what is about to be moved.
    pub bytes: u64,
    /// Whether to raise the question on startup. False once declined - the
    /// merge itself stays reachable from Settings, which is why the folders
    /// are still reported rather than hidden.
    pub prompt: bool,
}

/// Folders that look like a torrent root of their own (they contain `eXo/`)
/// and are not the configured root.
fn stray_roots(data_dir: &str) -> Vec<PathBuf> {
    let root = game_root(data_dir);
    COLLECTION_MAP
        .iter()
        .map(|c| c.inner_folder)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        // Three places a per-torrent root can sit. Two are inside the data
        // dir: beside the game root, and inside it once the data dir IS the
        // root. The third is BESIDE the data dir - only reachable when the
        // data dir is the root, because that is the legacy shape whose folders
        // were created while the data dir was one level up. Scanning a normal
        // install's parent would mean sweeping the user's home directory.
        .flat_map(|name| {
            let mut candidates = vec![PathBuf::from(data_dir).join(name), root.join(name)];
            if root == Path::new(data_dir) {
                if let Some(parent) = Path::new(data_dir).parent() {
                    candidates.push(parent.join(name));
                }
            }
            candidates
        })
        // `eXo/` inside tells a pack root apart from the root's own
        // `eXo/eXoWin3x/`, which is where those files belong.
        .filter(|p| *p != root && p.join("eXo").is_dir())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Is there anything to merge into the single root?
#[tauri::command]
pub async fn pending_layout_migration(
    db_state: State<'_, DbState>,
) -> Result<Option<LayoutMigration>, String> {
    let (data_dir, asked) = {
        let conn = db_state.lock()?;
        load_root_folder(&conn);
        (
            queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?,
            queries::get_config(&conn, "layout_migration").map_err(|e| e.to_string())?,
        )
    };
    let Some(data_dir) = data_dir else { return Ok(None) };
    let strays = stray_roots(&data_dir);
    if strays.is_empty() {
        return Ok(None);
    }
    if asked.as_deref() == Some("skip") {
        // Worth a log line every start: those folders are not read, so their
        // games look uninstalled and a re-download lands a second copy in the
        // real root while the first keeps occupying the disk.
        log::warn!(
            "Layout: {} folder(s) next to the game root are being ignored ({}). \
             Merge them from Settings to make their games visible again.",
            strays.len(),
            strays
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Apparent size only - walking 282 GB of files to add up bytes would
    // stall startup, and the dialog only needs an order of magnitude.
    let bytes = strays.iter().map(|p| dir_size_shallow(p)).sum();
    Ok(Some(LayoutMigration {
        folders: strays
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect(),
        bytes,
        prompt: asked.as_deref() != Some("skip"),
    }))
}

/// Sum of the game archives one level below `<root>/eXo/<pack>/`.
fn dir_size_shallow(root: &Path) -> u64 {
    fn walk(dir: &Path, depth: usize, acc: &mut u64) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.filter_map(|e| e.ok()) {
            match e.metadata() {
                Ok(m) if m.is_file() => *acc += m.len(),
                Ok(m) if m.is_dir() => walk(&e.path(), depth + 1, acc),
                _ => {}
            }
        }
    }
    let mut total = 0;
    walk(root, 0, &mut total);
    total
}

/// Remember that the user does not want the folders merged.
#[tauri::command]
pub async fn skip_layout_migration(db_state: State<'_, DbState>) -> Result<(), String> {
    let conn = db_state.lock()?;
    queries::set_config(&conn, "layout_migration", "skip").map_err(|e| e.to_string())
}

/// Move every stray root's contents into the single root.
///
/// Renames only: the folders sit on one filesystem, so this is instant and
/// nothing is copied. Anything already present at the destination is left
/// alone rather than overwritten - a half-finished manual merge must not lose
/// data. Torrent state is re-derived afterwards by re-initialising the
/// managers; librqbit re-checks the files in place rather than downloading
/// them again.
#[tauri::command]
pub async fn migrate_layout(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<MergeTally, String> {
    // Stop the torrent engine before touching a single file. librqbit's
    // session opens (and therefore CREATES) every selected file of a torrent,
    // so a live session pointed at the old folder re-materializes the whole
    // tree the instant the merge empties it - measured: 673 files back one
    // second after the move. Dropping the managers releases the last Arc to
    // the session; the frontend calls init_download_manager afterwards, which
    // rebuilds it against the single root.
    torrent_state.0.write().await.clear();

    let data_dir = {
        let conn = db_state.lock()?;
        load_root_folder(&conn);
        crate::commands::games::configured_data_dir(&conn)?
    };
    let root = game_root(&data_dir);
    let mut tally = MergeTally::default();
    let strays = stray_roots(&data_dir);
    log::info!(
        "Layout migration: merging {:?} into {}",
        strays.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        root.display()
    );
    for stray in strays {
        tally.add(merge_tree(&stray, &root)?);
        // Everything is either moved or a deleted duplicate by now, so what
        // remains is empty directories - clear them out so the folder is gone
        // and the prompt has nothing left to find. A folder that still holds
        // a file (something we did not put there) is kept, deliberately.
        remove_empty_tree(&stray);
        if stray.exists() {
            log::warn!(
                "Layout migration: {} kept - it still holds files Exodium did not place there",
                stray.display()
            );
        }
    }
    // Every fastresume bitfield now describes a tree that no longer exists at
    // the path it was recorded for. Leaving them would have librqbit skip its
    // check and read the freshly moved-in files as missing pieces.
    if let Ok(config_dir) = app.path().app_data_dir() {
        let persistence = fastresume_dir(&config_dir);
        if let Ok(entries) = std::fs::read_dir(&persistence) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "bitv") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    {
        let conn = db_state.lock()?;
        queries::set_config(&conn, "layout_migration", "done").map_err(|e| e.to_string())?;
    }
    log::info!(
        "Layout migration: {} moved, {} duplicates removed, {} left alone",
        tally.moved,
        tally.deduped,
        tally.skipped
    );
    Ok(tally)
}

/// Move `src`'s children into `dst`, descending where both sides have the
/// same directory so an existing `eXo/` or `Content/` is merged rather than
/// replaced.
///
/// When the same file exists on both sides, **the larger one wins** and the
/// other is deleted. That is not arbitrary: the loser is almost always a
/// zero-byte torrent placeholder (librqbit allocates one per file of the
/// collection) or a half-finished download, and the winner the real archive.
/// Leaving both behind was the first attempt - it meant the old folders never
/// disappeared, so the migration prompt came back at every start with nothing
/// left to do.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct MergeTally {
    /// Files that only existed on the old side and were renamed across.
    pub moved: usize,
    /// Duplicates resolved - the smaller copy (usually a zero-byte torrent
    /// placeholder) was deleted.
    pub deduped: usize,
    /// Entries neither side could resolve: a directory facing a file, or the
    /// reverse. Counted rather than ignored, because "nothing happened and
    /// nothing was reported" is exactly how the first version of this hid a
    /// folder that never emptied.
    pub skipped: usize,
}

impl MergeTally {
    fn add(&mut self, other: MergeTally) {
        self.moved += other.moved;
        self.deduped += other.deduped;
        self.skipped += other.skipped;
    }
}

fn merge_tree(src: &Path, dst: &Path) -> Result<MergeTally, String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let mut tally = MergeTally::default();
    let entries = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for entry in entries.filter_map(|e| e.ok()) {
        let from = entry.path();
        // Not worth carrying across, and counting it would inflate the report
        // the user reads. remove_empty_tree deletes what is left.
        if is_os_metadata(&from) {
            continue;
        }
        let to = dst.join(entry.file_name());
        if !to.exists() {
            std::fs::rename(&from, &to)
                .map_err(|e| format!("moving {} to {}: {e}", from.display(), to.display()))?;
            tally.moved += 1;
            continue;
        }
        if from.is_dir() && to.is_dir() {
            tally.add(merge_tree(&from, &to)?);
            let _ = std::fs::remove_dir(&from);
            continue;
        }
        let src_len = from.metadata().map(|m| m.len()).unwrap_or(0);
        let dst_len = to.metadata().map(|m| m.len()).unwrap_or(0);
        if from.is_file() && to.is_file() && src_len > dst_len {
            std::fs::remove_file(&to).map_err(|e| e.to_string())?;
            std::fs::rename(&from, &to)
                .map_err(|e| format!("replacing {}: {e}", to.display()))?;
            tally.moved += 1;
        } else if from.is_file() {
            std::fs::remove_file(&from)
                .map_err(|e| format!("removing duplicate {}: {e}", from.display()))?;
            tally.deduped += 1;
        } else {
            log::warn!(
                "Layout migration: {} and {} are different kinds of entry - left alone",
                from.display(),
                to.display()
            );
            tally.skipped += 1;
        }
    }
    Ok(tally)
}

/// Delete a directory tree that contains only (empty) directories.
fn remove_empty_tree(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            remove_empty_tree(&p);
        } else if is_os_metadata(&p) {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The merge has to END with the old folder gone, and it must resolve
    /// duplicates in favour of the real data: the loser is a zero-byte
    /// torrent placeholder or a half-finished download, never the archive.
    #[test]
    fn merging_keeps_the_larger_copy_and_clears_the_old_folder() {
        let dir = std::env::temp_dir().join(format!("exodium_merge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = dir.join("eXoWin9x/eXo/eXoWin9x/1995");
        let new_root = dir.join("eXoDOS/eXo/eXoWin9x/1995");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new_root).unwrap();
        std::fs::write(old.join("Moved.zip"), b"only in the old tree").unwrap();
        // The old tree holds the real archive, the new one a placeholder.
        std::fs::write(old.join("Real.zip"), b"the actual game archive").unwrap();
        std::fs::write(new_root.join("Real.zip"), b"").unwrap();
        // And the other way round.
        std::fs::write(old.join("Placeholder.zip"), b"").unwrap();
        std::fs::write(new_root.join("Placeholder.zip"), b"already downloaded").unwrap();

        // Finder leaves one of these in every folder it has been shown.
        std::fs::write(dir.join("eXoWin9x/.DS_Store"), b"finder junk").unwrap();

        let stray = dir.join("eXoWin9x");
        let tally = merge_tree(&stray, &dir.join("eXoDOS")).unwrap();
        remove_empty_tree(&stray);

        assert!(new_root.join("Moved.zip").is_file(), "new files move across");
        assert_eq!(
            std::fs::read_to_string(new_root.join("Real.zip")).unwrap(),
            "the actual game archive",
            "a placeholder must not win over real data"
        );
        assert_eq!(
            std::fs::read_to_string(new_root.join("Placeholder.zip")).unwrap(),
            "already downloaded",
            "and the existing archive is kept when the old side is empty"
        );
        assert_eq!(tally.moved, 2, "Moved.zip and the larger Real.zip");
        assert_eq!(tally.deduped, 1, "the zero-byte Placeholder.zip is dropped");
        assert_eq!(tally.skipped, 0);
        assert!(!stray.exists(), "the old folder must be gone, or we ask again forever");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
