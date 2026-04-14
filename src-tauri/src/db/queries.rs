use rusqlite::{params, Connection, Row};

use super::DbResult;
use crate::models::Game;

const GAME_COLUMNS: &str =
    "id, title, sort_title, platform, developer, publisher,
     release_date, year, genre, series, play_mode, rating,
     description, notes, source, application_path, dosbox_conf,
     status, region, max_players, language, shortcode, torrent_source,
     in_library, installed, game_torrent_index, gamedata_torrent_index, download_size,
     has_thumbnail, dosbox_variant, favorited, thumbnail_key";

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
        thumbnail_key: row.get(31)?,
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

pub fn clear_in_library(conn: &Connection, game_id: i64) -> DbResult<()> {
    conn.execute("UPDATE games SET in_library = 0 WHERE id = ?1", params![game_id])?;
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
        "rating" => "ORDER BY COALESCE(rating, -1) DESC, title ASC",
        "title_desc" => "ORDER BY title DESC",
        "genre" => "ORDER BY COALESCE(genre, 'zzz') ASC, title ASC",
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


/// Get all language variants for a shortcode.
pub fn fetch_game_variants(conn: &Connection, shortcode: &str) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games WHERE shortcode = ?1 ORDER BY CASE language WHEN 'EN' THEN 0 ELSE 1 END, language",
        GAME_COLUMNS
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut games = stmt
        .query_map(params![shortcode], row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;

    // LP overlay ZIPs (< 1 MB) are just localized bat files — they require the EN base game
    // to function. Always show the combined total (LP overlay + EN base) so the user sees a
    // consistent, realistic game size regardless of whether EN is already on disk.
    if let Some(en_game) = games.iter().find(|g| g.language == "EN") {
        let en_size = en_game.download_size.unwrap_or(0);
        if en_size > 0 {
            for game in &mut games {
                if game.language != "EN" {
                    let lp_size = game.download_size.unwrap_or(0);
                    if lp_size < 1_000_000 {
                        game.download_size = Some(lp_size + en_size);
                    }
                }
            }
        }
    }

    Ok(games)
}

/// Get all distinct genres (split from semicolon-separated values).
pub fn get_genres(conn: &Connection, collection: &str) -> DbResult<Vec<String>> {
    let (sql, params) = if collection.is_empty() {
        ("SELECT DISTINCT genre FROM games WHERE genre IS NOT NULL AND genre != ''".to_string(), vec![])
    } else {
        ("SELECT DISTINCT genre FROM games WHERE genre IS NOT NULL AND genre != '' AND torrent_source = ?1".to_string(),
         vec![collection.to_string()])
    };
    let mut stmt = conn.prepare_cached(&sql)?;
    let raw: Vec<String> = stmt
        .query_map(rusqlite::params_from_iter(&params), |row| row.get(0))?
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

/// Return the distinct section-header keys for the current filter + sort, matching
/// the groupKey() logic on the frontend. Used to populate the jump bar before all
/// games are loaded via infinite scroll.
pub fn get_section_keys(conn: &Connection, f: &GameFilter) -> DbResult<Vec<String>> {
    let (where_clause, params) = build_where_clause(f);

    let (select_expr, order_expr) = match f.sort_by {
        "title" => (
            "CASE WHEN UPPER(SUBSTR(COALESCE(sort_title,title),1,1)) GLOB '[A-Z]' \
             THEN UPPER(SUBSTR(COALESCE(sort_title,title),1,1)) ELSE '#' END",
            "key ASC",
        ),
        "title_desc" => (
            "CASE WHEN UPPER(SUBSTR(COALESCE(sort_title,title),1,1)) GLOB '[A-Z]' \
             THEN UPPER(SUBSTR(COALESCE(sort_title,title),1,1)) ELSE '#' END",
            "key DESC",
        ),
        "year_asc"  => ("COALESCE(CAST(year AS TEXT),'Unknown')", "COALESCE(year,9999) ASC"),
        "year_desc" => ("COALESCE(CAST(year AS TEXT),'Unknown')", "COALESCE(year,0) DESC"),
        "genre"     => ("COALESCE(genre,'Unknown')",               "COALESCE(genre,'zzz') ASC"),
        "rating"    => ("CAST(ROUND(COALESCE(rating,-1)) AS INTEGER)", "COALESCE(rating,-1) DESC"),
        _           => return Ok(vec![]),
    };

    let sql = format!(
        "SELECT DISTINCT {select} as key FROM games {where_clause} ORDER BY {order}",
        select = select_expr,
        where_clause = where_clause,
        order = order_expr,
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let raw: Vec<String> = stmt
        .query_map(rusqlite::params_from_iter(&params), |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();

    if f.sort_by == "rating" {
        return Ok(raw.iter().map(|s| {
            match s.parse::<i64>() {
                Ok(n) if n >= 0 => {
                    let n = n.clamp(0, 5) as usize;
                    "★".repeat(n) + &"☆".repeat(5 - n)
                }
                _ => "Unrated".to_string(),
            }
        }).collect());
    }

    Ok(raw)
}

/// Fetch installed games — flat list, one row per installed variant.
pub fn fetch_installed_games(conn: &Connection) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games WHERE installed = 1 ORDER BY title, language",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Game;
    use pretty_assertions::assert_eq;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn
    }

    fn make_game(title: &str) -> Game {
        Game {
            id: None,
            title: title.to_string(),
            sort_title: None,
            platform: "MS-DOS".to_string(),
            developer: None,
            publisher: None,
            release_date: None,
            year: None,
            genre: None,
            series: None,
            play_mode: None,
            rating: None,
            description: None,
            notes: None,
            source: None,
            application_path: None,
            dosbox_conf: None,
            status: None,
            region: None,
            max_players: None,
            language: "EN".to_string(),
            shortcode: None,
            available_languages: None,
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
        }
    }

    #[test]
    fn insert_and_fetch_game() {
        let conn = open_test_db();
        let game = make_game("Space Quest V");
        insert_games(&conn, &[game]).unwrap();

        let id: i64 = conn.query_row("SELECT id FROM games WHERE title = ?1", params!["Space Quest V"], |r| r.get(0)).unwrap();
        let fetched = fetch_game_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(fetched.title, "Space Quest V");
        assert_eq!(fetched.language, "EN");
        assert!(!fetched.installed);
        assert!(!fetched.favorited);
    }

    #[test]
    fn search_by_query() {
        let conn = open_test_db();
        insert_games(&conn, &[
            make_game("Space Quest V"),
            make_game("Space Quest IV"),
            make_game("Doom"),
        ]).unwrap();

        let f = GameFilter { query: "Space", genre: "", sort_by: "", collection: "", favorites_only: false };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|g| g.title.contains("Space")));
    }

    #[test]
    fn filter_by_genre() {
        let conn = open_test_db();
        let mut rpg = make_game("Baldur's Gate");
        rpg.genre = Some("Role-Playing;Strategy".to_string());
        let mut action = make_game("Doom");
        action.genre = Some("Action;Shooter".to_string());
        insert_games(&conn, &[rpg, action]).unwrap();

        let f = GameFilter { query: "", genre: "Role-Playing", sort_by: "", collection: "", favorites_only: false };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Baldur's Gate");
    }

    #[test]
    fn filter_by_collection() {
        let conn = open_test_db();
        insert_games(&conn, &[make_game("Doom"), make_game("Doom DE")]).unwrap();

        // torrent_source is set post-import by the torrent matching phase,
        // not by insert_games — update it directly here.
        conn.execute("UPDATE games SET torrent_source = 'eXoDOS' WHERE title = 'Doom'", []).unwrap();
        conn.execute("UPDATE games SET torrent_source = 'eXoDOS_GLP' WHERE title = 'Doom DE'", []).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "eXoDOS_GLP", favorites_only: false };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Doom DE");
    }

    #[test]
    fn filter_favorites_only() {
        let conn = open_test_db();
        insert_games(&conn, &[make_game("Doom"), make_game("Quake")]).unwrap();
        let id: i64 = conn.query_row("SELECT id FROM games WHERE title = 'Doom'", [], |r| r.get(0)).unwrap();
        toggle_favorite(&conn, id).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: true };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Doom");
    }

    #[test]
    fn pagination() {
        let conn = open_test_db();
        let games: Vec<Game> = (1..=10).map(|i| make_game(&format!("Game {:02}", i))).collect();
        insert_games(&conn, &games).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false };
        let page1 = fetch_games_filtered(&conn, 1, 4, &f).unwrap();
        let page2 = fetch_games_filtered(&conn, 2, 4, &f).unwrap();
        let total = count_games_filtered(&conn, &f).unwrap();

        assert_eq!(page1.len(), 4);
        assert_eq!(page2.len(), 4);
        assert_eq!(total, 10);

        // Pages must not overlap
        let ids1: std::collections::HashSet<_> = page1.iter().map(|g| &g.title).collect();
        let ids2: std::collections::HashSet<_> = page2.iter().map(|g| &g.title).collect();
        assert!(ids1.is_disjoint(&ids2));
    }

    #[test]
    fn toggle_favorite_persists() {
        let conn = open_test_db();
        insert_games(&conn, &[make_game("Doom")]).unwrap();
        let id: i64 = conn.query_row("SELECT id FROM games WHERE title = 'Doom'", [], |r| r.get(0)).unwrap();

        let new_state = toggle_favorite(&conn, id).unwrap();
        assert!(new_state, "first toggle should return true");

        let fetched = fetch_game_by_id(&conn, id).unwrap().unwrap();
        assert!(fetched.favorited);

        let new_state2 = toggle_favorite(&conn, id).unwrap();
        assert!(!new_state2, "second toggle should return false");

        let fetched2 = fetch_game_by_id(&conn, id).unwrap().unwrap();
        assert!(!fetched2.favorited);
    }

    #[test]
    fn config_round_trip() {
        let conn = open_test_db();
        assert_eq!(get_config(&conn, "data_dir").unwrap(), None);

        set_config(&conn, "data_dir", "/home/user/eXoDOS").unwrap();
        assert_eq!(get_config(&conn, "data_dir").unwrap().as_deref(), Some("/home/user/eXoDOS"));

        // Upsert: update existing key
        set_config(&conn, "data_dir", "/mnt/games").unwrap();
        assert_eq!(get_config(&conn, "data_dir").unwrap().as_deref(), Some("/mnt/games"));
    }

    #[test]
    fn set_in_library_and_installed() {
        let conn = open_test_db();
        insert_games(&conn, &[make_game("Doom")]).unwrap();
        let id: i64 = conn.query_row("SELECT id FROM games WHERE title = 'Doom'", [], |r| r.get(0)).unwrap();

        set_in_library(&conn, id).unwrap();
        let g = fetch_game_by_id(&conn, id).unwrap().unwrap();
        assert!(g.in_library);
        assert!(!g.installed);

        set_game_installed(&conn, id, true).unwrap();
        let g = fetch_game_by_id(&conn, id).unwrap().unwrap();
        assert!(g.in_library);
        assert!(g.installed);

        set_game_installed(&conn, id, false).unwrap();
        let g = fetch_game_by_id(&conn, id).unwrap().unwrap();
        assert!(g.in_library, "in_library stays set after uninstall");
        assert!(!g.installed);
    }

    #[test]
    fn count_games_filtered_matches_fetch() {
        let conn = open_test_db();
        let games: Vec<Game> = ["Alpha", "Beta", "Gamma", "Delta"]
            .iter()
            .map(|t| make_game(t))
            .collect();
        insert_games(&conn, &games).unwrap();

        let f = GameFilter { query: "a", genre: "", sort_by: "", collection: "", favorites_only: false };
        let count = count_games_filtered(&conn, &f).unwrap();
        let fetched = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(count, fetched.len(), "count must match number of fetched rows");
    }

    #[test]
    fn get_genres_splits_semicolons() {
        let conn = open_test_db();
        let mut g1 = make_game("A");
        g1.genre = Some("Action;Adventure".to_string());
        let mut g2 = make_game("B");
        g2.genre = Some("Action;Puzzle".to_string());
        insert_games(&conn, &[g1, g2]).unwrap();

        let genres = get_genres(&conn, "").unwrap();
        assert!(genres.contains(&"Action".to_string()));
        assert!(genres.contains(&"Adventure".to_string()));
        assert!(genres.contains(&"Puzzle".to_string()));
        // Deduplication: "Action" appears once despite two games
        assert_eq!(genres.iter().filter(|g| g.as_str() == "Action").count(), 1);
    }
}
