//! What an uninstall keeps: the files the user changed or added, decided
//! against the archive the game was unpacked from (§5).

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::install::copy_dir_recursive;

const RETRIES: u32 = 5;

/// `remove_dir_all` with retries: Windows refuses while an indexer or
/// scanner still holds a handle on a freshly written file, and a read-only
/// attribute from a CD image is cleared before the next attempt.
pub(crate) fn remove_tree(path: &Path) -> std::io::Result<()> {
    retry(|| std::fs::remove_dir_all(path), |e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            clear_readonly(path);
        }
    })
}

/// `rename` with the same retry policy; the parent of `to` is created first.
pub(crate) fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    retry(|| std::fs::rename(from, to), |_| {})
}

fn retry(
    mut op: impl FnMut() -> std::io::Result<()>,
    mut on_err: impl FnMut(&std::io::Error),
) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt + 1 < RETRIES => {
                on_err(&e);
                attempt += 1;
                std::thread::sleep(Duration::from_millis(250 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(windows)]
fn clear_readonly(root: &Path) {
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if let Ok(meta) = entry.metadata() {
            let mut perms = meta.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(entry.path(), perms);
            }
        }
    }
}

#[cfg(not(windows))]
fn clear_readonly(_root: &Path) {}

/// Size and CRC-32 of every file an archive unpacks, keyed by the path
/// below the archive's top-level directory (forward slashes).
#[derive(Default, Debug, Clone)]
pub(crate) struct PristineIndex(HashMap<String, (u64, u32)>);

impl PristineIndex {
    /// Reads the central directory only. `None` for a placeholder or a
    /// piece fragment (§16), which callers treat as "no archive".
    pub(crate) fn from_zip(path: &Path) -> Option<Self> {
        let file = std::fs::File::open(path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let mut map = HashMap::with_capacity(archive.len());
        // The game unpacks below ONE top-level directory (its shortcode);
        // only entries under it are keyed, root-level files are not on disk
        // as game files.
        let top: String = (0..archive.len()).find_map(|i| {
            let entry = archive.by_index_raw(i).ok()?;
            entry.name().split_once('/').map(|(first, _)| first.to_string())
        })?;
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i).ok()?;
            if entry.is_dir() {
                continue;
            }
            let Some((first, rel)) = entry.name().split_once('/') else { continue };
            if first != top || rel.is_empty() {
                continue;
            }
            map.insert(rel.to_string(), (entry.size(), entry.crc32()));
        }
        Some(Self(map))
    }

    /// Later archives win: an overlay patch replaces files of its base.
    pub(crate) fn merge(mut self, other: Self) -> Self {
        self.0.extend(other.0);
        self
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The archive's manifest for after the archive itself is gone.
    pub(crate) fn write_manifest(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            for (rel, (size, crc)) in &self.0 {
                writeln!(w, "{crc:08x}\t{size}\t{rel}")?;
            }
            w.flush()?;
        }
        std::fs::rename(&tmp, path)
    }

    pub(crate) fn from_manifest(path: &Path) -> Option<Self> {
        let file = std::fs::File::open(path).ok()?;
        let mut map = HashMap::new();
        for line in std::io::BufReader::new(file).lines() {
            let line = line.ok()?;
            let mut parts = line.splitn(3, '\t');
            let crc = u32::from_str_radix(parts.next()?, 16).ok()?;
            let size = parts.next()?.parse().ok()?;
            map.insert(parts.next()?.to_string(), (size, crc));
        }
        Some(Self(map))
    }

    /// Files under `game_dir` that the archive did not put there in this
    /// form: unknown paths, other sizes, and - only then - other checksums.
    /// A file that cannot be read counts as changed, and a directory that
    /// cannot be walked is an error: an unseen subtree must not be deleted.
    pub(crate) fn changed_files(&self, game_dir: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(game_dir) {
            let entry = entry.map_err(|e| format!("cannot read {}: {e}", game_dir.display()))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(game_dir) else { continue };
            #[cfg(windows)]
            let key = rel.to_string_lossy().replace('\\', "/");
            #[cfg(not(windows))]
            let key = rel.to_string_lossy().into_owned();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let pristine = match self.0.get(&key) {
                Some(&(psize, pcrc)) => psize == size && crc32_of(entry.path()) == Some(pcrc),
                None => false,
            };
            if !pristine {
                out.push((rel.to_path_buf(), size));
            }
        }
        Ok(out)
    }
}

fn crc32_of(path: &Path) -> Option<u32> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            return Some(hasher.finalize());
        }
        hasher.update(&buf[..n]);
    }
}

/// What an uninstall set aside.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SaveBackup {
    pub files: usize,
    pub bytes: u64,
    /// No archive to compare against, so the whole directory was kept.
    pub whole_dir: bool,
}

/// Marks a backup that holds a whole game folder, not just user files, so
/// the bulk "delete save backups" leaves it alone. Sits BESIDE the backup
/// directory - inside it, the restore would copy it into the game.
pub(crate) fn whole_dir_marker(save_dir: &Path) -> PathBuf {
    let mut name = save_dir.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".whole");
    save_dir.with_file_name(name)
}

/// Moves the user's own files from `game_dir` to `save_dir` and removes the
/// rest. Without a pristine index the whole directory is kept. Nothing is
/// deleted until the backup is complete, and a failed delete is an error:
/// a game folder that is still there must never read as uninstalled.
pub(crate) fn back_up_user_data(
    game_dir: &Path,
    save_dir: &Path,
    pristine: Option<&PristineIndex>,
) -> Result<SaveBackup, String> {
    if save_dir.exists() {
        remove_tree(save_dir).map_err(|e| format!("cannot replace {}: {e}", save_dir.display()))?;
    }
    let _ = std::fs::remove_file(whole_dir_marker(save_dir));
    // No index, or a subtree the walk could not read: keep everything.
    let changed = match pristine.filter(|p| !p.is_empty()).map(|p| p.changed_files(game_dir)) {
        Some(Ok(changed)) => Some(changed),
        Some(Err(e)) => {
            log::warn!("{e} - keeping the whole game folder");
            None
        }
        None => None,
    };
    let Some(changed) = changed else {
        let bytes = super::lp_overlay::dir_size(game_dir);
        let files = walkdir::WalkDir::new(game_dir).into_iter().filter_map(Result::ok)
            .filter(|e| e.file_type().is_file()).count();
        if let Err(e) = rename_retry(game_dir, save_dir) {
            log::warn!("Rename to {} failed ({e}), copying instead", save_dir.display());
            copy_dir_recursive(game_dir, save_dir)?;
            remove_tree(game_dir).map_err(|e| format!("cannot remove {}: {e}", game_dir.display()))?;
        }
        let _ = std::fs::write(whole_dir_marker(save_dir), b"");
        return Ok(SaveBackup { files, bytes, whole_dir: true });
    };

    // Same volume, so a rename per file: instant, and no second copy of a
    // multi-gigabyte VHD while both exist.
    let mut bytes = 0u64;
    for (rel, size) in &changed {
        let source = game_dir.join(rel);
        let target = save_dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if std::fs::rename(&source, &target).is_err() {
            std::fs::copy(&source, &target)
                .map_err(|e| format!("cannot back up {}: {e}", rel.display()))?;
        }
        bytes += size;
    }
    remove_tree(game_dir).map_err(|e| format!("cannot remove {}: {e}", game_dir.display()))?;
    Ok(SaveBackup { files: changed.len(), bytes, whole_dir: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    /// Only new and modified files are kept; the untouched rest is removed
    /// together with the game directory.
    #[test]
    fn uninstall_keeps_only_what_the_user_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let zip = root.join("Game (1990).zip");
        write_zip(&zip, &[("SQ5/SQ5.EXE", b"binary"), ("SQ5/DATA/RES.000", b"resource"), ("SQ5/SAVE.CFG", b"defaults")]);
        let game_dir = root.join("SQ5");
        fs::create_dir_all(game_dir.join("DATA")).unwrap();
        fs::write(game_dir.join("SQ5.EXE"), b"binary").unwrap();
        fs::write(game_dir.join("DATA/RES.000"), b"resource").unwrap();
        fs::write(game_dir.join("SAVE.CFG"), b"changed!").unwrap();
        fs::write(game_dir.join("SQ5SG.001"), b"a savegame").unwrap();

        let save_dir = root.join("!save/SQ5");
        let index = PristineIndex::from_zip(&zip).unwrap();
        let backup = back_up_user_data(&game_dir, &save_dir, Some(&index)).unwrap();

        assert!(!game_dir.exists(), "game dir must be gone");
        assert_eq!(backup.files, 2);
        assert!(!backup.whole_dir);
        assert_eq!(fs::read(save_dir.join("SAVE.CFG")).unwrap(), b"changed!");
        assert_eq!(fs::read(save_dir.join("SQ5SG.001")).unwrap(), b"a savegame");
        assert!(!save_dir.join("SQ5.EXE").exists(), "pristine files are not backed up");
        assert!(!save_dir.join("DATA").exists());
    }

    /// The `!save` parent does not exist on the first uninstall in a tree;
    /// the rename must still land (it used to fall back to a full copy).
    #[test]
    fn whole_dir_backup_creates_the_save_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let game_dir = tmp.path().join("deep/eXoDOS/SQ5");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("SQ5.EXE"), b"x").unwrap();
        let save_dir = tmp.path().join("deep/eXoDOS/!save/SQ5");

        let backup = back_up_user_data(&game_dir, &save_dir, None).unwrap();
        assert!(backup.whole_dir);
        assert_eq!(backup.files, 1);
        assert!(save_dir.join("SQ5.EXE").exists());
        assert!(!game_dir.exists());
        assert!(whole_dir_marker(&save_dir).is_file(), "a whole-folder backup is marked");
    }

    /// Root-level entries and a second top-level directory are not the
    /// game's files; only the shortcode directory is keyed.
    #[test]
    fn index_keys_only_the_top_level_game_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("g.zip");
        write_zip(&zip, &[("readme.txt", b"x"), ("SQ5/SQ5.EXE", b"exe"), ("OTHER/x.dat", b"y")]);
        let index = PristineIndex::from_zip(&zip).unwrap();
        assert_eq!(index.0.len(), 1);
        assert!(index.0.contains_key("SQ5.EXE"));
    }

    /// The manifest replaces the archive once that is dropped after install.
    #[test]
    fn manifest_round_trips_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = tmp.path().join("g.zip");
        write_zip(&zip, &[("G/A.EXE", b"aaaa"), ("G/sub dir/b c.dat", b"bb")]);
        let index = PristineIndex::from_zip(&zip).unwrap();
        let manifest = tmp.path().join("content/pristine/eXoDOS_7.idx");
        index.write_manifest(&manifest).unwrap();
        let back = PristineIndex::from_manifest(&manifest).unwrap();
        assert_eq!(back.0, index.0);
    }

    /// A patch archive overrides its base's entries, so an overlay install
    /// with the localized config in place reads as pristine.
    #[test]
    fn overlay_patch_entries_win_over_the_base() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base.zip");
        let patch = tmp.path().join("patch.zip");
        write_zip(&base, &[("AO/GAME.CFG", b"Language 0"), ("AO/GAME.EXE", b"exe")]);
        write_zip(&patch, &[("AO/GAME.CFG", b"Language 4")]);
        let index = PristineIndex::from_zip(&base).unwrap().merge(PristineIndex::from_zip(&patch).unwrap());
        let dir = tmp.path().join("AO");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("GAME.CFG"), b"Language 4").unwrap();
        fs::write(dir.join("GAME.EXE"), b"exe").unwrap();
        assert!(index.changed_files(&dir).unwrap().is_empty());
    }

    #[test]
    fn a_placeholder_is_not_an_index() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty.zip");
        fs::write(&empty, b"").unwrap();
        assert!(PristineIndex::from_zip(&empty).is_none());
    }
}
