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

/// eXo's own answer: `util/<lang>/multilanguage.txt` from the pack's
/// metadata archive, bundled as `metadata/multilanguage_<lang>.txt`. Its
/// entries are launcher-bat names without the extension, which is exactly
/// the `%GameName%` eXo's installer looks up before it unpacks the English
/// archive into the language folder. Empty when a pack ships no list.
fn exo_list(lang_dir: &str) -> &'static [String] {
    static CACHE: OnceLock<Mutex<HashMap<String, &'static Vec<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut map) = cache.lock() else { return &[] };
    let lang = lang_dir.trim_start_matches('!').to_string();
    if let Some(hit) = map.get(&lang) {
        return hit.as_slice();
    }
    let entries = crate::commands::paths::bundled_metadata_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join(format!("multilanguage_{lang}.txt"))).ok())
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if entries.is_empty() {
        log::warn!("lp_overlay: no bundled multilanguage list for '{lang}' - falling back to size");
    }
    let leaked: &'static Vec<String> = Box::leak(Box::new(entries));
    map.insert(lang, leaked);
    leaked.as_slice()
}

/// The launcher bat's name without the extension - eXo's `%GameName%`.
fn bat_stem(application_path: Option<&str>) -> Option<String> {
    let leaf = application_path?.replace('/', "\\");
    let leaf = leaf.rsplit('\\').next()?;
    leaf.strip_suffix(".bat")
        .or_else(|| leaf.strip_suffix(".BAT"))
        .map(str::to_ascii_lowercase)
}

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
    let lang_dir = collection_def(source).and_then(|c| c.lang_dir)?;
    let shortcode = game.shortcode.as_deref()?;

    let base = crate::db::queries::fetch_game_variants(conn, shortcode, source)
        .ok()?
        .into_iter()
        .find(|g| g.language == "EN")?;

    // eXo's list decides where it exists. It is the same lookup eXo's own
    // installer does, so a variant is treated exactly as the pack intends.
    let list = exo_list(lang_dir);
    if !list.is_empty() {
        match bat_stem(game.application_path.as_deref()) {
            Some(stem) => return list.contains(&stem).then_some(base),
            // No launcher path to look up. Rather than silently call it a
            // standalone game, fall through to the size rule below.
            None => log::info!(
                "lp_overlay: '{}' has no launcher path - falling back to size",
                game.title
            ),
        }
    }

    // No list for this pack: fall back to size. Measured against eXo's own
    // answer this misses cases but invents none, which is the safe direction
    // - a missed patch fails at launch and can be re-fetched, an invented one
    // pulls a multi-hundred-megabyte download nobody asked for.
    let lp_size = archive_size(source, game.game_torrent_index?)?;
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

/// One row's own archive size, from the bundled torrent.
pub fn archive_size_of(game: &Game) -> Option<u64> {
    archive_size(game.torrent_source.as_deref()?, game.game_torrent_index?)
}

/// The English tree is ready to be copied FROM: its row is marked installed
/// AND the directory is there. The directory alone is not enough - it exists
/// from the first file the extractor writes, and copying then yields a
/// truncated tree (measured: a 558 MB CD image copied at 43 MB, the game
/// booted to a black screen).
pub fn base_ready(torrent_root: &Path, base: &Game) -> bool {
    base.installed && base_game_dir(torrent_root, base).is_some()
}

/// The English game directory for a base row, if it is on disk. Says nothing
/// about whether the extraction has finished - use `base_ready` for that.
pub fn base_game_dir(torrent_root: &Path, base: &Game) -> Option<std::path::PathBuf> {
    let rel = collection_rel_game_dir(
        base.torrent_source.as_deref()?,
        base.shortcode.as_deref()?,
        base.application_path.as_deref(),
    );
    let dir = torrent_root.join(rel);
    dir.is_dir().then_some(dir)
}

/// Bytes a directory tree occupies, for the reset preflight. Cheap enough:
/// a game directory is thousands of files, not millions.
pub fn dir_size(root: &Path) -> u64 {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
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

    fn sample_game() -> Game {
        Game {
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

    /// A localized row is never a base, and a base with no installed
    /// translations has no dependents.
    #[test]
    fn dependents_are_only_installed_overlays_of_the_same_group() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(tmp.path().join("t.db")).unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        let mut row = Game {
            id: Some(1),
            shortcode: Some("AlienOdy".into()),
            torrent_source: Some("eXoDOS".into()),
            ..sample_game()
        };
        assert!(dependents_of(&conn, &row, true).is_empty());
        row.language = "DE".into();
        assert!(dependents_of(&conn, &row, true).is_empty());
    }

    /// The directory alone is not proof: the extractor creates it with its
    /// first file. Copying from it then yields a truncated tree.
    #[test]
    fn a_base_is_only_ready_once_its_row_says_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut base = Game {
            id: Some(1),
            language: "EN".into(),
            shortcode: Some("AlienOdy".into()),
            torrent_source: Some("eXoDOS".into()),
            application_path: Some("eXo\\eXoDOS\\!dos\\AlienOdy\\x.bat".into()),
            installed: false,
            ..sample_game()
        };
        std::fs::create_dir_all(tmp.path().join("eXo/eXoDOS/AlienOdy")).unwrap();
        assert!(base_game_dir(tmp.path(), &base).is_some(), "directory is there");
        assert!(!base_ready(tmp.path(), &base), "but the extraction is not done");
        base.installed = true;
        assert!(base_ready(tmp.path(), &base));
    }

    /// A row whose launcher path is missing must not be silently declared a
    /// standalone game - the size rule still gets its turn.
    #[test]
    fn a_row_without_a_launcher_path_falls_through_to_size() {
        assert_eq!(bat_stem(None), None);
        // The three GLP rows in this state are not in eXo's list either way,
        // so the fallback is what decides them.
        let german = exo_list("!german");
        if !german.is_empty() {
            assert!(!german.contains(&"cybermage: darklight awakening (1995)".to_string()));
        }
    }

    /// eXo's list is keyed on the launcher bat's name, which is what the
    /// row's `application_path` ends in.
    #[test]
    fn the_bat_stem_is_exos_game_name() {
        assert_eq!(
            bat_stem(Some("eXo\\eXoDOS\\!dos\\!german\\AlienOdy\\Alien Odyssey (1995).bat")),
            Some("alien odyssey (1995)".to_string())
        );
        // Forward slashes and upper case survive; anything else is not a bat.
        assert_eq!(
            bat_stem(Some("eXo/eXoDOS/!spanish/Alien Odyssey (1995).BAT")),
            Some("alien odyssey (1995)".to_string())
        );
        assert_eq!(bat_stem(Some("eXo/eXoDOS/!dos/SQ5/dosbox.conf")), None);
        assert_eq!(bat_stem(None), None);
    }

    /// The bundled lists must stay readable and keyed the way eXo writes
    /// them: one bat name per line, comments with '#'.
    #[test]
    fn the_bundled_lists_name_alien_odyssey() {
        let german = exo_list("!german");
        if german.is_empty() {
            return; // not running from the repo
        }
        assert!(german.iter().any(|n| n == "alien odyssey (1995)"));
        // The measured false positives of the old size rule are NOT in it.
        assert!(!german.iter().any(|n| n.starts_with("king's quest vi")));
        assert!(!exo_list("!spanish").is_empty());
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
