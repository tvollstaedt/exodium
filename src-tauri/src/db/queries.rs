use rusqlite::{params, Connection, Row};

use super::DbResult;
use crate::models::Game;

const GAME_COLUMNS: &str =
    "id, title, sort_title, platform, developer, publisher,
     release_date, year, genre, series, play_mode, rating,
     description, notes, source, application_path, dosbox_conf,
     status, region, max_players, language, shortcode, torrent_source,
     in_library, installed, game_torrent_index, gamedata_torrent_index, download_size,
     has_thumbnail, dosbox_variant, favorited, thumbnail_key, manual_path, last_played,
     rating_votes, music_file";

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
        variant_titles: None,
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
        manual_path: row.get(32)?,
        last_played: row.get(33)?,
        rating_votes: row.get(34)?,
        music_file: row.get(35)?,
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
            dosbox_conf, status, region, max_players, language, shortcode,
            manual_path, rating_votes, music_file
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24
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
            game.manual_path,
            game.rating_votes,
            game.music_file,
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
    pub playlist_id: Option<i64>,
    pub with_music: bool,
}

/// SQL expression yielding a row's pack family: its collection's base id, so
/// the four eXoDOS language packs share one family and a collection with its
/// own game tree forms its own. Built from COLLECTION_MAP - adding a pack
/// needs no SQL edit here.
/// `pub` (not `pub(crate)`) because the generate_db example builds the bundled
/// catalog with the same family rule - a private copy there drifted once.
pub fn family_expr(alias: &str) -> String {
    // Only the packs that resolve elsewhere need an arm; a CASE without a match
    // yields NULL, so COALESCE lets every other collection stand for itself.
    let arms: String = crate::COLLECTION_MAP
        .iter()
        .filter(|c| c.id != crate::collection_base_id(c.id))
        .map(|c| format!(" WHEN '{}' THEN '{}'", c.id, crate::collection_base_id(c.id)))
        .collect();
    format!(
        "COALESCE(CASE {a}.torrent_source{arms} END, {a}.torrent_source, 'eXoDOS')",
        a = alias,
        arms = arms
    )
}

/// SQL predicate: this row's catalogue hint names a track the webview can play.
/// Shared with music_shuffle_candidates so the two never drift.
pub(crate) fn playable_music_sql(alias: &str) -> String {
    format!(
        "{a}.music_file IS NOT NULL AND {a}.gamedata_torrent_index IS NOT NULL \
         AND (lower({a}.music_file) LIKE '%.mp3' OR lower({a}.music_file) LIKE '%.ogg')",
        a = alias
    )
}

/// Do two rows belong to the same multi-language group?
///
/// Shortcodes are unique per pack family, NOT globally: eXoWin3x reuses ten
/// eXoDOS codes for unrelated games ("EarthQue" is Earthquest under DOS and
/// Eyewitness Earth Quest under Win3x). Pairing on the shortcode alone would
/// merge those into one card and hide the other game from the catalogue.
pub fn same_group(a: &str, b: &str) -> String {
    format!(
        "{a}.shortcode = {b}.shortcode AND {fa} = {fb}",
        a = a,
        b = b,
        fa = family_expr(a),
        fb = family_expr(b)
    )
}

/// One grid row per game: rows sharing a shortcode (within one pack family)
/// are one multi-language group, represented by its primary row (EN preferred,
/// lowest id as tiebreak). Rows without a shortcode stand alone.
/// All consumers must alias the table as `g` (FROM games g).
fn primary_row_condition() -> String {
    format!(
        "(g.shortcode IS NULL OR g.id = (
        SELECT p.id FROM games p WHERE {}
        ORDER BY CASE WHEN p.language = 'EN' THEN 0 ELSE 1 END, p.id LIMIT 1))",
        same_group("p", "g")
    )
}

/// Build WHERE clause from filters. Filters are evaluated against EVERY
/// variant of a group (EXISTS subquery), so searching a localized title or
/// filtering an LP collection still surfaces the merged primary card - see
/// CLAUDE.md "Multi-language games are merged".
fn build_where_clause(f: &GameFilter) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut variant_conds = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if !f.query.is_empty() {
        params.push(Box::new(format!("%{}%", f.query)));
        variant_conds.push(format!("v.title LIKE ?{}", params.len()));
    }

    if !f.genre.is_empty() {
        // Genre is semicolon-separated, use LIKE for partial match
        params.push(Box::new(format!("%{}%", f.genre)));
        variant_conds.push(format!("v.genre LIKE ?{}", params.len()));
    }

    if !f.collection.is_empty() {
        params.push(Box::new(f.collection.to_string()));
        variant_conds.push(format!("v.torrent_source = ?{}", params.len()));
    }

    if f.favorites_only {
        variant_conds.push("v.favorited = 1".to_string());
    }

    let mut conditions = vec![primary_row_condition()];

    if f.with_music {
        // Its OWN EXISTS, not part of variant_conds: the hint sits on the EN
        // row (localized rows all carry a NULL gamedata_torrent_index) while
        // the collection filter matches the LP row, so requiring ONE variant
        // to satisfy both would empty the GLP/PLP/SLP shelves. "Some variant
        // of this card has a playable hint" is also the honest question -
        // playback resolves to the EN sibling's archive anyway
        // (media::resolve_gamedata).
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM games w \
             WHERE (w.id = g.id OR (g.shortcode IS NOT NULL AND {})) \
             AND {})",
            same_group("w", "g"),
            playable_music_sql("w")
        ));
    }

    if let Some(pid) = f.playlist_id {
        // Top-level condition, NOT part of the per-variant EXISTS: curated
        // membership sits almost entirely on EN rows while e.g. the
        // collection filter matches LP rows, so requiring one variant to
        // satisfy both would empty the grid (GLP + "Games with MT-32"
        // returned 3 of 218 groups). Driving from playlist_games also makes
        // the query O(members) instead of a full-catalog scan: each member
        // maps to its group's primary row id, and g must be one of them.
        params.push(Box::new(pid));
        conditions.push(format!(
            "g.id IN (SELECT CASE WHEN m.shortcode IS NULL THEN m.id ELSE (
                 SELECT p.id FROM games p WHERE {}
                 ORDER BY CASE WHEN p.language = 'EN' THEN 0 ELSE 1 END, p.id LIMIT 1) END
             FROM playlist_games pg JOIN games m ON m.id = pg.game_id
             WHERE pg.playlist_id = ?{})",
            same_group("p", "m"),
            params.len()
        ));
    }
    if !variant_conds.is_empty() {
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM games v \
             WHERE (v.id = g.id OR (g.shortcode IS NOT NULL AND {})) \
             AND {})",
            same_group("v", "g"),
            variant_conds.join(" AND ")
        ));
    }

    (format!(" WHERE {}", conditions.join(" AND ")), params)
}

// Alphabetical order MUST use the same expression the section keys and the
// frontend's groupKey() use (COALESCE(sort_title,title)): ordering by bare
// title while sectioning by sort_title scattered "The X"/"X 2"-style games
// into fake mid-alphabet sections, and the jump bar scrolled to those.
const TITLE_ORDER: &str = "COALESCE(sort_title, title) COLLATE NOCASE";

fn order_clause(sort_by: &str) -> String {
    match sort_by {
        "year_asc" => format!("ORDER BY COALESCE(year, 9999) ASC, {TITLE_ORDER} ASC"),
        "year_desc" => format!("ORDER BY COALESCE(year, 0) DESC, {TITLE_ORDER} ASC"),
        // Star bucket first (keeps the section labels monotonic), then vote
        // count: eXoDOS carries 185 games at a flat 5.0 from a single vote,
        // and raw-rating order put that wall of one-vote entries above every
        // widely-rated classic.
        "rating" => format!(
            "ORDER BY CAST(ROUND(COALESCE(rating, -1)) AS INTEGER) DESC, \
             COALESCE(rating_votes, 0) DESC, COALESCE(rating, -1) DESC, {TITLE_ORDER} ASC"
        ),
        "title_desc" => format!("ORDER BY {TITLE_ORDER} DESC"),
        "genre" => format!("ORDER BY COALESCE(genre, 'zzz') ASC, {TITLE_ORDER} ASC"),
        // List-view column sorts (#21). Unknowns last via a leading boolean
        // key, NOT a text sentinel: NOCASE folds ASCII only and compares
        // bytes, so multibyte names ("Åkesoft", "Ère Informatique" - both in
        // the catalogue) collate AFTER any 'zzzz' and ended up stranded
        // behind the unknown block.
        "developer" => format!(
            "ORDER BY (developer IS NULL OR developer = ''), developer COLLATE NOCASE ASC, {TITLE_ORDER} ASC"
        ),
        "developer_desc" => format!(
            "ORDER BY (developer IS NULL OR developer = ''), developer COLLATE NOCASE DESC, {TITLE_ORDER} ASC"
        ),
        "publisher" => format!(
            "ORDER BY (publisher IS NULL OR publisher = ''), publisher COLLATE NOCASE ASC, {TITLE_ORDER} ASC"
        ),
        "publisher_desc" => format!(
            "ORDER BY (publisher IS NULL OR publisher = ''), publisher COLLATE NOCASE DESC, {TITLE_ORDER} ASC"
        ),
        "size" => format!(
            "ORDER BY (download_size IS NULL), download_size ASC, {TITLE_ORDER} ASC"
        ),
        "size_desc" => format!(
            "ORDER BY (download_size IS NULL), download_size DESC, {TITLE_ORDER} ASC"
        ),
        _ => format!("ORDER BY {TITLE_ORDER} ASC"),
    }
}

/// Count total games with filters.
pub fn count_games(conn: &Connection, query: &str) -> DbResult<usize> {
    let f = GameFilter { query, genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
    count_games_filtered(conn, &f)
}

pub fn count_games_filtered(conn: &Connection, f: &GameFilter) -> DbResult<usize> {
    let (where_clause, params) = build_where_clause(f);
    let sql = format!("SELECT COUNT(*) FROM games g{}", where_clause);
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
        "SELECT {} FROM games g{} {} LIMIT ?{} OFFSET ?{}",
        GAME_COLUMNS, where_clause, order, limit_idx, offset_idx
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let mut games = stmt
        .query_map(rusqlite::params_from_iter(&params), row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;

    attach_language_maps(conn, &mut games)?;
    Ok(games)
}

/// Populate `available_languages` ("EN:0,DE:2" - state 0=available,
/// 1=in_library, 2=installed; EN first, then alphabetical) for every game
/// whose shortcode has more than one language variant. Single-variant games
/// keep None so the frontend renders no badge.
fn attach_language_maps(conn: &Connection, games: &mut [Game]) -> DbResult<()> {
    let shortcodes: Vec<&str> = games
        .iter()
        .filter_map(|g| g.shortcode.as_deref())
        .collect();
    if shortcodes.is_empty() {
        return Ok(());
    }

    let placeholders: Vec<String> = (1..=shortcodes.len()).map(|i| format!("?{}", i)).collect();
    // Grouped by (family, shortcode), not shortcode alone - see `same_group`.
    let sql = format!(
        "SELECT {}, g.shortcode, g.language, g.installed, g.in_library, g.title \
         FROM games g WHERE g.shortcode IN ({}) \
         ORDER BY CASE WHEN g.language = 'EN' THEN 0 ELSE 1 END, g.language",
        family_expr("g"),
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    type GroupKey = (String, String);
    let mut map: std::collections::HashMap<GroupKey, Vec<String>> = std::collections::HashMap::new();
    // Localized titles of the group, so a client-side search over already
    // loaded rows (My Library) can match "Zauberteppich" the way the Browse
    // SQL filter does. Only multi-variant groups get one attached.
    let mut titles: std::collections::HashMap<GroupKey, Vec<String>> = std::collections::HashMap::new();
    let rows = stmt.query_map(rusqlite::params_from_iter(shortcodes.iter()), |row| {
        let key: GroupKey = (row.get(0)?, row.get(1)?);
        let lang: Option<String> = row.get(2)?;
        let installed: i32 = row.get::<_, i32>(3).unwrap_or(0);
        let in_library: i32 = row.get::<_, i32>(4).unwrap_or(0);
        let title: String = row.get::<_, Option<String>>(5)?.unwrap_or_default();
        Ok((key, lang.unwrap_or_else(|| "EN".to_string()), installed, in_library, title))
    })?;
    for row in rows {
        let (key, lang, installed, in_library, title) = row?;
        let state = if installed != 0 { 2 } else if in_library != 0 { 1 } else { 0 };
        map.entry(key.clone()).or_default().push(format!("{}:{}", lang, state));
        if !title.is_empty() {
            titles.entry(key).or_default().push(title);
        }
    }

    for game in games.iter_mut() {
        if let Some(sc) = game.shortcode.as_deref() {
            let base = crate::collection_base_id(game.torrent_source.as_deref().unwrap_or("eXoDOS"));
            let key: GroupKey = (base.to_string(), sc.to_string());
            if let Some(entries) = map.get(&key) {
                if entries.len() > 1 {
                    game.available_languages = Some(entries.join(","));
                    if let Some(group_titles) = titles.get(&key) {
                        let others: Vec<&str> = group_titles
                            .iter()
                            .map(|t| t.as_str())
                            .filter(|t| *t != game.title)
                            .collect();
                        if !others.is_empty() {
                            game.variant_titles = Some(others.join("\u{1f}"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}


/// Get all language variants for a shortcode within `collection`'s pack family.
/// The family is required, not optional: a shortcode alone can name a different
/// game in another pack - see `same_group`.
pub fn fetch_game_variants(
    conn: &Connection,
    shortcode: &str,
    collection: &str,
) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games g WHERE g.shortcode = ?1 AND {} = ?2 \
         ORDER BY CASE g.language WHEN 'EN' THEN 0 ELSE 1 END, g.language",
        GAME_COLUMNS,
        family_expr("g")
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut games = stmt
        .query_map(params![shortcode, crate::collection_base_id(collection)], row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;

    // LP overlay ZIPs (< 1 MB) are just localized bat files - they require the EN base game
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
        "year_asc"  => ("COALESCE(CAST(year AS TEXT),'Unknown')", "key ASC"),
        "year_desc" => ("COALESCE(CAST(year AS TEXT),'Unknown')", "key DESC"),
        "genre"     => ("COALESCE(genre,'Unknown')",               "key ASC"),
        "rating"    => ("CAST(ROUND(COALESCE(rating,-1)) AS INTEGER)", "key DESC"),
        _           => return Ok(vec![]),
    };

    let sql = format!(
        "SELECT DISTINCT {select} as key FROM games g {where_clause} ORDER BY {order}",
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

    if f.sort_by == "genre" {
        // The `genre` column stores semicolon-joined values like
        // "Action;Adventure;RPG", and individual entries can contain
        // " / "-delimited parent/child like "Sports / Baseball". For the
        // jumpbar we collapse to just the parent so users see ~15 top-level
        // categories (matches the parent rows in the genre filter dropdown)
        // instead of dozens of subgenre permutations.
        // IMPORTANT: use only the FIRST entry's parent - Library.tsx's
        // sectionKey() does the same, and a key derived from a later entry
        // would appear in the jumpbar with no section to scroll to.
        let mut seen = std::collections::BTreeSet::new();
        for entry in raw {
            let first = entry.split(';').next().unwrap_or("");
            let parent = first.split(" / ").next().unwrap_or(first).trim();
            if !parent.is_empty() {
                seen.insert(parent.to_string());
            }
        }
        return Ok(seen.into_iter().collect());
    }

    Ok(raw)
}

/// Fetch installed games - one card per game: variants sharing a shortcode
/// collapse to a single row (EN preferred AMONG INSTALLED variants, so a
/// DE-only install shows its playable DE row, not the uninstalled EN one).
/// Language badges on the card carry the per-variant states.
pub fn fetch_installed_games(conn: &Connection) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games g WHERE g.installed = 1 AND (g.shortcode IS NULL OR g.id = (
            SELECT p.id FROM games p WHERE {} AND p.installed = 1
            ORDER BY CASE WHEN p.language = 'EN' THEN 0 ELSE 1 END, p.id LIMIT 1))
         ORDER BY title, language",
        GAME_COLUMNS,
        same_group("p", "g")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut games: Vec<Game> = stmt
        .query_map([], row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;
    attach_language_maps(conn, &mut games)?;
    Ok(games)
}

/// Fetch recently played games, ordered by last_played descending. One card
/// per game: when several variants of a shortcode were played, only the most
/// recently played row represents the group.
pub fn fetch_recently_played(conn: &Connection, limit: usize) -> DbResult<Vec<Game>> {
    let sql = format!(
        "SELECT {} FROM games g WHERE g.last_played IS NOT NULL AND (g.shortcode IS NULL OR g.id = (
            SELECT p.id FROM games p WHERE {} AND p.last_played IS NOT NULL
            ORDER BY p.last_played DESC, p.id LIMIT 1))
         ORDER BY last_played DESC LIMIT ?1",
        GAME_COLUMNS,
        same_group("p", "g")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut games: Vec<Game> = stmt
        .query_map(params![limit as i64], row_to_game)?
        .collect::<Result<Vec<_>, _>>()?;
    attach_language_maps(conn, &mut games)?;
    Ok(games)
}

/// Set the last_played timestamp for a game to the current time.
pub fn set_last_played(conn: &Connection, id: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE games SET last_played = datetime('now') WHERE id = ?1",
        params![id],
    )?;
    Ok(())
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

// ── Per-game config (game_config table) ─────────────────────────────────────

pub fn set_game_config(conn: &Connection, game_id: i64, key: &str, value: &str) -> DbResult<()> {
    conn.execute(
        "INSERT INTO game_config (game_id, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(game_id, key) DO UPDATE SET value = excluded.value",
        params![game_id, key, value],
    )?;
    Ok(())
}

pub fn delete_game_config(conn: &Connection, game_id: i64, key: &str) -> DbResult<()> {
    conn.execute(
        "DELETE FROM game_config WHERE game_id = ?1 AND key = ?2",
        params![game_id, key],
    )?;
    Ok(())
}

pub fn get_all_game_config(
    conn: &Connection,
    game_id: i64,
) -> DbResult<std::collections::HashMap<String, String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT key, value FROM game_config WHERE game_id = ?1",
    )?;
    let map = stmt
        .query_map(params![game_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(map)
}

// ── Playlists ──────────────────────────────────────────────────────────
//
// Two kinds share the same tables: kind='curated' rows ship inside the
// bundled catalog DB (seeded by generate_db, re-synced by refresh_catalog)
// and are read-only; kind='user' rows are created in-app and never touched
// by catalog updates.

/// All playlists with their visible-card count.
///
/// One grid card per shortcode group, so the count is the number of
/// DISTINCT groups among the playlist's members - equivalent to counting
/// primary rows that pass the Browse EXISTS-over-variants filter, but
/// O(members) instead of O(all games x playlists). The naive primary-row
/// formulation took ~4s over the full catalog and, called from the create
/// flow while the 5s shelf polling queued behind the same DB mutex,
/// stretched "Saving..." into the tens of seconds.
/// ('#' can't appear in a shortcode, so the id fallback key can't collide.)
pub fn fetch_playlists(conn: &Connection) -> DbResult<Vec<crate::models::Playlist>> {
    let sql = "SELECT pl.id, pl.name, pl.kind, pl.description,
            (SELECT COUNT(DISTINCT COALESCE(m.shortcode, 'id#' || m.id))
             FROM playlist_games pg
             JOIN games m ON m.id = pg.game_id
             WHERE pg.playlist_id = pl.id)
         FROM playlists pl
         ORDER BY CASE pl.kind WHEN 'user' THEN 0 ELSE 1 END, pl.name";
    let mut stmt = conn.prepare_cached(sql)?;
    let playlists = stmt
        .query_map([], |row| {
            Ok(crate::models::Playlist {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                description: row.get(3)?,
                game_count: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(playlists)
}

/// Create a user playlist. Fails on duplicate name (UNIQUE constraint).
pub fn create_playlist(conn: &Connection, name: &str) -> DbResult<i64> {
    conn.execute(
        "INSERT INTO playlists (name, kind) VALUES (?1, 'user')",
        params![name],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Guard shared by rename/delete: curated playlists are catalog content.
fn ensure_user_playlist(conn: &Connection, id: i64) -> DbResult<()> {
    let kind: String = conn.query_row(
        "SELECT kind FROM playlists WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    if kind != "user" {
        return Err(super::DbError::ReadOnlyPlaylist);
    }
    Ok(())
}

pub fn rename_playlist(conn: &Connection, id: i64, name: &str) -> DbResult<()> {
    ensure_user_playlist(conn, id)?;
    conn.execute(
        "UPDATE playlists SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    Ok(())
}

pub fn delete_playlist(conn: &Connection, id: i64) -> DbResult<()> {
    ensure_user_playlist(conn, id)?;
    conn.execute("DELETE FROM playlists WHERE id = ?1", params![id])?;
    Ok(())
}

/// Add or remove a game from a user playlist. Adding is idempotent;
/// position is append-order and only informational for now.
pub fn set_playlist_membership(
    conn: &Connection,
    playlist_id: i64,
    game_id: i64,
    member: bool,
) -> DbResult<()> {
    ensure_user_playlist(conn, playlist_id)?;
    if member {
        conn.execute(
            "INSERT OR IGNORE INTO playlist_games (playlist_id, game_id, position)
             VALUES (?1, ?2,
                (SELECT COALESCE(MAX(position), 0) + 1 FROM playlist_games
                 WHERE playlist_id = ?1))",
            params![playlist_id, game_id],
        )?;
    } else {
        // Group-wide, mirroring fetch_game_playlist_ids: the membership row
        // may sit on a sibling variant (added from a shelf that rendered the
        // installed LP row), and an exact-id DELETE would silently no-op
        // while the checkmark keeps coming back.
        conn.execute(
            &format!(
                "DELETE FROM playlist_games WHERE playlist_id = ?1 AND game_id IN (
                SELECT v.id FROM games v
                JOIN games me ON me.id = ?2
                WHERE v.id = me.id
                   OR (me.shortcode IS NOT NULL AND {}))",
                same_group("v", "me")
            ),
            params![playlist_id, game_id],
        )?;
    }
    Ok(())
}

/// Playlist ids a game belongs to (any variant of its shortcode group, so
/// the check works no matter which variant row the caller holds).
pub fn fetch_game_playlist_ids(conn: &Connection, game_id: i64) -> DbResult<Vec<i64>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT DISTINCT pg.playlist_id FROM playlist_games pg
         WHERE pg.game_id = ?1
            OR pg.game_id IN (
                SELECT v.id FROM games v
                JOIN games me ON me.id = ?1
                WHERE me.shortcode IS NOT NULL AND {})",
        same_group("v", "me")
    ))?;
    let ids = stmt
        .query_map(params![game_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
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
            rating_votes: None,
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
            variant_titles: None,
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
            manual_path: None,
            last_played: None,
            music_file: None,
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

    /// My Library filters already-loaded rows client-side, so the localized
    /// titles have to travel with the merged row - otherwise searching the
    /// German name works in Browse (SQL, across variants) and silently fails
    /// on the library tab.
    #[test]
    fn merged_rows_carry_their_variant_titles() {
        let conn = open_test_db();
        let mut en = make_game("The 11th Hour");
        en.shortcode = Some("11thHour".to_string());
        let mut de = make_game("Die 11te Stunde");
        de.language = "DE".to_string();
        de.shortcode = Some("11thHour".to_string());
        let mut es = make_game("La Undecima Hora");
        es.language = "ES".to_string();
        es.shortcode = Some("11thHour".to_string());
        let solo = make_game("Bloxit");
        insert_games(&conn, &[en, de, es, solo]).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        let games = fetch_games_filtered(&conn, 1, 50, &f).unwrap();

        let merged = games.iter().find(|g| g.shortcode.as_deref() == Some("11thHour")).unwrap();
        let titles = merged.variant_titles.as_deref().expect("merged row needs variant titles");
        let parts: Vec<&str> = titles.split('\u{1f}').collect();
        assert!(parts.contains(&"Die 11te Stunde"), "got {:?}", parts);
        assert!(parts.contains(&"La Undecima Hora"), "got {:?}", parts);
        assert!(!parts.contains(&"The 11th Hour"), "own title must not be repeated: {:?}", parts);

        // A game with no siblings carries nothing - the field is a multi-language
        // marker as much as a search aid.
        let solo = games.iter().find(|g| g.title == "Bloxit").unwrap();
        assert_eq!(solo.variant_titles, None);
    }

    /// The list order and the section keys must derive from the SAME
    /// expression (COALESCE(sort_title, title)). Ordering by bare title while
    /// sectioning by sort_title scattered "The X"-style games into fake
    /// mid-alphabet sections and the jump bar scrolled to those.
    #[test]
    fn title_sort_follows_sort_title() {
        let conn = open_test_db();
        let mut the_aardvark = make_game("The Aardvark");
        the_aardvark.sort_title = Some("Aardvark, The".to_string());
        let beta = make_game("Beta");
        insert_games(&conn, &[beta, the_aardvark]).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "title", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        let games = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        // sort_title "Aardvark, The" files it under A, before Beta - title
        // order would put it under T, after.
        assert_eq!(titles, vec!["The Aardvark", "Beta"]);
    }

    /// "Top rated" orders by vote count inside a star bucket: eXoDOS carries
    /// 185 games at a flat 5.0 from a single vote, and raw-rating order put
    /// that wall above every widely-rated classic.
    #[test]
    fn rating_sort_prefers_vote_count_inside_a_star_bucket() {
        let conn = open_test_db();
        let mut one_vote_five = make_game("Obscurity");
        one_vote_five.rating = Some(5.0);
        one_vote_five.rating_votes = Some(1);
        let mut classic = make_game("DOOM");
        classic.rating = Some(4.62); // rounds into the same 5-star bucket
        classic.rating_votes = Some(147);
        let mut lower_bucket = make_game("Solid");
        lower_bucket.rating = Some(4.4); // 4-star bucket, must stay below both
        lower_bucket.rating_votes = Some(500);
        insert_games(&conn, &[one_vote_five, classic, lower_bucket]).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "rating", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        let games = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, vec!["DOOM", "Obscurity", "Solid"]);
    }

    /// eXoWin3x reuses ten eXoDOS shortcodes for entirely different games
    /// ("EarthQue" is Earthquest under DOS, Eyewitness Earth Quest under
    /// Win3x). They must stay two cards - merging them hides one game.
    #[test]
    fn same_shortcode_in_another_pack_is_not_a_variant() {
        let conn = open_test_db();
        let mut dos = make_game("Earthquest");
        dos.shortcode = Some("EarthQue".to_string());
        let mut win3x = make_game("Eyewitness Virtual Reality: Earth Quest");
        win3x.shortcode = Some("EarthQue".to_string());
        insert_games(&conn, &[dos, win3x]).unwrap();
        conn.execute(
            "UPDATE games SET torrent_source = 'eXoWin3x' WHERE title LIKE 'Eyewitness%'", [],
        ).unwrap();
        conn.execute(
            "UPDATE games SET torrent_source = 'eXoDOS' WHERE title = 'Earthquest'", [],
        ).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        let games = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(games.len(), 2, "got {:?}", games.iter().map(|g| &g.title).collect::<Vec<_>>());
        // Neither may claim the other as a language variant.
        assert!(games.iter().all(|g| g.available_languages.is_none()));

        // Variant lookup is scoped the same way.
        let dos_variants = fetch_game_variants(&conn, "EarthQue", "eXoDOS").unwrap();
        assert_eq!(dos_variants.len(), 1);
        assert_eq!(dos_variants[0].title, "Earthquest");
        let w3x_variants = fetch_game_variants(&conn, "EarthQue", "eXoWin3x").unwrap();
        assert_eq!(w3x_variants.len(), 1);
        assert_eq!(w3x_variants[0].title, "Eyewitness Virtual Reality: Earth Quest");
    }

    /// The list view's column sorts (#21): games without a developer/size
    /// sort last in BOTH directions instead of clumping at the top - and a
    /// multibyte name must not land behind the unknown block (NOCASE compares
    /// bytes, so a text sentinel like 'zzzz' put "Åkesoft" after it).
    #[test]
    fn column_sorts_put_unknown_values_last() {
        let conn = open_test_db();
        let mut a = make_game("Alpha");
        a.developer = Some("Sierra".to_string());
        let b = make_game("Beta");
        let mut c = make_game("Gamma");
        c.developer = Some("id Software".to_string());
        let mut d = make_game("Delta");
        d.developer = Some("Åkesoft".to_string());
        insert_games(&conn, &[a, b, c, d]).unwrap();
        // download_size is not part of insert_games - it arrives via the
        // torrent-matching UPDATE, so the test writes it the same way.
        conn.execute("UPDATE games SET download_size = 500 WHERE title = 'Alpha'", []).unwrap();
        conn.execute("UPDATE games SET download_size = 2000 WHERE title = 'Gamma'", []).unwrap();

        let fetch = |sort_by: &str| {
            let f = GameFilter { query: "", genre: "", sort_by, collection: "", favorites_only: false, playlist_id: None, with_music: false };
            fetch_games_filtered(&conn, 1, 50, &f)
                .unwrap()
                .into_iter()
                .map(|g| g.title)
                .collect::<Vec<_>>()
        };

        assert_eq!(fetch("developer"), vec!["Gamma", "Alpha", "Delta", "Beta"]);
        assert_eq!(fetch("developer_desc"), vec!["Delta", "Alpha", "Gamma", "Beta"]);
        assert_eq!(fetch("size"), vec!["Alpha", "Gamma", "Beta", "Delta"]);
        assert_eq!(fetch("size_desc"), vec!["Gamma", "Alpha", "Beta", "Delta"]);
    }

    #[test]
    fn merged_grid_one_card_per_shortcode() {
        let conn = open_test_db();
        let mut en = make_game("The 11th Hour");
        en.shortcode = Some("11thHour".to_string());
        let mut de = make_game("Die 11te Stunde");
        de.language = "DE".to_string();
        de.shortcode = Some("11thHour".to_string());
        let solo = make_game("Bloxit");
        insert_games(&conn, &[en, de, solo]).unwrap();
        conn.execute(
            "UPDATE games SET installed = 1, torrent_source = 'eXoDOS_GLP' WHERE language = 'DE'", [],
        ).unwrap();
        conn.execute(
            "UPDATE games SET torrent_source = 'eXoDOS' WHERE language = 'EN'", [],
        ).unwrap();

        // Grid: one merged card (EN primary) + one standalone = 2, not 3.
        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        assert_eq!(count_games_filtered(&conn, &f).unwrap(), 2);
        let games = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(games.len(), 2);
        let merged = games.iter().find(|g| g.shortcode.as_deref() == Some("11thHour")).unwrap();
        assert_eq!(merged.language, "EN");
        assert_eq!(merged.available_languages.as_deref(), Some("EN:0,DE:2"));
        let solo = games.iter().find(|g| g.title == "Bloxit").unwrap();
        assert_eq!(solo.available_languages, None);

        // Searching the localized title surfaces the merged EN primary.
        let f = GameFilter { query: "11te Stunde", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        let hits = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].language, "EN");

        // Filtering by the LP collection also surfaces the EN primary.
        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "eXoDOS_GLP", favorites_only: false, playlist_id: None, with_music: false };
        let hits = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].language, "EN");
        assert_eq!(count_games_filtered(&conn, &f).unwrap(), 1);
    }

    #[test]
    fn installed_shelf_merges_variants() {
        let conn = open_test_db();
        let mut en = make_game("The 11th Hour");
        en.shortcode = Some("11thHour".to_string());
        let mut de = make_game("Die 11te Stunde");
        de.language = "DE".to_string();
        de.shortcode = Some("11thHour".to_string());
        insert_games(&conn, &[en, de]).unwrap();

        // Only DE installed: the shelf must show the playable DE row, with
        // badges telling the full story (EN available, DE installed).
        conn.execute("UPDATE games SET installed = 1 WHERE language = 'DE'", []).unwrap();
        let shelf = fetch_installed_games(&conn).unwrap();
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf[0].language, "DE");
        assert_eq!(shelf[0].available_languages.as_deref(), Some("EN:0,DE:2"));

        // Only one variant played: it represents the group on the shelf.
        conn.execute(
            "UPDATE games SET last_played = '2026-07-29 09:00:00' WHERE language = 'DE'", [],
        ).unwrap();
        let recent = fetch_recently_played(&conn, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].language, "DE");
        conn.execute("UPDATE games SET last_played = NULL", []).unwrap();

        // Both installed: one card, EN preferred, badges show both as installed.
        conn.execute("UPDATE games SET installed = 1", []).unwrap();
        let shelf = fetch_installed_games(&conn).unwrap();
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf[0].language, "EN");
        assert_eq!(shelf[0].available_languages.as_deref(), Some("EN:2,DE:2"));

        // Recently played: both variants played -> one card, most recent wins.
        conn.execute(
            "UPDATE games SET last_played = CASE language WHEN 'EN' THEN '2026-07-29 10:00:00' ELSE '2026-07-29 11:00:00' END",
            [],
        ).unwrap();
        let recent = fetch_recently_played(&conn, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].language, "DE");
    }

    #[test]
    fn search_by_query() {
        let conn = open_test_db();
        insert_games(&conn, &[
            make_game("Space Quest V"),
            make_game("Space Quest IV"),
            make_game("Doom"),
        ]).unwrap();

        let f = GameFilter { query: "Space", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
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

        let f = GameFilter { query: "", genre: "Role-Playing", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Baldur's Gate");
    }

    #[test]
    fn filter_by_collection() {
        let conn = open_test_db();
        insert_games(&conn, &[make_game("Doom"), make_game("Doom DE")]).unwrap();

        // torrent_source is set post-import by the torrent matching phase,
        // not by insert_games - update it directly here.
        conn.execute("UPDATE games SET torrent_source = 'eXoDOS' WHERE title = 'Doom'", []).unwrap();
        conn.execute("UPDATE games SET torrent_source = 'eXoDOS_GLP' WHERE title = 'Doom DE'", []).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "eXoDOS_GLP", favorites_only: false, playlist_id: None, with_music: false };
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

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: true, playlist_id: None, with_music: false };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Doom");
    }

    /// The hint has to name a format the webview plays AND an archive to read
    /// it out of - a tracker module or a row without a GameData index is not a
    /// playable theme.
    #[test]
    fn filter_with_music_keeps_playable_hints_only() {
        let conn = open_test_db();
        insert_games(&conn, &[
            make_game("Has MP3"),
            make_game("Has Module"),
            make_game("Has Nothing"),
            make_game("No Archive"),
        ]).unwrap();
        // music_file and gamedata_torrent_index are written post-import (XML
        // hint + torrent matching), not by insert_games - set them directly.
        conn.execute(
            "UPDATE games SET music_file = 'Music/MS-DOS/X.mp3', gamedata_torrent_index = 1 WHERE title = 'Has MP3'", [],
        ).unwrap();
        conn.execute(
            "UPDATE games SET music_file = 'Music/MS-DOS/X.XM', gamedata_torrent_index = 2 WHERE title = 'Has Module'", [],
        ).unwrap();
        conn.execute(
            "UPDATE games SET gamedata_torrent_index = 3 WHERE title = 'Has Nothing'", [],
        ).unwrap();
        conn.execute(
            "UPDATE games SET music_file = 'Music/MS-DOS/X.mp3' WHERE title = 'No Archive'", [],
        ).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: true };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(
            results.iter().map(|g| g.title.as_str()).collect::<Vec<_>>(),
            vec!["Has MP3"]
        );
        assert_eq!(count_games_filtered(&conn, &f).unwrap(), 1);
    }

    /// Localized rows all carry a NULL gamedata index, so the hint lives on the
    /// EN row only - the filter must still surface the merged card.
    #[test]
    fn filter_with_music_surfaces_the_merged_en_card() {
        let conn = open_test_db();
        let mut en = make_game("The 11th Hour");
        en.shortcode = Some("11thHour".to_string());
        let mut de = make_game("Die 11te Stunde");
        de.language = "DE".to_string();
        de.shortcode = Some("11thHour".to_string());
        insert_games(&conn, &[en, de, make_game("Bloxit")]).unwrap();
        conn.execute(
            "UPDATE games SET torrent_source = CASE language WHEN 'EN' THEN 'eXoDOS' ELSE 'eXoDOS_GLP' END",
            [],
        ).unwrap();
        conn.execute(
            "UPDATE games SET music_file = 'Music/MS-DOS/X.ogg', gamedata_torrent_index = 7 \
             WHERE title = 'The 11th Hour'", [],
        ).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: true };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].language, "EN");
        assert_eq!(results[0].title, "The 11th Hour");
        assert_eq!(results[0].available_languages.as_deref(), Some("EN:0,DE:0"));
        assert_eq!(count_games_filtered(&conn, &f).unwrap(), 1);

        // The collection shelf is always active, so this is the common case,
        // not a corner: the GLP filter matches the DE row and the hint sits on
        // the EN one. Both are group-wide questions, so the card still shows.
        let f_glp = GameFilter { query: "", genre: "", sort_by: "", collection: "eXoDOS_GLP", favorites_only: false, playlist_id: None, with_music: true };
        let hits = fetch_games_filtered(&conn, 1, 50, &f_glp).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].language, "EN");
        assert_eq!(hits[0].title, "The 11th Hour");
        assert_eq!(count_games_filtered(&conn, &f_glp).unwrap(), 1);
    }

    #[test]
    fn playlist_filter_merges_variants_and_counts_cards() {
        let conn = open_test_db();
        let mut en = make_game("The 11th Hour");
        en.shortcode = Some("11thHour".to_string());
        let mut de = make_game("Die 11te Stunde");
        de.language = "DE".to_string();
        de.shortcode = Some("11thHour".to_string());
        insert_games(&conn, &[en, de, make_game("Doom")]).unwrap();

        let pid = create_playlist(&conn, "Backlog").unwrap();
        // Membership on the DE variant row: the merged EN card must still
        // surface (EXISTS-over-variants), and the count must be 1 card.
        let de_id: i64 = conn
            .query_row("SELECT id FROM games WHERE language = 'DE'", [], |r| r.get(0))
            .unwrap();
        set_playlist_membership(&conn, pid, de_id, true).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: Some(pid), with_music: false };
        let results = fetch_games_filtered(&conn, 1, 50, &f).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].language, "EN");

        let playlists = fetch_playlists(&conn).unwrap();
        let backlog = playlists.iter().find(|p| p.name == "Backlog").unwrap();
        assert_eq!(backlog.kind, "user");
        assert_eq!(backlog.game_count, 1);

        // Membership lookup works from any variant of the group.
        let en_id: i64 = conn
            .query_row("SELECT id FROM games WHERE language = 'EN' AND shortcode = '11thHour'", [], |r| r.get(0))
            .unwrap();

        // Both variants of the group in the playlist -> still ONE card.
        set_playlist_membership(&conn, pid, en_id, true).unwrap();
        let playlists = fetch_playlists(&conn).unwrap();
        assert_eq!(playlists.iter().find(|p| p.name == "Backlog").unwrap().game_count, 1);
        assert_eq!(count_games_filtered(&conn, &f).unwrap(), 1);

        // Removal is GROUP-wide, mirroring the lookup: the membership rows
        // sit on both variants, removing via the EN id must clear the DE
        // row too - otherwise a checkmark unchecked from the merged card
        // silently comes back.
        set_playlist_membership(&conn, pid, en_id, false).unwrap();
        assert_eq!(fetch_game_playlist_ids(&conn, en_id).unwrap(), Vec::<i64>::new());
        assert_eq!(fetch_game_playlist_ids(&conn, de_id).unwrap(), Vec::<i64>::new());
        assert_eq!(count_games_filtered(&conn, &f).unwrap(), 0);

        // Playlist composes with the collection filter per GROUP, not per
        // single variant: membership on the EN row, GLP filter matching the
        // DE row - the merged card must still surface.
        set_playlist_membership(&conn, pid, en_id, true).unwrap();
        conn.execute(
            "UPDATE games SET torrent_source = CASE language WHEN 'EN' THEN 'eXoDOS' ELSE 'eXoDOS_GLP' END",
            [],
        ).unwrap();
        let f_glp = GameFilter { query: "", genre: "", sort_by: "", collection: "eXoDOS_GLP", favorites_only: false, playlist_id: Some(pid), with_music: false };
        let hits = fetch_games_filtered(&conn, 1, 50, &f_glp).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].language, "EN");
    }

    #[test]
    fn curated_playlists_are_read_only() {
        let conn = open_test_db();
        insert_games(&conn, &[make_game("Doom")]).unwrap();
        let game_id: i64 = conn.query_row("SELECT id FROM games", [], |r| r.get(0)).unwrap();
        conn.execute(
            "INSERT INTO playlists (name, kind, slug) VALUES ('MT-32', 'curated', 'mt-32')",
            [],
        )
        .unwrap();
        let pid: i64 = conn.query_row("SELECT id FROM playlists", [], |r| r.get(0)).unwrap();

        assert!(rename_playlist(&conn, pid, "Hijacked").is_err());
        assert!(delete_playlist(&conn, pid).is_err());
        assert!(set_playlist_membership(&conn, pid, game_id, true).is_err());

        // User playlists: full CRUD, duplicate names rejected.
        let upid = create_playlist(&conn, "Mine").unwrap();
        assert!(create_playlist(&conn, "Mine").is_err());
        rename_playlist(&conn, upid, "Renamed").unwrap();
        delete_playlist(&conn, upid).unwrap();
        assert_eq!(fetch_playlists(&conn).unwrap().len(), 1);
    }

    #[test]
    fn pagination() {
        let conn = open_test_db();
        let games: Vec<Game> = (1..=10).map(|i| make_game(&format!("Game {:02}", i))).collect();
        insert_games(&conn, &games).unwrap();

        let f = GameFilter { query: "", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
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

        let f = GameFilter { query: "a", genre: "", sort_by: "", collection: "", favorites_only: false, playlist_id: None, with_music: false };
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
