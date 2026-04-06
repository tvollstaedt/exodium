use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: Option<i64>,
    pub name: String,
    /// Populated from the playlist_games join table, not stored as a column.
    #[serde(default)]
    pub game_ids: Vec<i64>,
}
