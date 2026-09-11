//! Language-pack variants that are patches, not games.
//!
//! A share of GLP/SLP/PLP archives hold only localized config and launcher
//! files and expect eXo's installer to have laid the English game down
//! first (Alien Odyssey DE: `GAME.CFG` + `run.bat`, whose `call game` lives
//! in the English tree). Such a variant is installed as English-plus-patch,
//! and the English row is installed alongside it as a real dependency.
//!
//! Detection reads the bundled `.torrent` files rather than
//! `games.download_size`: that column is archive PLUS the shared English
//! GameData (§6), so for a 1 KB overlay beside a 17 MB extras archive no
//! threshold on it can work.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::commands::collections::{collection_def, collection_rel_game_dir};
use crate::commands::paths::bundled_torrent_path;
use crate::models::Game;
use crate::torrent::TorrentIndex;

/// An archive this small next to its English counterpart carries no game.
/// Measured across the three packs: overlays sit near 0.1%, the smallest
/// genuine standalone translation near 30%.
const OVERLAY_RATIO: f64 = 0.05;

/// Archive sizes of one collection, indexed like the torrent's file list.
type Sizes = Option<&'static Vec<u64>>;

/// Parsed once per collection; the bundled torrents never change at runtime.
fn sizes_for(collection: &str) -> Sizes {
    static CACHE: OnceLock<Mutex<HashMap<String, Sizes>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().ok()?;
    if let Some(hit) = map.get(collection) {
        return *hit;
    }
    let parsed = load_sizes(collection).map(|v| &*Box::leak(Box::new(v)));
    map.insert(collection.to_string(), parsed);
    parsed
}

fn load_sizes(collection: &str) -> Option<Vec<u64>> {
    let def = collection_def(collection)?;
    let path = bundled_torrent_path(def.torrent_file).ok()?;
    let index = TorrentIndex::from_file(&path).ok()?;
    Some(index.files.iter().map(|f| f.size).collect())
}

fn archive_size(collection: &str, index: i64) -> Option<u64> {
    let idx = usize::try_from(index).ok()?;
    sizes_for(collection)?.get(idx).copied()
}

/// The English row this variant needs underneath it, or None when the
/// variant stands on its own. Only language packs can be overlays - they
/// are the collections that share eXoDOS' directory tree.
pub fn base_for(conn: &rusqlite::Connection, game: &Game) -> Option<Game> {
    if game.language == "EN" {
        return None;
    }
    let source = game.torrent_source.as_deref()?;
    collection_def(source).filter(|c| c.lang_dir.is_some())?;
    let shortcode = game.shortcode.as_deref()?;
    let lp_size = archive_size(source, game.game_torrent_index?)?;

    let base = crate::db::queries::fetch_game_variants(conn, shortcode, source)
        .ok()?
        .into_iter()
        .find(|g| g.language == "EN")?;
    let base_size = archive_size(base.torrent_source.as_deref()?, base.game_torrent_index?)?;
    if base_size == 0 || (lp_size as f64) >= (base_size as f64) * OVERLAY_RATIO {
        return None;
    }
    Some(base)
}

/// The variants that sit on this English row, `installed` ones or the ones
/// still waiting to be. The relationship is derived, never stored: up to
/// three translations share one base, so a single column on the base could
/// not hold it anyway.
pub fn dependents_of(
    conn: &rusqlite::Connection,
    base: &Game,
    installed_only: bool,
) -> Vec<Game> {
    if base.language != "EN" {
        return Vec::new();
    }
    let (Some(base_id), Some(shortcode), Some(source)) = (
        base.id,
        base.shortcode.as_deref(),
        base.torrent_source.as_deref(),
    ) else {
        return Vec::new();
    };
    let Ok(variants) = crate::db::queries::fetch_game_variants(conn, shortcode, source) else {
        return Vec::new();
    };
    variants
        .into_iter()
        .filter(|g| g.id != Some(base_id))
        .filter(|g| if installed_only { g.installed } else { g.in_library && !g.installed })
        .filter(|g| base_for(conn, g).is_some_and(|b| b.id == Some(base_id)))
        .collect()
}

/// `base_for`'s size alone, for the panel and the disk preflight.
pub fn base_archive_size(conn: &rusqlite::Connection, game: &Game) -> Option<u64> {
    let base = base_for(conn, game)?;
    archive_size(base.torrent_source.as_deref()?, base.game_torrent_index?)
}

/// The English game directory for a base row, if it has been extracted.
pub fn base_game_dir(torrent_root: &Path, base: &Game) -> Option<std::path::PathBuf> {
    let rel = collection_rel_game_dir(
        base.torrent_source.as_deref()?,
        base.shortcode.as_deref()?,
        base.application_path.as_deref(),
    );
    let dir = torrent_root.join(rel);
    dir.is_dir().then_some(dir)
}

/// Copy the English tree into the overlay variant's own directory, sharing
/// blocks with the original where the filesystem can (APFS, Btrfs, XFS).
/// The copy has to be real and writable: the game writes its saves there,
/// and a link farm would either lose them or corrupt the English install.
pub fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // clonefile takes the whole tree in one call and refuses an existing
        // destination, which is why it runs before the overlay is unpacked.
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        if !dst.exists() {
            let (s, d) = (
                CString::new(src.as_os_str().as_bytes())?,
                CString::new(dst.as_os_str().as_bytes())?,
            );
            // SAFETY: both paths are NUL-terminated and live across the call.
            if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } == 0 {
                return Ok(());
            }
        }
    }
    copy_tree(src, dst)
}

/// Plain recursive copy, per file reflinked on Linux where the filesystem
/// supports it. Existing files are overwritten - the caller lays the English
/// tree down first and the localized archive over it.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // FICLONE: _IOW(0x94, 9, int). Btrfs and XFS share the blocks, every
    // other filesystem answers EOPNOTSUPP and we fall through to a copy.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let (Ok(input), Ok(output)) = (
        std::fs::File::open(src),
        std::fs::File::create(dst),
    ) else {
        std::fs::copy(src, dst)?;
        return Ok(());
    };
    // SAFETY: both descriptors are open for the duration of the call.
    if unsafe { libc::ioctl(output.as_raw_fd(), FICLONE, input.as_raw_fd()) } == 0 {
        return Ok(());
    }
    drop((input, output));
    std::fs::copy(src, dst)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base with no installed translations has no dependents, and a
    /// translation is never a base for anything.
    #[test]
    fn dependents_are_only_installed_overlays_of_the_same_group() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(tmp.path().join("t.db")).unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        // No torrents are reachable from a temp db, so `base_for` answers
        // None for everything: the guard rails are what this checks.
        let mut base = crate::models::Game {
            id: Some(1),
            language: "EN".into(),
            shortcode: Some("AlienOdy".into()),
            torrent_source: Some("eXoDOS".into()),
            ..sample_game()
        };
        assert!(dependents_of(&conn, &base, true).is_empty());
        base.language = "DE".into();
        assert!(
            dependents_of(&conn, &base, true).is_empty(),
            "a localized row is never a base"
        );
    }

    fn sample_game() -> crate::models::Game {
        crate::models::Game {
            id: None,
            title: "Alien Odyssey".into(),
            sort_title: None,
            platform: "MS-DOS".into(),
            developer: None,
            publisher: None,
            release_date: None,
            year: None,
            genre: None,
            series: None,
            play_mode: None,
            rating: None,
            rating_votes: None,
            description: None,
            notes: None,
            source: None,
            application_path: None,
            dosbox_conf: None,
            status: None,
            region: None,
            max_players: None,
            language: "EN".into(),
            shortcode: None,
            available_languages: None,
            variant_titles: None,
            torrent_source: None,
            in_library: false,
            installed: false,
            favorited: false,
            game_torrent_index: None,
            gamedata_torrent_index: None,
            download_size: None,
            has_thumbnail: false,
            dosbox_variant: None,
            thumbnail_key: None,
            manual_path: None,
            last_played: None,
            music_file: None,
            requires_base: false,
            installed_with: None,
        }
    }

    /// The catalogue's own numbers: Alien Odyssey's German archive is a
    /// patch, its Spanish one a full translation. Guards the ratio against
    /// a future catalogue where both would read the same.
    #[test]
    fn the_ratio_separates_a_patch_from_a_full_translation() {
        let overlay = 1_168f64 / 476_214_170f64;
        let standalone = 474_082_680f64 / 476_214_170f64;
        assert!(overlay < OVERLAY_RATIO, "{overlay}");
        assert!(standalone >= OVERLAY_RATIO, "{standalone}");
    }

    #[test]
    fn copying_onto_an_existing_tree_overwrites_file_by_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("en");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("A"), b"en-a").unwrap();
        std::fs::write(src.join("B"), b"en-b").unwrap();
        let dst = tmp.path().join("de");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("B"), b"de-b").unwrap();
        copy_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("A")).unwrap(), b"en-a");
        assert_eq!(std::fs::read(dst.join("B")).unwrap(), b"en-b");
    }
}
