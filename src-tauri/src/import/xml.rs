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
/// Only the fields we care about — everything else is silently ignored.
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
}

fn blank_to_none(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

/// Extract shortcode from application_path.
/// e.g., "eXo\eXoDOS\!dos\captlsm\Capitalism (1995).bat" → "captlsm"
/// e.g., "eXo\eXoDOS\!dos\!german\SQ5\Space Quest V (1993).bat" → "SQ5"
fn extract_shortcode(app_path: &Option<String>) -> Option<String> {
    let path = app_path.as_ref()?;
    let normalized = path.replace('\\', "/");
    let dos_idx = normalized.find("/!dos/")?;
    let after_dos = &normalized[dos_idx + 6..]; // skip "/!dos/"
    // Skip language dir if present (e.g., "!german/")
    let after_lang = if after_dos.starts_with('!') {
        after_dos.find('/')?.checked_add(1).and_then(|i| after_dos.get(i..))?
    } else {
        after_dos
    };
    // Take the shortcode (next path segment)
    let end = after_lang.find('/')?;
    Some(after_lang[..end].to_string())
}

fn extract_year(date_str: &Option<String>) -> Option<i32> {
    date_str.as_ref().and_then(|s| s.get(..4)?.parse().ok())
}

/// Extract language code from the Series field.
/// e.g. "Language: DE" → "DE", "Playlist: Roland MT-32; Language: FR" → "FR"
fn extract_language(series: &Option<String>) -> String {
    if let Some(s) = series {
        for part in s.split(';') {
            let trimmed = part.trim();
            if let Some(lang) = trimmed.strip_prefix("Language:") {
                let code = lang.trim().to_uppercase();
                if !code.is_empty() {
                    return code;
                }
            }
        }
    }
    "EN".to_string()
}

impl From<XmlGame> for Game {
    fn from(x: XmlGame) -> Self {
        let year = extract_year(&x.release_date);
        let language = extract_language(&x.series);
        let shortcode = extract_shortcode(&x.application_path)
            .or_else(|| extract_shortcode(&x.root_folder));
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
            description: blank_to_none(x.notes),
            notes: None,
            source: blank_to_none(x.source),
            application_path: blank_to_none(x.application_path),
            dosbox_conf: x
                .root_folder
                .as_deref()
                .map(|rf| format!("{}/dosbox.conf", rf)),
            status: blank_to_none(x.status),
            region: blank_to_none(x.region),
            max_players: x.max_players.as_deref().and_then(|s| s.parse().ok()),
            language,
            shortcode,
            available_languages: None,
            torrent_source: None,
            in_library: false,
            installed: false,
            game_torrent_index: None,
            gamedata_torrent_index: None,
            download_size: None,
            has_thumbnail: false,
        }
    }
}

/// Parse the eXoDOS MS-DOS.xml game database from a buffered reader.
/// The XML is ~37 MB with ~7,667 <Game> entries.
pub fn parse_games_xml<R: BufRead>(reader: R) -> ImportResult<Vec<Game>> {
    let doc: LaunchBoxGames = from_reader(reader)?;
    let games: Vec<Game> = doc
        .games
        .into_iter()
        .map(Game::from)
        .filter(|g| !g.title.is_empty())
        .collect();
    log::info!("Parsed {} games from XML", games.len());
    Ok(games)
}
