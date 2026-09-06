//! Playlist commands: curated (read-only, shipped with the catalog) and
//! user-created playlists. All DB-touching commands are async per the
//! project convention (sync commands run on the native main thread).

use tauri::State;

use super::games::DbState;
use crate::db::queries;
use crate::models::Playlist;

#[tauri::command]
pub async fn get_playlists(state: State<'_, DbState>) -> Result<Vec<Playlist>, String> {
    let conn = state.lock()?;
    queries::fetch_playlists(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_playlist(state: State<'_, DbState>, name: String) -> Result<i64, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Playlist name cannot be empty".into());
    }
    let conn = state.lock()?;
    queries::create_playlist(&conn, &name).map_err(|e| map_name_conflict(e, &name))
}

/// UNIQUE(kind, name) violations read as raw SQLite otherwise; both create
/// and rename hit the same constraint.
fn map_name_conflict(e: crate::db::DbError, name: &str) -> String {
    match e {
        crate::db::DbError::Sqlite(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            format!("A playlist named \"{}\" already exists", name)
        }
        other => other.to_string(),
    }
}

#[tauri::command]
pub async fn rename_playlist(
    state: State<'_, DbState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Playlist name cannot be empty".into());
    }
    let conn = state.lock()?;
    queries::rename_playlist(&conn, id, &name).map_err(|e| map_name_conflict(e, &name))
}

#[tauri::command]
pub async fn delete_playlist(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = state.lock()?;
    queries::delete_playlist(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_playlist_membership(
    state: State<'_, DbState>,
    playlist_id: i64,
    game_id: i64,
    member: bool,
) -> Result<(), String> {
    let conn = state.lock()?;
    queries::set_playlist_membership(&conn, playlist_id, game_id, member).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_game_playlists(
    state: State<'_, DbState>,
    game_id: i64,
) -> Result<Vec<i64>, String> {
    let conn = state.lock()?;
    queries::fetch_game_playlist_ids(&conn, game_id).map_err(|e| e.to_string())
}
