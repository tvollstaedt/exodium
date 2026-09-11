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

/// A patch is a tiny fraction of its English counterpart AND small in
/// absolute terms. The ratio alone is not enough: the English row is often
/// the CD release while the translation is the complete FLOPPY release, and
/// 57 such rows - German King's Quest VI at 24.9 MB against a 710 MB English
/// CD - read as 3% and are whole games. Anything that could hold a game has
/// to be treated as one, because the cost of guessing wrong is an unwanted
/// multi-hundred-megabyte download plus a mixed-up install.
const OVERLAY_RATIO: f64 = 0.05;
/// No DOS game fits in this, and every measured patch is far below it
/// (Alien Odyssey German: 1,168 bytes). 142 of the 225 rows the ratio alone
/// accepted survive it; the rest are handled as ordinary games.
const OVERLAY_MAX_BYTES: u64 = 64 * 1024;

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
    if lp_size > OVERLAY_MAX_BYTES
        || base_size == 0
        || (lp_size as f64) >= (base_size as f64) * OVERLAY_RATIO
    {
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

    /// Both halves of the rule, against measured catalogue rows. The ratio
    /// alone accepts a complete floppy translation whose English sibling is
    /// the CD release; the absolute cap is what rejects it.
    #[test]
    fn a_patch_needs_both_a_tiny_ratio_and_a_tiny_size() {
        let is_overlay = |lp: u64, base: u64| {
            lp <= OVERLAY_MAX_BYTES && (lp as f64) < (base as f64) * OVERLAY_RATIO
        };
        // Alien Odyssey: German patch, Spanish full translation.
        assert!(is_overlay(1_168, 476_214_170));
        assert!(!is_overlay(474_082_680, 476_214_170));
        // King's Quest VI German floppy (24.9 MB) against the English CD
        // (710 MB): 3.5% of it, and a whole game.
        assert!(!is_overlay(24_900_000, 710_000_000));
        // Death Knights of Krynn German, the smallest of the false ones.
        assert!(!is_overlay(1_350_000, 48_000_000));
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
