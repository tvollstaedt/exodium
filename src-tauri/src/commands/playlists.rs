//! Playlist commands: curated (read-only, shipped with the catalog) and
//! user-created playlists. All DB-touching commands are async per the
//! project convention (sync commands run on the native main thread).

use tauri::State;

use super::games::DbState;
use super::playlist_io::{self, DownloadPlan, ImportPreview, Pick, PlanItem};
use super::TorrentState;
use crate::db::queries;
use crate::models::Playlist;

#[tauri::command]
pub async fn get_playlists(state: State<'_, DbState>) -> Result<Vec<Playlist>, String> {
    let conn = state.lock()?;
    queries::fetch_playlists(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_playlist(state: State<'_, DbState>, name: String) -> Result<i64, String> {
    super::kids::ensure_unlocked()?;
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
    super::kids::ensure_unlocked()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Playlist name cannot be empty".into());
    }
    let conn = state.lock()?;
    queries::rename_playlist(&conn, id, &name).map_err(|e| map_name_conflict(e, &name))
}

#[tauri::command]
pub async fn delete_playlist(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    super::kids::ensure_unlocked()?;
    let conn = state.lock()?;
    queries::delete_playlist(&conn, id).map_err(|e| e.to_string())?;
    super::kids::on_playlist_deleted(&conn, id);
    Ok(())
}

#[tauri::command]
pub async fn set_playlist_membership(
    state: State<'_, DbState>,
    playlist_id: i64,
    game_id: i64,
    member: bool,
) -> Result<(), String> {
    super::kids::ensure_unlocked()?;
    let conn = state.lock()?;
    queries::set_playlist_membership(&conn, playlist_id, game_id, member).map_err(|e| e.to_string())
}

/// A selection's membership change in one transaction (§25).
#[tauri::command]
pub async fn set_playlist_membership_many(
    state: State<'_, DbState>,
    playlist_id: i64,
    game_ids: Vec<i64>,
    member: bool,
) -> Result<(), String> {
    super::kids::ensure_unlocked()?;
    let conn = state.lock()?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for game_id in game_ids {
        queries::set_playlist_membership(&tx, playlist_id, game_id, member).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_game_playlists(
    state: State<'_, DbState>,
    game_id: i64,
) -> Result<Vec<i64>, String> {
    let conn = state.lock()?;
    queries::fetch_game_playlist_ids(&conn, game_id).map_err(|e| e.to_string())
}

/// A hand-written list is a few KB; anything this size is not one.
const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;

/// Read a `.txt`/`.csv` list and match every title, for the import preview.
#[tauri::command]
pub async fn import_playlist_preview(state: State<'_, DbState>, path: String) -> Result<ImportPreview, String> {
    let path = std::path::PathBuf::from(path);
    let unreadable = |e: std::io::Error| format!("Could not read {}: {e}", path.display());
    if std::fs::metadata(&path).map_err(unreadable)?.len() > MAX_LIST_BYTES {
        return Err(format!("{} is too large for a list of games", path.display()));
    }
    let bytes = std::fs::read(&path).map_err(unreadable)?;
    let is_csv = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    let entries = playlist_io::parse_list(&bytes, is_csv);
    let name = path.file_stem().map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default();
    let conn = state.lock()?;
    let index = playlist_io::CatalogIndex::load(&conn).map_err(|e| e.to_string())?;
    Ok(ImportPreview { name, rows: playlist_io::preview(&index, &entries) })
}

/// The import's last step: a new playlist holding `game_ids`, all or nothing.
#[tauri::command]
pub async fn create_playlist_from(
    state: State<'_, DbState>,
    name: String,
    game_ids: Vec<i64>,
) -> Result<i64, String> {
    super::kids::ensure_unlocked()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Playlist name cannot be empty".into());
    }
    let conn = state.lock()?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let id = queries::create_playlist(&tx, &name).map_err(|e| map_name_conflict(e, &name))?;
    for game_id in game_ids {
        queries::set_playlist_membership(&tx, id, game_id, true).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(id)
}

#[tauri::command]
pub async fn export_playlist(state: State<'_, DbState>, id: i64, path: String) -> Result<(), String> {
    let bytes = {
        let conn = state.lock()?;
        playlist_io::export_csv(&conn, id)?
    };
    std::fs::write(&path, bytes).map_err(|e| format!("Could not write {path}: {e}"))
}

/// How many of the playlist's games "Download playlist" would fetch: none
/// installed in any language, none downloading. Cheap enough for a button.
#[tauri::command]
pub async fn playlist_missing_count(state: State<'_, DbState>, id: i64) -> Result<usize, String> {
    let conn = state.lock()?;
    let groups = playlist_io::playlist_groups(&conn, id, false)?;
    Ok(groups.iter().filter(|g| matches!(playlist_io::pick_variant(g, "EN"), Some(Pick::Fetch(_)))).count())
}

/// What "Download playlist" would fetch in `language` (§24), priced like the
/// panel's language picker.
#[tauri::command]
pub async fn playlist_download_plan(
    state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
    language: String,
) -> Result<DownloadPlan, String> {
    let (mut targets, present, in_progress, languages, data_dir) = {
        let conn = state.lock()?;
        let mut languages = std::collections::BTreeSet::new();
        let (mut targets, mut present, mut in_progress) = (Vec::new(), 0, 0);
        for group in playlist_io::playlist_groups(&conn, id, true)? {
            languages.extend(group.iter().map(|v| v.language.to_uppercase()));
            match playlist_io::pick_variant(&group, &language) {
                Some(Pick::Fetch(game)) => targets.push(*game),
                Some(Pick::Present) => present += 1,
                Some(Pick::InProgress) => in_progress += 1,
                None => {}
            }
        }
        let data_dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
        (targets, present, in_progress, languages, data_dir)
    };
    super::games::deduct_held_bytes(&mut targets, &torrent_state).await;
    let items: Vec<PlanItem> = targets
        .into_iter()
        .filter_map(|g| {
            Some(PlanItem {
                game_id: g.id?,
                title: g.title,
                size_bytes: g.download_size.unwrap_or(0),
                torrent_source: g.torrent_source,
                thumbnail_key: g.thumbnail_key,
            })
        })
        .collect();
    let mut languages: Vec<String> = languages.into_iter().collect();
    languages.sort_by_key(|l| l != "EN");
    Ok(DownloadPlan {
        total_bytes: items.iter().map(|i| i.size_bytes).sum(),
        items,
        present,
        in_progress,
        languages,
        free_bytes: data_dir.and_then(|d| fs4::available_space(std::path::Path::new(&d)).ok()),
    })
}
