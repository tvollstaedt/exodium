use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: Option<i64>,
    pub title: String,
    pub sort_title: Option<String>,
    pub platform: String,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub series: Option<String>,
    pub play_mode: Option<String>,
    pub rating: Option<f64>,
    pub rating_votes: Option<i64>,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub source: Option<String>,
    pub application_path: Option<String>,
    pub dosbox_conf: Option<String>,
    pub status: Option<String>,
    pub region: Option<String>,
    pub max_players: Option<i32>,
    pub language: String,
    pub shortcode: Option<String>,
    pub available_languages: Option<String>,
    /// Titles of the OTHER language variants in this group, unit-separated.
    /// Lets a client-side search over already-loaded rows match a localized
    /// title, the way the Browse SQL filter does. None for single-variant
    /// games.
    pub variant_titles: Option<String>,
    pub torrent_source: Option<String>,
    pub in_library: bool,
    pub installed: bool,
    pub favorited: bool,
    pub game_torrent_index: Option<i64>,
    pub gamedata_torrent_index: Option<i64>,
    pub download_size: Option<i64>,
    pub has_thumbnail: bool,
    pub dosbox_variant: Option<String>,
    /// SHA-256(normalized title)[:16] hex. Filename for the bundled or
    /// content-pack thumbnail. Null when no title is available or the game
    /// predates the content-addressed thumbnail scheme.
    pub thumbnail_key: Option<String>,
    /// LaunchBox ManualPath (e.g. "Manuals\MS-DOS\Capitalism (1995).pdf").
    /// Relative to the torrent root. Null when no manual exists.
    pub manual_path: Option<String>,
    /// ISO 8601 timestamp (UTC) of the last time the game was launched.
    /// Used for ordering only - convert to local time if displaying to user.
    pub last_played: Option<String>,
    /// Hint that the game's GameData archive carries a theme track, and its
    /// file name (e.g. "Capitalism (1995).mp3"): the basename of LaunchBox's
    /// MusicPath, or `<Title>.mp3` when MissingMusic is false. The archive is
    /// the truth - the name is never resolved as a path, only its extension
    /// decides whether the track is worth probing (tracker modules are not).
    pub music_file: Option<String>,
}
