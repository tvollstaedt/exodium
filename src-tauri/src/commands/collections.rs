//! The collection table and the path shapes it implies (§1, §15, §16, §18).

use serde::Serialize;


/// Metadata describing a single eXo collection.
/// All path conventions for a collection are captured here so that game
/// launch / install / uninstall code does not need to hard-code any
/// collection-specific strings.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionDef {
    /// Internal collection ID (e.g. "eXoDOS", "eXoDOS_GLP").
    pub id: &'static str,
    /// Human-readable name shown in the UI.
    pub display_name: &'static str,
    /// Bundled metadata XML gz file (e.g. "MS-DOS.xml.gz").
    pub metadata_file: &'static str,
    /// Bundled .torrent filename (e.g. "eXoDOS.torrent").
    pub torrent_file: &'static str,
    /// Optional bundled DOSBox/emulator configs ZIP.
    pub configs_zip: Option<&'static str>,
    /// The folder name the torrent creates inside the data dir (always "eXoDOS").
    /// All four collections (eXoDOS, GLP, PLP, SLP) share the same output folder via
    /// the overlay model - their torrents all have the internal name "eXoDOS" and write
    /// to <data_dir>/eXoDOS/ without any per-collection subdirectory.
    pub inner_folder: &'static str,
    /// Path from <inner_folder> to the individual game directories.
    /// e.g. "eXo/eXoDOS" → games are at <inner_folder>/eXo/eXoDOS/<shortcode>/
    pub game_prefix: &'static str,
    /// Segment in the LaunchBox application_path used to extract the shortcode.
    /// e.g. "!dos" for eXoDOS (path looks like "eXo\eXoDOS\!dos\<shortcode>\…")
    pub shortcode_segment: &'static str,
    /// Language subdirectory inside game_prefix for LP variant games.
    /// None for the base English collection.
    pub lang_dir: Option<&'static str>,
    /// LaunchBox platform name. Names the media subtree in the metadata pack
    /// (`Images/<platform>/`, `Manuals/<platform>/`) and matches the XML's
    /// `<Platform>` value.
    pub platform: &'static str,
    /// Games live under a 4-digit year subdirectory and are keyed by their
    /// title directory instead of an 8-char shortcode:
    /// `<game_prefix>/<year>/<Title (Year)>/` (eXoWin9x layout). All path
    /// derivation keys off this flag, never off the collection id.
    pub year_subdirs: bool,
    /// Which launch pipeline runs the collection's games. Path derivation
    /// never keys on this (that is `year_subdirs` and `lang_dir`); it is the
    /// one switch `launch_game`/`download_game` dispatch on, so a collection
    /// with its own emulator is added by naming it here, not by string
    /// comparisons on the id.
    pub launcher: Launcher,
}

/// The emulator pipeline a collection's games go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Launcher {
    /// DOSBox Staging (or eXo's ECE build on Windows) driven by a patched
    /// per-game dosbox.conf - eXoDOS, its language packs and eXoWin3x.
    DosBox,
    /// DOSBox-X / 86Box booting a Windows 9x VHD - eXoWin9x.
    Win9x,
    /// ScummVM launched with a game id, no conf at all - eXoScummVM.
    ScummVm,
}

/// Look up a collection definition by ID.  Returns None for unknown IDs.
pub fn collection_def(id: &str) -> Option<&'static CollectionDef> {
    COLLECTION_MAP.iter().find(|c| c.id == id)
}

/// The collection to consult for assets `collection` has none of its own.
///
/// Language packs borrow their base collection's art and manuals - their
/// variants hash to the EN title's key. A collection with its own game tree
/// borrows nothing: its games are not in the other pack, so a same-title hit
/// would show a different game's cover. `None` means "no fallback".
pub fn asset_fallback(collection: &str) -> Option<&'static str> {
    let base = collection_base_id(collection);
    (base != collection).then_some(base)
}

/// The base (non-language-pack) collection a source belongs to.
///
/// Language packs share the base collection's game tree and its GameData
/// archives, so "eXoDOS_GLP" resolves to "eXoDOS". A collection with its own
/// game tree resolves to itself. Used wherever a lookup may only cross
/// collection boundaries WITHIN one pack family - shortcodes are unique per
/// family, not globally, so an unqualified match can hit a different game in
/// another pack that happens to share the code.
pub fn collection_base_id(source: &str) -> &'static str {
    let Some(def) = collection_def(source) else {
        return "eXoDOS";
    };
    if def.lang_dir.is_none() {
        return def.id;
    }
    COLLECTION_MAP
        .iter()
        .find(|c| c.lang_dir.is_none() && c.game_prefix == def.game_prefix)
        .map(|c| c.id)
        .unwrap_or("eXoDOS")
}

/// All known eXo collections.
/// Language packs are listed BEFORE eXoDOS so their games are matched to the
/// correct torrent before eXoDOS can claim same-title translations.
/// To add a new collection, append a CollectionDef entry here - no other
/// Rust file needs to be changed for path/emulator dispatch.
pub const COLLECTION_MAP: &[CollectionDef] = &[
    CollectionDef {
        id: "eXoDOS_GLP",
        display_name: "German Language Pack",
        metadata_file: "GLP.xml.gz",
        torrent_file: "eXoDOS_GLP.torrent",
        configs_zip: Some("GLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!german"),
        platform: "MS-DOS",
        year_subdirs: false,
        launcher: Launcher::DosBox,
    },
    CollectionDef {
        id: "eXoDOS_PLP",
        display_name: "Polish Language Pack",
        metadata_file: "PLP.xml.gz",
        torrent_file: "eXoDOS_PLP.torrent",
        configs_zip: Some("PLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!polish"),
        platform: "MS-DOS",
        year_subdirs: false,
        launcher: Launcher::DosBox,
    },
    CollectionDef {
        id: "eXoDOS_SLP",
        display_name: "Spanish Language Pack",
        metadata_file: "SLP.xml.gz",
        torrent_file: "eXoDOS_SLP.torrent",
        configs_zip: Some("SLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!spanish"),
        platform: "MS-DOS",
        year_subdirs: false,
        launcher: Launcher::DosBox,
    },
    CollectionDef {
        id: "eXoDOS",
        display_name: "eXoDOS",
        metadata_file: "MS-DOS.xml.gz",
        torrent_file: "eXoDOS.torrent",
        configs_zip: Some("eXoDOS_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: None,
        platform: "MS-DOS",
        year_subdirs: false,
        launcher: Launcher::DosBox,
    },
    // First collection with an inner_folder of its own: the eXoWin3x torrent
    // carries the internal name "eXoWin3x", so it cannot collide with the four
    // eXoDOS torrents and writes to <data_dir>/eXoWin3x/ instead.
    CollectionDef {
        id: "eXoWin3x",
        display_name: "eXoWin3x",
        metadata_file: "Win3x.xml.gz",
        torrent_file: "eXoWin3x.torrent",
        configs_zip: Some("Win3x_configs.zip"),
        inner_folder: "eXoWin3x",
        game_prefix: "eXo/eXoWin3x",
        shortcode_segment: "!win3x",
        lang_dir: None,
        platform: "Windows 3x",
        year_subdirs: false,
        launcher: Launcher::DosBox,
    },
    // eXoWin9x nests its games one level deeper than every other pack
    // (`eXo/eXoWin9x/<year>/<Title (Year)>.zip`) and has no 8-char shortcodes:
    // the title directory doubles as the shortcode. Games boot Windows 95/98
    // inside DOSBox-X (or 86Box) from VHD images - Staging cannot run them.
    CollectionDef {
        id: "eXoWin9x",
        display_name: "eXoWin9x",
        metadata_file: "Win9x.xml.gz",
        torrent_file: "eXoWin9x.torrent",
        configs_zip: Some("Win9x_configs.zip"),
        inner_folder: "eXoWin9x",
        game_prefix: "eXo/eXoWin9x",
        shortcode_segment: "!win9x",
        lang_dir: None,
        platform: "Windows 9x",
        year_subdirs: true,
        launcher: Launcher::Win9x,
    },
    // eXoScummVM: flat title-named zips like eXoWin3x, but no shortcodes -
    // the zip stem (`Maniac Mansion (Multi-Platform)`) is the title directory
    // AND the key into eXo's launch index (metadata/scummvm.txt). No configs
    // zip: there is no per-game conf, the whole launch is one command line.
    CollectionDef {
        id: "eXoScummVM",
        display_name: "eXoScummVM",
        metadata_file: "ScummVM.xml.gz",
        torrent_file: "eXoScummVM.torrent",
        configs_zip: None,
        inner_folder: "eXoScummVM",
        game_prefix: "eXo/eXoScummVM",
        shortcode_segment: "!ScummVM",
        lang_dir: None,
        platform: "ScummVM",
        year_subdirs: false,
        launcher: Launcher::ScummVm,
    },
];


/// Get the game directory prefix for a collection (path from inner_folder to game dirs).
pub(crate) fn collection_game_prefix(source: &str) -> &'static str {
    crate::commands::collections::collection_def(source)
        .map(|c| c.game_prefix)
        .unwrap_or("eXo/eXoDOS")
}

/// Get the language subdirectory for an LP collection, if any.
pub(crate) fn collection_lang_dir(source: &str) -> Option<&'static str> {
    crate::commands::collections::collection_def(source).and_then(|c| c.lang_dir)
}

/// The year directory a `year_subdirs` collection nests its games under,
/// read from the application_path (`eXo\eXoWin9x\!win9x\<year>\<TitleDir>\…`).
/// None for every other collection - and for a malformed path, in which case
/// callers fall back to the flat `<game_prefix>/<shortcode>` layout.
pub(crate) fn collection_year_dir(source: &str, app_path: Option<&str>) -> Option<String> {
    let def = crate::commands::collections::collection_def(source)?;
    if !def.year_subdirs {
        return None;
    }
    let normalized = app_path?.replace('\\', "/");
    let needle = format!("/{}/", def.shortcode_segment);
    let idx = normalized.find(&needle)?;
    let year = normalized[idx + needle.len()..].split('/').next()?;
    (year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit())).then(|| year.to_string())
}

/// Torrent-relative directory holding a game's installed files.
/// Standard: <game_prefix>[/<lang_dir>]/<shortcode>
/// year_subdirs (eXoWin9x): <game_prefix>/<year>/<shortcode> - the shortcode
/// IS the title directory there ("Connect4 (1995)").
pub(crate) fn collection_rel_game_dir(source: &str, shortcode: &str, app_path: Option<&str>) -> String {
    let prefix = collection_game_prefix(source);
    if let Some(year) = collection_year_dir(source, app_path) {
        return format!("{}/{}/{}", prefix, year, shortcode);
    }
    match collection_lang_dir(source) {
        Some(ld) => format!("{}/{}/{}", prefix, ld, shortcode),
        None => format!("{}/{}", prefix, shortcode),
    }
}

/// Torrent-relative path of a game's ZIP (same year/lang nesting as the dir).
pub(crate) fn collection_rel_zip(source: &str, game_name: &str, app_path: Option<&str>) -> String {
    let prefix = collection_game_prefix(source);
    if let Some(year) = collection_year_dir(source, app_path) {
        return format!("{}/{}/{}.zip", prefix, year, game_name);
    }
    match collection_lang_dir(source) {
        Some(ld) => format!("{}/{}/{}.zip", prefix, ld, game_name),
        None => format!("{}/{}.zip", prefix, game_name),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rel_paths_nest_win9x_games_under_their_year_dir() {
        let app = Some(r"eXo\eXoWin9x\!win9x\1995\Connect4 (1995)\Connect4 (1995).bat");
        assert_eq!(
            super::collection_rel_game_dir("eXoWin9x", "Connect4 (1995)", app),
            "eXo/eXoWin9x/1995/Connect4 (1995)"
        );
        assert_eq!(
            super::collection_rel_zip("eXoWin9x", "Connect4 (1995)", app),
            "eXo/eXoWin9x/1995/Connect4 (1995).zip"
        );
        // A malformed path falls back to the flat layout instead of panicking.
        assert_eq!(
            super::collection_rel_game_dir("eXoWin9x", "Connect4 (1995)", None),
            "eXo/eXoWin9x/Connect4 (1995)"
        );
    }
}
