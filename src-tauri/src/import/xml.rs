use std::io::BufRead;

use quick_xml::de::from_reader;
use serde::Deserialize;

use super::ImportResult;
use crate::models::Game;

/// Root element of the MS-DOS.xml file.
#[derive(Debug, Deserialize)]
#[serde(rename = "LaunchBox")]
struct LaunchBoxGames {
    #[serde(rename = "Game", default)]
    games: Vec<XmlGame>,
}

/// Raw XML representation of a LaunchBox <Game> element.
/// Only the fields we care about - everything else is silently ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XmlGame {
    #[serde(default)]
    title: String,
    #[serde(default)]
    sort_title: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    developer: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    series: Option<String>,
    #[serde(default)]
    play_mode: Option<String>,
    #[serde(default)]
    community_star_rating: Option<String>,
    #[serde(default)]
    community_star_rating_total_votes: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    application_path: Option<String>,
    #[serde(default)]
    root_folder: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    max_players: Option<String>,
    #[serde(default)]
    manual_path: Option<String>,
    #[serde(default)]
    music_path: Option<String>,
    #[serde(default)]
    missing_music: Option<String>,
}

fn blank_to_none(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

/// Name of the theme track LaunchBox expects in the game's GameData archive.
///
/// `MusicPath` is only written when the track is not the default
/// `Music\MS-DOS\<Title>.mp3` (175 of 7,667 eXoDOS rows, mostly .ogg and
/// tracker modules); for the rest `MissingMusic=false` is the only signal.
/// That flag is LaunchBox's DEFAULT, not an inventory: the eXoWin3x catalogue
/// says "false" for 1,120 games and ships no music at all, so a caller must
/// treat the result as a hint and let the archive have the last word.
fn derive_music_file(title: &str, music_path: Option<&str>, missing_music: Option<&str>) -> Option<String> {
    if let Some(path) = music_path.filter(|p| !p.trim().is_empty()) {
        let name = path.rsplit(['\\', '/']).next().unwrap_or(path).trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    if missing_music.is_some_and(|m| m.trim().eq_ignore_ascii_case("false")) && !title.is_empty() {
        return Some(format!("{}.mp3", title));
    }
    None
}

/// Extract shortcode from application_path using a collection-specific path segment.
///
/// eXoDOS:   "eXo\eXoDOS\!dos\captlsm\Capitalism (1995).bat"   → segment "!dos" → "captlsm"
/// eXoDOS:   "eXo\eXoDOS\!dos\!german\SQ5\Space Quest V.bat"   → segment "!dos" → "SQ5"
/// eXoWin3x: "eXo\eXoWin3X\!win3x\101Dalma\101 Dalmatians (1997).bat" → segment "!win3x" → "101Dalma"
/// eXoWin9x: "eXo\eXoWin9x\!win9x\1995\Connect4 (1995)\Connect4 (1995).bat"
///           → segment "!win9x" → "Connect4 (1995)" (the pack has no 8-char
///           shortcodes; a 4-digit YEAR directory is skipped and the title
///           directory doubles as the shortcode)
fn extract_shortcode(app_path: &Option<String>, segment: &str) -> Option<String> {
    let path = app_path.as_ref()?;
    let normalized = path.replace('\\', "/");
    let needle = format!("/{}/", segment);
    let seg_idx = normalized.find(&needle)?;
    let after_seg = &normalized[seg_idx + needle.len()..];
    // Skip language dir if present (e.g., "!german/")
    let after_lang = if after_seg.starts_with('!') {
        after_seg.find('/')?.checked_add(1).and_then(|i| after_seg.get(i..))?
    } else {
        after_seg
    };
    // Take the shortcode (next path segment)
    let end = after_lang.find('/')?;
    let code = &after_lang[..end];
    // A 4-digit segment FOLLOWED BY another directory is a year folder
    // (eXoWin9x nests `!win9x/<year>/<Title (Year)>/<bat>`); a bare 4-digit
    // shortcode like eXoDOS "1939" sits directly before the bat and stays.
    let rest = &after_lang[end + 1..];
    if code.len() == 4 && code.bytes().all(|b| b.is_ascii_digit()) {
        if let Some(next_end) = rest.find('/') {
            return Some(rest[..next_end].to_string());
        }
    }
    Some(code.to_string())
}

fn extract_year(date_str: &Option<String>) -> Option<i32> {
    date_str.as_ref().and_then(|s| s.get(..4)?.parse().ok())
}

/// Extract language code from the Series field.
/// e.g. "Language: DE" → "DE", "Playlist: Roland MT-32; Language: FR" → "FR"
///
/// The source data has no standard: the language packs write ISO-style codes,
/// the main eXoDOS catalog spells the language out ("Language: Japanese").
/// Normalize the spelled-out names so the badges read uniformly - unknown
/// values pass through verbatim rather than being guessed at.
fn extract_language(series: &Option<String>) -> String {
    if let Some(s) = series {
        for part in s.split(';') {
            let trimmed = part.trim();
            if let Some(lang) = trimmed.strip_prefix("Language:") {
                let code = lang.trim().to_uppercase();
                if !code.is_empty() {
                    return normalize_language(&code);
                }
            }
        }
    }
    "EN".to_string()
}

fn normalize_language(code: &str) -> String {
    match code {
        "ENGLISH" => "EN",
        "GERMAN" => "DE",
        "SPANISH" => "ES",
        "POLISH" => "PL",
        "FRENCH" => "FR",
        "ITALIAN" => "IT",
        "DUTCH" => "NL",
        "FINNISH" => "FI",
        "JAPANESE" => "JA",
        "CHINESE" => "ZH",
        other => other,
    }
    .to_string()
}

/// Convert a raw XML game record to our Game model.
/// `shortcode_segment` is the collection-specific path segment used to extract
/// the shortcode from application_path (e.g. "!dos" for eXoDOS, "!win3x" for eXoWin3x).
fn xml_game_to_game(x: XmlGame, shortcode_segment: &str) -> Game {
    let year = extract_year(&x.release_date);
    let language = extract_language(&x.series);
    let shortcode = extract_shortcode(&x.application_path, shortcode_segment)
        .or_else(|| extract_shortcode(&x.root_folder, shortcode_segment));
    let music_file = derive_music_file(&x.title, x.music_path.as_deref(), x.missing_music.as_deref());
    Game {
        id: None,
        title: x.title,
        sort_title: blank_to_none(x.sort_title),
        platform: x.platform.unwrap_or_else(|| "MS-DOS".to_string()),
        developer: blank_to_none(x.developer),
        publisher: blank_to_none(x.publisher),
        release_date: blank_to_none(x.release_date),
        year,
        genre: blank_to_none(x.genre),
        series: blank_to_none(x.series),
        play_mode: blank_to_none(x.play_mode),
        rating: x.community_star_rating
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|&r| r > 0.0),
        // Vote count separates a 5.0 from one voter from a 4.6 from fifty -
        // "Top rated" orders by it inside each star bucket.
        rating_votes: x
            .community_star_rating_total_votes
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&v| v > 0),
        description: blank_to_none(x.notes),
        notes: None,
        source: blank_to_none(x.source),
        application_path: blank_to_none(x.application_path),
        dosbox_conf: x
            .root_folder
            .as_deref()
            .filter(|rf| !rf.is_empty())
            .map(|rf| format!("{}/dosbox.conf", rf)),
        status: blank_to_none(x.status),
        region: blank_to_none(x.region),
        max_players: x.max_players.as_deref().and_then(|s| s.parse().ok()),
        language,
        shortcode,
        available_languages: None,
        variant_titles: None,
        torrent_source: None,
        in_library: false,
        installed: false,
        game_torrent_index: None,
        gamedata_torrent_index: None,
        download_size: None,
        has_thumbnail: false,
        dosbox_variant: None, // populated later by generate_db from dosbox.txt
        favorited: false,
        thumbnail_key: None, // populated by generate_db from normalized title
        music_file,
        manual_path: blank_to_none(x.manual_path),
        last_played: None,
    }
}

/// Parse a LaunchBox XML game database from a buffered reader.
/// `shortcode_segment` selects the path component used for shortcode extraction
/// (e.g. "!dos" for eXoDOS, "!windows" for eXoWin3x).
/// LaunchBox catalogues carry one entry for the pack itself, pinned to the top
/// of the list by a `!`-prefixed SortTitle. The shape varies per pack:
/// eXoWin3x's has no ApplicationPath at all, eXoDOS's and eXoWin9x's point at
/// a root-level "Setup <pack>.bat". What they share is the artificial sort
/// prefix and the absence of a real game path (every launchable game lives
/// under `eXo\...`, so its path has a directory separator). The GLP rows that
/// legitimately lack paths carry no SortTitle at all and stay in.
fn is_pack_sentinel(g: &Game) -> bool {
    // No dosbox_conf guard: the eXoDOS/eXoWin9x sentinels DO carry a
    // RootFolder ("..\"), which the field mapping turns into a junk conf path.
    let pathless_or_root = match g.application_path.as_deref() {
        None => true,
        Some(p) => !p.contains('\\') && !p.contains('/'),
    };
    pathless_or_root && g.sort_title.as_deref().is_some_and(|s| s.starts_with('!'))
}

pub fn parse_games_xml<R: BufRead>(reader: R, shortcode_segment: &str) -> ImportResult<Vec<Game>> {
    let doc: LaunchBoxGames = from_reader(reader)?;
    let games: Vec<Game> = doc
        .games
        .into_iter()
        .map(|x| xml_game_to_game(x, shortcode_segment))
        .filter(|g| !g.title.is_empty())
        .filter(|g| !is_pack_sentinel(g))
        .collect();
    log::info!("Parsed {} games from XML", games.len());
    Ok(games)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    // ── derive_music_file ────────────────────────────────────────────────────

    #[test]
    fn music_file_derivation() {
        // An explicit MusicPath wins, basename only, extension kept verbatim.
        assert_eq!(
            derive_music_file("Furcol", Some(r"Music\MS-DOS\Furcol (1997).XM"), Some("false")),
            Some("Furcol (1997).XM".to_string())
        );
        // Without a MusicPath the flag alone names the default mp3.
        assert_eq!(
            derive_music_file("+K (1996)", None, Some("false")),
            Some("+K (1996).mp3".to_string())
        );
        // Missing music, or no flag at all: nothing to probe for.
        assert_eq!(derive_music_file("Bingo", None, Some("true")), None);
        assert_eq!(derive_music_file("Bingo", None, None), None);
        // A blank MusicPath falls through to the flag.
        assert_eq!(
            derive_music_file("Game", Some(""), Some("false")),
            Some("Game.mp3".to_string())
        );
    }

    // ── extract_shortcode ────────────────────────────────────────────────────

    #[test]
    fn extract_shortcode_dos() {
        let path = Some(r"eXo\eXoDOS\!dos\SQ5\Space Quest V.bat".to_string());
        assert_eq!(extract_shortcode(&path, "!dos"), Some("SQ5".to_string()));
    }

    #[test]
    fn extract_shortcode_german_lang_dir_skipped() {
        // German LP games have an extra !german dir before the shortcode
        let path = Some(r"eXo\eXoDOS\!dos\!german\SQ5\Space Quest V DE.bat".to_string());
        assert_eq!(extract_shortcode(&path, "!dos"), Some("SQ5".to_string()));
    }

    #[test]
    fn extract_shortcode_windows_collection() {
        let path = Some(r"eXo\eXoWin3x\!windows\MYST\Myst.bat".to_string());
        assert_eq!(extract_shortcode(&path, "!windows"), Some("MYST".to_string()));
    }

    #[test]
    fn extract_shortcode_win9x_year_dir_skipped() {
        // eXoWin9x nests a 4-digit year dir; the title dir is the shortcode
        let path =
            Some(r"eXo\eXoWin9x\!win9x\1995\Connect4 (1995)\Connect4 (1995).bat".to_string());
        assert_eq!(
            extract_shortcode(&path, "!win9x"),
            Some("Connect4 (1995)".to_string())
        );
    }

    #[test]
    fn extract_shortcode_four_digit_code_without_subdir_is_kept() {
        // A real 4-digit shortcode (eXoDOS "1939") sits directly before the
        // bat and must not be mistaken for a year directory
        let path = Some(r"eXo\eXoDOS\!dos\1939\1939.bat".to_string());
        assert_eq!(extract_shortcode(&path, "!dos"), Some("1939".to_string()));
    }

    #[test]
    fn extract_shortcode_missing_segment_returns_none() {
        let path = Some(r"eXo\eXoDOS\SQ5\Space Quest V.bat".to_string());
        assert_eq!(extract_shortcode(&path, "!dos"), None);
    }

    #[test]
    fn extract_shortcode_none_path_returns_none() {
        assert_eq!(extract_shortcode(&None, "!dos"), None);
    }

    // ── extract_year ────────────────────────────────────────────────────────

    #[test]
    fn extract_year_valid_iso_date() {
        assert_eq!(extract_year(&Some("1993-05-01T00:00:00".to_string())), Some(1993));
    }

    #[test]
    fn extract_year_year_only() {
        assert_eq!(extract_year(&Some("1999".to_string())), Some(1999));
    }

    #[test]
    fn extract_year_empty_string_returns_none() {
        assert_eq!(extract_year(&Some(String::new())), None);
    }

    #[test]
    fn extract_year_none_returns_none() {
        assert_eq!(extract_year(&None), None);
    }

    #[test]
    fn extract_year_non_numeric_returns_none() {
        assert_eq!(extract_year(&Some("XXXX-01-01".to_string())), None);
    }

    // ── extract_language ────────────────────────────────────────────────────

    #[test]
    fn extract_language_normalizes_spelled_out_names() {
        assert_eq!(extract_language(&Some("Language: German".to_string())), "DE");
        assert_eq!(extract_language(&Some("Language: Japanese".to_string())), "JA");
        // Unknown values pass through instead of being guessed at.
        assert_eq!(extract_language(&Some("Language: Klingon".to_string())), "KLINGON");
    }

    #[test]
    fn extract_language_de() {
        assert_eq!(extract_language(&Some("Language: DE".to_string())), "DE");
    }

    #[test]
    fn extract_language_no_tag_defaults_to_en() {
        assert_eq!(extract_language(&Some("Playlist: Roland MT-32".to_string())), "EN");
        assert_eq!(extract_language(&None), "EN");
    }

    #[test]
    fn extract_language_playlist_combo() {
        assert_eq!(
            extract_language(&Some("Playlist: Roland MT-32; Language: FR".to_string())),
            "FR"
        );
    }

    #[test]
    fn extract_language_code_uppercased() {
        // The language code value is uppercased regardless of the casing in the XML.
        // Note: the "Language:" tag itself must be capital-L - strip_prefix is case-sensitive.
        assert_eq!(extract_language(&Some("Language: pl".to_string())), "PL");
    }

    // ── parse_games_xml ─────────────────────────────────────────────────────

    const FIXTURE_XML: &str = r#"<?xml version="1.0"?>
<LaunchBox>
  <Game>
    <Title>Space Quest V</Title>
    <ApplicationPath>eXo\eXoDOS\!dos\SQ5\Space Quest V.bat</ApplicationPath>
    <ReleaseDate>1993-03-01T00:00:00</ReleaseDate>
    <Genre>Adventure</Genre>
    <Series>Language: EN</Series>
    <CommunityStarRating>4.2</CommunityStarRating>
  </Game>
  <Game>
    <Title>Space Quest V DE</Title>
    <ApplicationPath>eXo\eXoDOS\!dos\!german\SQ5\Space Quest V.bat</ApplicationPath>
    <Series>Language: DE</Series>
  </Game>
  <Game>
    <Title></Title>
    <ApplicationPath>eXo\eXoDOS\!dos\EMPTY\empty.bat</ApplicationPath>
  </Game>
</LaunchBox>"#;

    #[test]
    fn parse_games_xml_fixture_count_and_fields() {
        let reader = BufReader::new(FIXTURE_XML.as_bytes());
        let games = parse_games_xml(reader, "!dos").unwrap();

        // Empty-title game must be filtered out
        assert_eq!(games.len(), 2, "empty-title game must be filtered");

        let en = games.iter().find(|g| g.language == "EN").unwrap();
        assert_eq!(en.title, "Space Quest V");
        assert_eq!(en.shortcode.as_deref(), Some("SQ5"));
        assert_eq!(en.year, Some(1993));
        assert_eq!(en.genre.as_deref(), Some("Adventure"));
        assert!((en.rating.unwrap() - 4.2).abs() < 0.001);

        let de = games.iter().find(|g| g.language == "DE").unwrap();
        assert_eq!(de.title, "Space Quest V DE");
        assert_eq!(de.shortcode.as_deref(), Some("SQ5"));
    }

    // Pack sentinels come in two shapes: pathless (eXoWin3x) and pointing at a
    // root-level Setup bat (eXoDOS, eXoWin9x). Both must be dropped; a real
    // game with a directory path stays even if someone gave it a SortTitle.
    #[test]
    fn parse_games_xml_drops_both_sentinel_shapes() {
        let xml = r#"<?xml version="1.0"?>
<LaunchBox>
  <Game>
    <Title>eXoDOS</Title>
    <SortTitle>! eXoDOS</SortTitle>
    <ApplicationPath>Setup eXoDOS.bat</ApplicationPath>
    <RootFolder>..\</RootFolder>
  </Game>
  <Game>
    <Title>eXoWin3x</Title>
    <SortTitle>! eXoWin3x</SortTitle>
  </Game>
  <Game>
    <Title>Space Quest V</Title>
    <SortTitle>!Pinned But Real</SortTitle>
    <ApplicationPath>eXo\eXoDOS\!dos\SQ5\dosbox.conf</ApplicationPath>
  </Game>
</LaunchBox>"#;
        let games = parse_games_xml(BufReader::new(xml.as_bytes()), "!dos").unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title, "Space Quest V");
    }
}
