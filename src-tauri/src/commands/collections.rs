//! The collection table and the path shapes it implies (§1, §15, §16, §18).

use serde::Serialize;


/// One eXo collection: every path convention and the launcher it dispatches
/// to, so no other code keys on the id string.
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
    /// The torrent's internal folder name (`eXoDOS` for all four DOS packs).
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
    /// Games sit under `<game_prefix>/<year>/<Title (Year)>/`, the title dir
    /// being the shortcode (eXoWin9x). Path derivation keys on this flag.
    pub year_subdirs: bool,
    /// The launch pipeline `launch_game` and `download_game` dispatch on.
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

/// Where a collection borrows art and manuals from: language packs use
/// their base collection's, a collection with its own game tree none.
pub fn asset_fallback(collection: &str) -> Option<&'static str> {
    let base = collection_base_id(collection);
    (base != collection).then_some(base)
}

/// The pack family: language packs resolve to their base collection, the
/// rest to themselves. Shortcodes are unique per family only (§1).
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

/// Every collection. Language packs come BEFORE eXoDOS so title matching
/// reaches them first. A new pack is one entry here (§15).
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

/// The year dir of a `year_subdirs` game, read from its application_path;
/// None elsewhere (callers then use the flat layout).
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

/// A game's dir relative to the root: `<prefix>[/<lang>]/<shortcode>`, or
/// `<prefix>/<year>/<shortcode>` for `year_subdirs`.
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

    #[test]
    fn rel_paths_keep_flat_and_lang_layouts_for_other_packs() {
        let app = Some(r"eXo\eXoDOS\!dos\SQ5\Space Quest V.bat");
        assert_eq!(
            super::collection_rel_game_dir("eXoDOS", "SQ5", app),
            "eXo/eXoDOS/SQ5"
        );
        assert_eq!(
            super::collection_rel_game_dir("eXoDOS_GLP", "SQ5", app),
            "eXo/eXoDOS/!german/SQ5"
        );
        assert_eq!(
            super::collection_rel_zip("eXoDOS_GLP", "Space Quest V", app),
            "eXo/eXoDOS/!german/Space Quest V.zip"
        );
    }
}
