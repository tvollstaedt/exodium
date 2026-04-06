use rusqlite::{params, Connection, Row};

use super::DbResult;
use crate::models::Game;

const GAME_COLUMNS: &str =
    "id, title, sort_title, platform, developer, publisher,
     release_date, year, genre, series, play_mode, rating,
     description, notes, source, application_path, dosbox_conf,
     status, region, max_players, language, shortcode, torrent_source,
     in_library, installed, game_torrent_index, gamedata_torrent_index, download_size,
     has_thumbnail, dosbox_variant, favorited";

fn row_to_game(row: &Row) -> rusqlite::Result<Game> {
    Ok(Game {
        id: row.get(0)?,
        title: row.get(1)?,
        sort_title: row.get(2)?,
        platform: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        developer: row.get(4)?,
        publisher: row.get(5)?,
        release_date: row.get(6)?,
        year: row.get(7)?,
        genre: row.get(8)?,
        series: row.get(9)?,
        play_mode: row.get(10)?,
        rating: row.get(11)?,
        description: row.get(12)?,
        notes: row.get(13)?,
        source: row.get(14)?,
        application_path: row.get(15)?,
        dosbox_conf: row.get(16)?,
        status: row.get(17)?,
        region: row.get(18)?,
        max_players: row.get(19)?,
        language: row.get::<_, Option<String>>(20)?.unwrap_or_else(|| "EN".to_string()),
        shortcode: row.get(21)?,
        available_languages: None, // populated by merged query
        torrent_source: row.get(22)?,
        in_library: row.get::<_, i32>(23).unwrap_or(0) != 0,
        installed: row.get::<_, i32>(24).unwrap_or(0) != 0,
        game_torrent_index: row.get(25)?,
        gamedata_torrent_index: row.get(26)?,
        download_size: row.get(27)?,
        has_thumbnail: row.get::<_, i32>(28).unwrap_or(0) != 0,
        dosbox_variant: row.get(29)?,
        favorited: row.get::<_, i32>(30).unwrap_or(0) != 0,
    })
}

/// Clear all games (used before re-import to prevent duplicates).
pub fn clear_games(conn: &Connection) -> DbResult<()> {
    conn.execute_batch("DELETE FROM games")?;
    Ok(())
}

/// Insert games in a single transaction. Returns the number inserted.
pub fn insert_games(conn: &Connection, games: &[Game]) -> DbResult<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut stmt = tx.prepare_cached(
        "INSERT INTO games (
            title, sort_title, platform, developer, publisher,
            release_date, year, genre, series, play_mode,
            rating, description, notes, source, application_path,
            dosbox_conf, status, region, max_players, language, shortcode
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21
        )",
    )?;

    let mut count = 0;
    for game in games {
        stmt.execute(params![
            game.title,
            game.sort_title,
            game.platform,
            game.developer,
            game.publisher,
            game.release_date,
            game.year,
            game.genre,
            game.series,
            game.play_mode,
            game.rating,
            game.description,
            game.notes,
            game.source,
            game.application_path,
            game.dosbox_conf,
            game.status,
            game.region,
            game.max_players,
            game.language,
            game.shortcode,
        ])?;
        count += 1;
    }
    drop(stmt);
    tx.commit()?;

    log::info!("Inserted {} games into database", count);
    Ok(count)
}

/// Update torrent indices and download size for a game by title.
pub fn set_game_torrent_info(
    conn: &Connection,
    title: &str,
    game_index: Option<i64>,
    gamedata_index: Option<i64>,
    download_size: Option<i64>,
) -> DbResult<usize> {
    let changed = conn.execute(
        "UPDATE games SET game_torrent_index = ?1, gamedata_torrent_index = ?2,
         download_size = ?3 WHERE title = ?4",
        params![game_index, gamedata_index, download_size, title],
    )?;
    Ok(changed)
}

/// Add a game to the user's library (triggered on download).
pub fn set_in_library(conn: &Connection, game_id: i64) -> DbResult<()> {
    conn.execute("UPDATE games SET in_library = 1 WHERE id = ?1", params![game_id])?;
    Ok(())
}

/// Mark a game as installed (also sets in_library).
pub fn set_game_installed(conn: &Connection, game_id: i64, installed: bool) -> DbResult<()> {
    conn.execute(
        "UPDATE games SET installed = ?1, in_library = CASE WHEN ?1 = 1 THEN 1 ELSE in_library END WHERE id = ?2",
        params![installed as i32, game_id],
    )?;
    Ok(())
}

/// Toggle the favorited flag for a single game row.
/// Returns the new favorited state.
pub fn toggle_favorite(conn: &Connection, id: i64) -> DbResult<bool> {
    let current: i32 = conn.query_row(
        "SELECT favorited FROM games WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let new_val = if current == 0 { 1i32 } else { 0i32 };
    conn.execute(
        "UPDATE games SET favorited = ?1 WHERE id = ?2",
        params![new_val, id],
    )?;
    Ok(new_val != 0)
}

/// Filter parameters for game queries.
pub struct GameFilter<'a> {
    pub query: &'a str,
    pub genre: &'a str,
    pub sort_by: &'a str,
    pub collection: &'a str,
    pub favorites_only: bool,
}

/// Build WHERE clause from filters.
fn build_where_clause(f: &GameFilter) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut conditions = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if !f.query.is_empty() {
        params.push(Box::new(format!("%{}%", f.query)));
        conditions.push(format!("title LIKE ?{}", params.len()));
    }

    if !f.genre.is_empty() {
        // Genre is semicolon-separated, use LIKE for partial match
        params.push(Box::new(format!("%{}%", f.genre)));
        conditions.push(format!("genre LIKE ?{}", params.len()));
    }

    if !f.collection.is_empty() {
        params.push(Box::new(f.collection.to_string()));
        conditions.push(format!("torrent_source = ?{}", params.len()));
    }

    if f.favorites_only {
        conditions.push("favorited = 1".to_string());
    }

    let clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    (clause, params)
}

fn order_clause(sort_by: &str) -> &str {
    match sort_by {
        "year_asc" => "ORDER BY COALESCE(year, 9999) ASC, title ASC",
        "year_desc" => "ORDER BY COALESCE(year, 0) DESC, title ASC",
        "rating" => "ORDER BY COALESCE(rating, 0) DESC, title ASC",
        "title_desc" => "ORDER BY title DESC",
        _ => "ORDER BY title ASC",
    }
}

/// Count total games with filters.
pub fn count_games(conn: &Connection, query: &str) -> DbResult<usize> {
    let f = GameFilter { query, genre: "", sort_by: "", collection: "", favorites_only: false };
    count_games_filtered(conn, &f)
}

pub fn count_games_filtered(conn: &Connection, f: &GameFilter) -> DbResult<usize> {
    let (where_clause, params) = build_where_clause(f);
    let sql = format!("SELECT COUNT(*) FROM games{}", where_clause);
    let mut stmt = conn.prepare_cached(&sql)?;
    let count: usize = stmt.query_row(rusqlite::params_from_iter(&params), |row| row.get(0))?;
    Ok(count)
}

pub fn fetch_games_filtered(
    conn: &Connection,
    page: usize,
    per_page: usize,
    f: &GameFilter,
) -> DbResult<Vec<Game>> {
    let offset = (page.saturating_sub(1)) * per_page;
    let (where_clause, mut params) = build_where_clause(f);
    let order = order_clause(f.sort_by);

    params.push(Box::new(per_page as i64));
    let limit_idx = params.len();
    params.push(Box::new(offset as i64));
    let offset_idx = params.len();

    let sql = format!(
        "SELECT {} FROM games{} {} LIMIT ?{} OFFSET ?{}",
        GAME_COLUMNS, where_clause, order, limit_idx, offset_idx
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let games = stmt
        .query_map(rusqlite::params_from_iter(&params), row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(games)
}

fn offset_val(page: usize, per_page: usize) -> usize {
    (page.saturating_sub(1)) * per_page
}

/// Get all language variants for a shortcode.
pub fn fetch_game_variants(conn: &Connection, shortcode: &str) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games WHERE shortcode = ?1 ORDER BY CASE language WHEN 'EN' THEN 0 ELSE 1 END, language",
        GAME_COLUMNS
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let games = stmt
        .query_map(params![shortcode], row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(games)
}

/// Get all distinct genres (split from semicolon-separated values).
pub fn get_genres(conn: &Connection) -> DbResult<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT DISTINCT genre FROM games WHERE genre IS NOT NULL AND genre != ''")?;
    let raw: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    // Split semicolon-separated genres and deduplicate
    let mut genres: Vec<String> = raw
        .iter()
        .flat_map(|g| g.split(';').map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    genres.sort();
    genres.dedup();
    Ok(genres)
}

/// Fetch installed games — flat list, one row per installed variant.
pub fn fetch_installed_games(conn: &Connection) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games WHERE in_library = 1 ORDER BY title, language",
        GAME_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let games: Vec<Game> = stmt
        .query_map([], row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(games)
}

/// Fetch a single game by ID.
pub fn fetch_game_by_id(conn: &Connection, id: i64) -> DbResult<Option<Game>> {
    let sql = format!("SELECT {} FROM games WHERE id = ?1", GAME_COLUMNS);
    let mut stmt = conn.prepare_cached(&sql)?;

    let game = stmt.query_row(params![id], row_to_game).optional()?;
    Ok(game)
}

/// Get a config value by key.
pub fn get_config(conn: &Connection, key: &str) -> DbResult<Option<String>> {
    let mut stmt = conn.prepare_cached("SELECT value FROM config WHERE key = ?1")?;
    let result = stmt
        .query_row(params![key], |row| row.get(0))
        .optional()?;
    Ok(result)
}

/// Set a config value (upsert).
pub fn set_config(conn: &Connection, key: &str, value: &str) -> DbResult<()> {
    conn.execute(
        "INSERT INTO config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Trait extension to make `.optional()` work on rusqlite results.
trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
