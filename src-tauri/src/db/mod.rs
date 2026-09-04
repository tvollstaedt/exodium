pub mod queries;
pub mod schema;

use rusqlite::Connection;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Database not found at {0}")]
    NotFound(String),
    #[error("Curated playlists cannot be modified")]
    ReadOnlyPlaylist,
}

pub type DbResult<T> = Result<T, DbError>;

/// Version of the bundled game catalog. Bump whenever the bundled
/// exodium.db ships materially different catalog data (new eXoDOS torrent,
/// corrected torrent indices, ...). At startup, an installed DB whose
/// `catalog_version` config is older gets its catalog rows refreshed from
/// the bundled DB via `refresh_catalog` - user state is preserved.
/// History: 1 = pre-versioning (0.6.x), 2 = path-anchored torrent indices,
/// 3 = curated playlists shipped in the bundled DB, 4 = eXoWin3x,
/// 5 = case-insensitive torrent matching (recovers games whose bat and zip
/// disagree in case, e.g. "I Can be a Dinosaur Finder"), 6 = eXoWin9x,
/// 7 = rating_votes ("Top rated" orders by vote count inside a star bucket),
/// 8 = pack sentinels dropped, family-scoped LP shortcodes/keys, normalized
/// language codes, 9 = LP rows linked to their EN game by canonical title
/// (they inherit its dosbox.conf and could not launch without it),
/// 10 = Spanish/Polish rows carry eXo's own directory code and config path,
/// which also merges them into their English game's card, 11 = music_file
/// hint (theme track name from LaunchBox MusicPath/MissingMusic).
pub const CATALOG_VERSION: i64 = 11;

/// Open (or create) the Exodium database at the given path.
pub fn open(path: &Path) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    // busy_timeout: side connections (import, extraction watcher, uninstall)
    // write concurrently with the main DbState connection; without a timeout
    // rusqlite surfaces SQLITE_BUSY instantly instead of waiting.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    Ok(conn)
}

/// Initialize the database schema (idempotent).
pub fn init(conn: &Connection) -> DbResult<()> {
    schema::create_tables(conn)?;
    migrate(conn)?;
    Ok(())
}

/// Column names of a table, in declaration order. Accepts a
/// schema-qualified name ("cat.games") - PRAGMA requires the schema on the
/// pragma itself, not in the argument.
fn table_columns(conn: &Connection, table: &str) -> DbResult<Vec<String>> {
    let pragma = match table.split_once('.') {
        Some((schema, name)) => format!("PRAGMA {}.table_info({})", schema, name),
        None => format!("PRAGMA table_info({})", table),
    };
    let mut stmt = conn.prepare(&pragma)?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cols)
}

/// Refresh catalog data from a newer bundled DB while preserving user state.
///
/// Rows are matched on `application_path` (unique in practice; the few rows
/// with an empty path fall back to title+language). Matched rows are updated
/// in place so `games.id` stays stable - `game_config` and `downloads` FKs
/// remain valid. Rows new in the bundled catalog are inserted with clean
/// user state; installed rows that vanished from the catalog are kept and
/// logged (their torrent indices may be stale).
///
/// Returns (updated, inserted).
pub fn refresh_catalog(conn: &mut Connection, bundled_db: &Path) -> DbResult<(usize, usize)> {
    conn.execute(
        "ATTACH DATABASE ?1 AS cat",
        [bundled_db.to_string_lossy().as_ref()],
    )?;

    let result = (|| {
        // Catalog columns = intersection of both schemas, minus the id and
        // the per-user columns. Computed dynamically so future ALTER TABLE
        // additions are picked up without touching this function.
        const USER_COLS: [&str; 5] = ["id", "in_library", "installed", "favorited", "last_played"];
        let installed_cols = table_columns(conn, "games")?;
        let cat_cols = table_columns(conn, "cat.games")?;
        let catalog_cols: Vec<&str> = installed_cols
            .iter()
            .filter(|c| cat_cols.contains(c) && !USER_COLS.contains(&c.as_str()))
            .map(|c| c.as_str())
            .collect();

        // The rows-match rule: by application_path when the catalog row has
        // one, by title+language otherwise. IMPORTANT: this must stay split
        // into separate equality-joined statements - a single OR'd predicate
        // defeats the query planner and turns each statement into a 9k x 9k
        // nested scan (~70 s of frozen startup, measured).
        let tx = conn.unchecked_transaction()?;

        // Equality joins need an index to be cheap; the games table has none
        // on application_path.
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_games_application_path ON games(application_path)",
        )?;

        let set_list = catalog_cols
            .iter()
            .map(|c| format!("{c} = c.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut updated = tx.execute(
            &format!(
                "UPDATE games AS g SET {set_list} FROM cat.games AS c \
                 WHERE c.application_path IS NOT NULL AND c.application_path != '' \
                   AND g.application_path = c.application_path"
            ),
            [],
        )?;
        updated += tx.execute(
            &format!(
                "UPDATE games AS g SET {set_list} FROM cat.games AS c \
                 WHERE (c.application_path IS NULL OR c.application_path = '') \
                   AND g.title = c.title AND g.language = c.language"
            ),
            [],
        )?;

        let col_list = catalog_cols.join(", ");
        let sel_list = catalog_cols
            .iter()
            .map(|c| format!("c.{c}"))
            .collect::<Vec<_>>()
            .join(", ");
        // NOT IN (SELECT ...) materializes into an ephemeral index - one
        // build over ~9k rows, then O(1) probes per catalog row.
        let inserted = tx.execute(
            &format!(
                "INSERT INTO games ({col_list}) \
                 SELECT {sel_list} FROM cat.games AS c \
                 WHERE (c.application_path IS NOT NULL AND c.application_path != '' \
                        AND c.application_path NOT IN \
                            (SELECT application_path FROM games \
                             WHERE application_path IS NOT NULL AND application_path != '')) \
                    OR ((c.application_path IS NULL OR c.application_path = '') \
                        AND NOT EXISTS (SELECT 1 FROM games g \
                                        WHERE g.title = c.title AND g.language = c.language))"
            ),
            [],
        )?;

        // Rows the catalog no longer contains are kept (with possibly stale
        // torrent indices); updated counts each matched row once, so the
        // difference is the stale set. Log-only.
        let total: i64 = tx.query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))?;
        let stale = (total as usize).saturating_sub(updated + inserted);
        if stale > 0 {
            log::warn!(
                "refresh_catalog: {} installed rows are no longer in the bundled catalog \
                 (kept; their torrent indices may be stale)",
                stale
            );
        }

        // ── Curated playlist sync ─────────────────────────────────────
        // Curated rows are catalog content with nothing user-owned hanging
        // off them, so the sync is a plain rebuild: drop them all (their
        // memberships cascade) and re-insert from the bundled catalog.
        // UNIQUE is (kind, name), so a user playlist sharing a curated name
        // can never collide - no OR IGNORE, no skipped lists, and a failure
        // here is a real bug that should fail the refresh loudly.
        // kind='user' playlists are never touched. The membership remap goes
        // through application_path (title+language for empty-path rows) -
        // same keys, same planner-friendly split as the games sync above.
        tx.execute_batch("DELETE FROM playlists WHERE kind = 'curated';")?;
        tx.execute(
            "INSERT INTO playlists (name, kind, slug, description)
             SELECT c.name, 'curated', c.slug, c.description
             FROM cat.playlists AS c
             WHERE c.kind = 'curated'",
            [],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO playlist_games (playlist_id, game_id)
             SELECT p.id, g.id
             FROM cat.playlist_games AS cpg
             JOIN cat.playlists AS cp ON cp.id = cpg.playlist_id AND cp.kind = 'curated'
             JOIN playlists AS p ON p.slug = cp.slug AND p.kind = 'curated'
             JOIN cat.games AS cg ON cg.id = cpg.game_id
             JOIN games AS g ON g.application_path = cg.application_path
             WHERE cg.application_path IS NOT NULL AND cg.application_path != ''",
            [],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO playlist_games (playlist_id, game_id)
             SELECT p.id, g.id
             FROM cat.playlist_games AS cpg
             JOIN cat.playlists AS cp ON cp.id = cpg.playlist_id AND cp.kind = 'curated'
             JOIN playlists AS p ON p.slug = cp.slug AND p.kind = 'curated'
             JOIN cat.games AS cg ON cg.id = cpg.game_id
             JOIN games AS g ON g.title = cg.title AND g.language = cg.language
             WHERE cg.application_path IS NULL OR cg.application_path = ''",
            [],
        )?;

        // Stamp the version OF THE BUNDLED DB, not the code constant: during
        // development the code can be ahead of a not-yet-regenerated bundled
        // catalog, and stamping the constant would mark that stale import as
        // current forever (seen live: CATALOG_VERSION 6 shipped hours before
        // the v6 exodium.db - installs that started in between imported zero
        // eXoWin9x rows and never refreshed again).
        let bundled_version: String = tx
            .query_row(
                "SELECT value FROM cat.config WHERE key = 'catalog_version'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| CATALOG_VERSION.to_string());
        tx.execute(
            "INSERT INTO config (key, value) VALUES ('catalog_version', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [bundled_version],
        )?;
        tx.commit()?;
        Ok((updated, inserted))
    })();

    let _ = conn.execute("DETACH DATABASE cat", []);
    // The bundled artefact can carry stale LP thumbnail keys (its generator
    // matched across pack families), and the row copy above just wrote them
    // over whatever migrate() fixed at startup. Re-link after every refresh,
    // or the catalog update resurrects the broken covers it was meant to fix.
    // Log-only: the version stamp is already committed above, so an Err here
    // would make the caller skip enable_new_collections with no retry on the
    // next start - and migrate() re-runs both helpers anyway.
    if result.is_ok() {
        if let Err(e) = populate_thumbnail_keys(conn) {
            log::warn!("post-refresh thumbnail key populate failed: {e}");
        }
        if let Err(e) = propagate_lp_thumbnail_keys(conn) {
            log::warn!("post-refresh LP thumbnail relink failed: {e}");
        }
    }
    result
}

/// Installed DB's catalog version (0 when the key predates versioning).
pub fn catalog_version(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT value FROM config WHERE key = 'catalog_version'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(0)
}

/// Additive migrations for existing databases.
fn migrate(conn: &Connection) -> DbResult<()> {
    // Add dosbox_variant column if missing (added after initial release).
    let has_dosbox_variant: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('games') WHERE name = 'dosbox_variant'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_dosbox_variant {
        conn.execute_batch("ALTER TABLE games ADD COLUMN dosbox_variant TEXT")?;
    }

    // Add favorited column if missing (added after initial release).
    let has_favorited: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('games') WHERE name = 'favorited'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_favorited {
        conn.execute_batch("ALTER TABLE games ADD COLUMN favorited INTEGER NOT NULL DEFAULT 0")?;
    }
    // Ensure index exists (safe for both new and migrated DBs)
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_games_favorited ON games(favorited)")?;

    // Content-addressed thumbnail identifier (SHA-256(normalized title)[:16]).
    // Supersedes shortcode-derived filenames; populated by generate_db.rs at
    // build time and copied from EN → LP variants by the backfill in setup.rs.
    let has_thumbnail_key: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('games') WHERE name = 'thumbnail_key'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_thumbnail_key {
        conn.execute_batch("ALTER TABLE games ADD COLUMN thumbnail_key TEXT")?;
    }

    // LaunchBox ManualPath (e.g. "Manuals\MS-DOS\Capitalism (1995).pdf").
    let has_manual_path: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('games') WHERE name = 'manual_path'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_manual_path {
        conn.execute_batch("ALTER TABLE games ADD COLUMN manual_path TEXT")?;
    }

    // ISO 8601 timestamp of last launch. Updated by launch_game on each play.
    let has_last_played: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('games') WHERE name = 'last_played'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !has_last_played {
        conn.execute_batch("ALTER TABLE games ADD COLUMN last_played TEXT")?;
    }

    // Community vote count behind `rating` - "Top rated" sorts by it inside
    // each star bucket so one-vote 5.0s stop outranking widely-rated games.
    let game_cols = table_columns(conn, "games")?;
    if !game_cols.iter().any(|c| c == "rating_votes") {
        conn.execute_batch("ALTER TABLE games ADD COLUMN rating_votes INTEGER")?;
    }
    // Theme-track hint from LaunchBox (MusicPath / MissingMusic); filled by
    // the bundled catalog, which refresh_catalog copies column-wise.
    if !game_cols.iter().any(|c| c == "music_file") {
        conn.execute_batch("ALTER TABLE games ADD COLUMN music_file TEXT")?;
    }

    // Playlist support (curated eXo playlists + user playlists). The tables
    // existed since 0.1 but were never populated, so plain ALTERs are safe.
    let playlist_cols = table_columns(conn, "playlists")?;
    if !playlist_cols.iter().any(|c| c == "kind") {
        conn.execute_batch(
            "ALTER TABLE playlists ADD COLUMN kind TEXT NOT NULL DEFAULT 'user'",
        )?;
    }
    if !playlist_cols.iter().any(|c| c == "slug") {
        conn.execute_batch("ALTER TABLE playlists ADD COLUMN slug TEXT")?;
    }
    if !playlist_cols.iter().any(|c| c == "description") {
        conn.execute_batch("ALTER TABLE playlists ADD COLUMN description TEXT")?;
    }
    let playlist_game_cols = table_columns(conn, "playlist_games")?;
    if !playlist_game_cols.iter().any(|c| c == "position") {
        conn.execute_batch("ALTER TABLE playlist_games ADD COLUMN position INTEGER")?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_playlist_games_game ON playlist_games(game_id)",
    )?;

    // Name uniqueness is per KIND, not global: identity for curated rows is
    // the slug, for user rows the name. A global UNIQUE(name) let a user
    // playlist collide with a curated one, which made the curated sync in
    // refresh_catalog either skip lists silently or fail the whole refresh.
    // Rebuild the table when it still carries the old constraint (its
    // CREATE sql lacks "UNIQUE (kind"). Copying preserves ids, so
    // playlist_games FKs stay valid; foreign_keys is toggled off so the
    // DROP doesn't trip the child table's references.
    let playlists_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'playlists'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();
    if !playlists_sql.contains("UNIQUE (kind") {
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE playlists_new (
                 id          INTEGER PRIMARY KEY,
                 name        TEXT NOT NULL,
                 kind        TEXT NOT NULL DEFAULT 'user',
                 slug        TEXT,
                 description TEXT,
                 UNIQUE (kind, name)
             );
             INSERT INTO playlists_new (id, name, kind, slug, description)
                 SELECT id, name, kind, slug, description FROM playlists;
             DROP TABLE playlists;
             ALTER TABLE playlists_new RENAME TO playlists;
             PRAGMA foreign_keys=ON;",
        )?;
    }

    // Force thumbnail_key recomputation whenever the hash or canonical-matcher
    // algorithms change. Bumped on every release that alters:
    //   - title_thumbnail_key() (the hash function itself), or
    //   - title_canonical() (LP↔EN propagation rule)
    // Bump history:
    //   v1 - initial content-addressed (title.trim().lowercase().whitespace-collapse)
    //   v2 - stripped-alnum hash + basic article drop
    //   v3 - marketing-modifier drop + British/American spelling folds
    //   v4 - stop-word prepositions + standalone "1"/"i" dropped
    //
    // Without this check, existing users keep their old thumbnail_key values
    // and the new canonical matcher never runs against them - bundled files
    // use current hashes, DB rows use old hashes, every card 404s.
    //   v5 - shortcode-based propagation added to propagate_lp_thumbnail_keys
    const CURRENT_HASH_VERSION: &str = "5";
    let stored_version: Option<String> =
        queries::get_config(conn, "thumbnail_hash_version").ok().flatten();
    if stored_version.as_deref() != Some(CURRENT_HASH_VERSION) {
        log::info!(
            "thumbnail_hash_version changed ({:?} → {}), recomputing all keys",
            stored_version, CURRENT_HASH_VERSION
        );
        conn.execute_batch("UPDATE games SET thumbnail_key = NULL")?;
    }

    // Populate any NULL thumbnail_key values from the title hash. Handles:
    //   (1) existing v0.2.x DBs that just got the column added,
    //   (2) any row whose thumbnail_key got wiped by a re-import, and
    //   (3) the version-bump recompute above.
    populate_thumbnail_keys(conn)?;

    // Ensure LP variants share their EN primary's cover art even when shortcode
    // matching didn't link them (divergent auto-generated shortcodes for the
    // "same" game). Runs after populate so every row has a key to potentially
    // overwrite.
    propagate_lp_thumbnail_keys(conn)?;

    if stored_version.as_deref() != Some(CURRENT_HASH_VERSION) {
        queries::set_config(conn, "thumbnail_hash_version", CURRENT_HASH_VERSION)?;
    }

    // Backfill manual_path from bundled XML for DBs that were built before the
    // ManualPath field was added. Runs once: checks if ANY row has a non-NULL
    // manual_path; if zero, reads all bundled .xml.gz files and updates matching
    // rows by title. Idempotent - subsequent calls find rows populated and skip.
    populate_manual_paths(conn)?;

    // Purge pack sentinel rows ("! eXoDOS" / "! eXoWin9x" with a root-level
    // Setup bat) that older imports let through - the XML filter only knew the
    // pathless eXoWin3x shape. They sat at the very top of every sort under
    // "All Collections". refresh_catalog never deletes rows, so existing
    // installs need this even after the regenerated catalog drops them.
    let purged = conn.execute(
        "DELETE FROM games
         WHERE sort_title LIKE '!%'
           AND (application_path IS NULL
                OR (application_path NOT LIKE '%\\%' AND application_path NOT LIKE '%/%'))",
        [],
    )?;
    if purged > 0 {
        log::info!("Removed {} pack sentinel rows", purged);
    }

    // Normalize spelled-out language names left by older imports (the main
    // eXoDOS catalog writes "Language: Japanese" where the packs write codes).
    // Mirrors import::xml::normalize_language; a catalog refresh also carries
    // the corrected values, but a DB already stamped current never refreshes.
    conn.execute_batch(
        "UPDATE games SET language = CASE language
            WHEN 'ENGLISH' THEN 'EN' WHEN 'GERMAN' THEN 'DE'
            WHEN 'SPANISH' THEN 'ES' WHEN 'POLISH' THEN 'PL'
            WHEN 'FRENCH' THEN 'FR' WHEN 'ITALIAN' THEN 'IT'
            WHEN 'DUTCH' THEN 'NL' WHEN 'FINNISH' THEN 'FI'
            WHEN 'JAPANESE' THEN 'JA' WHEN 'CHINESE' THEN 'ZH'
            ELSE language END
         WHERE language IN ('ENGLISH','GERMAN','SPANISH','POLISH','FRENCH',
                            'ITALIAN','DUTCH','FINNISH','JAPANESE','CHINESE')",
    )?;

    Ok(())
}

/// One-time backfill: read <ManualPath> from bundled XML files and update games
/// rows that still have NULL manual_path.
fn populate_manual_paths(conn: &Connection) -> DbResult<()> {
    // Version guard: bump to force re-population when the backfill logic changes.
    const MANUAL_PATH_VERSION: &str = "3";
    let stored = queries::get_config(conn, "manual_path_version").ok().flatten();
    if stored.as_deref() == Some(MANUAL_PATH_VERSION) {
        return Ok(());
    }
    // Wipe stale values from a previous (buggy) backfill before re-populating.
    conn.execute_batch("UPDATE games SET manual_path = NULL")?;

    // Find the metadata directory (dev or production).
    let metadata_dir = {
        let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join("metadata"));
        if let Some(ref d) = dev {
            if d.exists() {
                Some(d.clone())
            } else {
                crate::commands::setup::RESOURCE_DIR
                    .get()
                    .map(|r| r.join("metadata"))
            }
        } else {
            None
        }
    };

    let Some(metadata_dir) = metadata_dir else {
        log::debug!("populate_manual_paths: metadata dir not found, skipping");
        return Ok(());
    };

    // Parse ManualPath from each bundled XML and build a title → path map.
    // Derived from COLLECTION_MAP rather than listed here: a hardcoded copy
    // silently skips any collection added later, and the games just go missing.
    let xml_files: Vec<&str> = crate::COLLECTION_MAP
        .iter()
        .map(|c| c.metadata_file)
        .collect();
    let mut manual_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for filename in &xml_files {
        let path = metadata_dir.join(filename);
        if !path.exists() {
            continue;
        }
        if let Ok(file) = std::fs::File::open(&path) {
            let decoder = flate2::read::GzDecoder::new(file);
            let reader = std::io::BufReader::new(decoder);
            // Scan per <Game> block: collect Title + ManualPath, emit pair at </Game>.
            // ManualPath appears BEFORE Title in LaunchBox XML, so we can't pair sequentially.
            let mut block_title: Option<String> = None;
            let mut block_manual: Option<String> = None;
            use std::io::BufRead;
            for line in reader.lines() {
                let Ok(line) = line else { continue };
                let trimmed = line.trim();
                if trimmed == "<Game>" {
                    block_title = None;
                    block_manual = None;
                } else if trimmed == "</Game>" {
                    if let (Some(title), Some(mp)) = (block_title.take(), block_manual.take()) {
                        manual_map.entry(title).or_insert(mp);
                    }
                } else if let Some(t) = extract_xml_value(trimmed, "Title") {
                    block_title = Some(t);
                } else if let Some(mp) = extract_xml_value(trimmed, "ManualPath") {
                    if !mp.is_empty() {
                        block_manual = Some(mp);
                    }
                }
            }
        }
    }

    if manual_map.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "UPDATE games SET manual_path = ?1 WHERE title = ?2 AND manual_path IS NULL",
        )?;
        let mut updated = 0usize;
        for (title, mp) in &manual_map {
            updated += stmt.execute(rusqlite::params![mp, title])?;
        }
        log::info!("populate_manual_paths: updated {} rows from {} XML entries", updated, manual_map.len());
    }
    tx.commit()?;
    queries::set_config(conn, "manual_path_version", MANUAL_PATH_VERSION)?;
    Ok(())
}

/// Extract the text content of a simple XML element like `<Title>foo</Title>`.
/// Decodes the standard XML entities (`&amp;`, `&lt;`, `&gt;`, `&apos;`, `&quot;`)
/// so the returned string matches what quick_xml's serde deserializer produces.
fn extract_xml_value(line: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = line.find(&open)? + open.len();
    let end = line.find(&close)?;
    if start <= end {
        let raw = &line[start..end];
        Some(
            raw.replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&apos;", "'")
                .replace("&quot;", "\""),
        )
    } else {
        None
    }
}

/// SHA-256(alnum-only lowercase title)[:16] - must match the Python and
/// generate_db.rs binary implementations exactly, or filename lookup misses.
///
/// The normalization is deliberately aggressive: lowercase, then keep only
/// ASCII alphanumerics. This means "3-K Trivia" and "3K Trivia" and
/// "3, K. Trivia!" all hash to the same filename - punctuation variants
/// across XML / zip / image filenames merge automatically.
/// Lowercase + strip to ASCII alphanumeric only. Shared by the thumbnail hash
/// and the metadata image-file matcher in `commands::setup`.
pub fn normalize_alnum(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn title_thumbnail_key(title: &str) -> String {
    use sha2::{Digest, Sha256};
    let norm = normalize_alnum(title);
    let hash = format!("{:x}", Sha256::digest(norm.as_bytes()));
    hash[..16].to_string()
}

/// Aggressive title canonicalization used only for LP↔EN matching (not for
/// filename hashing). Produces a stable "fingerprint" that survives the
/// common cross-language title drift we see in eXoDOS LP catalogues:
///   - leading articles dropped ("The Legend of..." → "Legend of...")
///   - English word-numbers folded into digits ("Book Two" → "Book 2")
///   - Roman numerals folded into digits ("Settlers II" → "Settlers 2")
///   - all non-alphanumerics stripped, lowercased
///
/// Example: "The Legend of Kyrandia: Book Two - The Hand of Fate" and
/// "Legend of Kyrandia Book 2 - The Hand of Fate" both produce
/// "legendofkyrandiabook2thehandoffate".
///
/// Only the leading article is stripped (not every occurrence) so subtitles
/// like "The Hand of Fate" inside a longer title stay intact.
pub fn title_canonical(title: &str) -> String {
    let t = title.trim().to_lowercase();

    // Word-number and Roman-numeral substitutions. Applied token-by-token so
    // we never replace substrings inside words (e.g. "two" inside "twofold").
    const SUBSTITUTIONS: &[(&str, &str)] = &[
        ("one", "1"), ("two", "2"), ("three", "3"), ("four", "4"), ("five", "5"),
        ("six", "6"), ("seven", "7"), ("eight", "8"), ("nine", "9"), ("ten", "10"),
        ("ii", "2"), ("iii", "3"), ("iv", "4"), ("vi", "6"), ("vii", "7"),
        ("viii", "8"), ("ix", "9"),
    ];

    // Tokens to drop entirely - they're noise when matching LP↔EN titles:
    //   - articles: "the", "a", "an" (LaunchBox's ", The" suffix convention)
    //   - stop-word prepositions/conjunctions that LP packs include or omit
    //       inconsistently: "in", "of", "and", "to", "for", "on"
    //   - "first in series" markers - LP packs often add "1"/"i" where EN
    //       has no number (the first game's sequel is "2" but the first
    //       itself is unnumbered). Standalone "1" and "i" drops, higher
    //       numbers stay (they distinguish "Larry 2" from "Larry 3").
    //   - structural connectors: "part", "book", "chapter", "volume", "episode"
    //   - marketing modifiers: "enhanced", "version", "edition", "gold",
    //       "deluxe", "special", "cd", "cdrom", "vga", "ega", "collectors",
    //       "limited", "talkie", "sci", "remake"
    //   - British/American spelling noise is folded below (not dropped)
    //
    // Dropping these is deliberately aggressive - we accept the occasional
    // false positive (e.g. two unrelated games whose canonical forms collide
    // because all differentiating words were stopwords) in exchange for
    // catching the bulk of LP title drift. Game titles distinctive enough to
    // matter have multiple content words.
    const DROP_TOKENS: &[&str] = &[
        "the", "a", "an",
        "in", "of", "and", "to", "for", "on",
        "1", "i",
        "part", "book", "chapter", "volume", "episode",
        "enhanced", "version", "edition", "gold", "deluxe", "special",
        "cd", "cdrom", "vga", "ega", "collectors", "collector",
        "limited", "talkie", "sci", "remake", "classic", "classics",
    ];

    // Cross-spelling token substitutions (bidirectional - whichever variant
    // appears gets folded into the other so both hash the same).
    const SPELLING_FOLDS: &[(&str, &str)] = &[
        ("judgement", "judgment"),
        ("colour", "color"),
        ("armour", "armor"),
        ("honour", "honor"),
        ("centre", "center"),
        ("grey", "gray"),
    ];

    let tokens: Vec<String> = t
        .split_whitespace()
        .filter_map(|tok| {
            let trimmed: String = tok.chars().filter(|c| c.is_alphanumeric()).collect();
            if DROP_TOKENS.contains(&trimmed.as_str()) {
                return None;
            }
            for (from, to) in SPELLING_FOLDS {
                if trimmed == *from {
                    return Some(to.to_string());
                }
            }
            for (from, to) in SUBSTITUTIONS {
                if trimmed == *from {
                    return Some(to.to_string());
                }
            }
            Some(tok.to_string())
        })
        .collect();
    let rejoined = tokens.join(" ");

    rejoined.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// For each LP game, find an EN game with a matching *canonical* title and
/// copy its thumbnail_key. Catches the cases where shortcode-based matching
/// fails (LP-generated shortcodes that diverge from EN) but the titles are
/// clearly the same game modulo article/numeral/punctuation differences.
///
/// Idempotent: running twice makes no further changes.
pub fn propagate_lp_thumbnail_keys(conn: &Connection) -> DbResult<()> {
    // Both passes are family-scoped (§1 in CLAUDE.md): shortcodes AND titles
    // repeat across pack families (eXoWin9x carries "Gabriel Knight 2 - The
    // Beast Within"), and an unscoped match hands an LP row a key whose file
    // only exists in the OTHER family's poster pack - the detail panel then
    // 404s into the placeholder for that variant.
    let same = queries::same_group("en", "games");

    // Pass 1: shortcode-based - most reliable, catches cases like
    // "Space Quest V - The Next Mutation" (DE) ↔ "Space Quest V: Roger Wilco
    // The Next Mutation" (EN) where the titles diverge too much for canonical
    // matching but the shortcode (SQ5) is shared.
    let shortcode_updated: usize = conn.execute(
        &format!(
            "UPDATE games SET thumbnail_key = (
                SELECT en.thumbnail_key FROM games en
                WHERE en.language = 'EN'
                  AND {same}
                  AND en.thumbnail_key IS NOT NULL
                LIMIT 1
            )
            WHERE shortcode IS NOT NULL
              AND language != 'EN'
              AND EXISTS (
                  SELECT 1 FROM games en
                  WHERE en.language = 'EN'
                    AND {same}
                    AND en.thumbnail_key IS NOT NULL
              )"
        ),
        [],
    )?;

    // Pass 2: canonical-title matching - catches LP games with divergent
    // shortcodes but recognizably-same titles. Keyed per (family, canonical),
    // and rows pass 1 already matched are excluded: the shortcode link is the
    // stronger signal, and this pass used to overwrite it on every start.
    let mut en_map: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    let fam = queries::family_expr("games");
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT title, thumbnail_key, {fam} FROM games
             WHERE language = 'EN' AND thumbnail_key IS NOT NULL",
        ))?;
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for (title, key, family) in iter.flatten() {
            en_map.entry((family, title_canonical(&title))).or_insert(key);
        }
    }

    // For each LP game without a shortcode match, look up canonical title and
    // overwrite thumbnail_key when an EN match exists and differs.
    let lp_rows: Vec<(i64, String, Option<String>, String)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT id, title, thumbnail_key, {fam} FROM games
             WHERE language != 'EN'
               AND NOT EXISTS (
                   SELECT 1 FROM games en
                   WHERE en.language = 'EN'
                     AND {same}
                     AND en.thumbnail_key IS NOT NULL
               )",
        ))?;
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        iter.flatten().collect()
    };

    let tx = conn.unchecked_transaction()?;
    let mut updated = 0usize;
    {
        let mut upd = tx.prepare_cached("UPDATE games SET thumbnail_key = ?1 WHERE id = ?2")?;
        for (id, title, current, family) in &lp_rows {
            if let Some(en_hash) = en_map.get(&(family.clone(), title_canonical(title))) {
                if current.as_deref() != Some(en_hash) {
                    upd.execute(rusqlite::params![en_hash, id])?;
                    updated += 1;
                }
            }
        }
    }
    tx.commit()?;
    log::info!(
        "propagate_lp_thumbnail_keys: {} via shortcode, {} via canonical title",
        shortcode_updated, updated
    );
    Ok(())
}

pub fn populate_thumbnail_keys(conn: &Connection) -> DbResult<()> {
    let null_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM games WHERE thumbnail_key IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if null_count == 0 {
        return Ok(());
    }

    let rows: Vec<(i64, String)> = {
        let mut stmt =
            conn.prepare("SELECT id, title FROM games WHERE thumbnail_key IS NULL")?;
        let iter = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        iter.filter_map(|r| r.ok()).collect()
    };

    let tx = conn.unchecked_transaction()?;
    {
        let mut upd =
            tx.prepare_cached("UPDATE games SET thumbnail_key = ?1 WHERE id = ?2")?;
        for (id, title) in &rows {
            upd.execute(rusqlite::params![title_thumbnail_key(title), id])?;
        }
    }
    tx.commit()?;
    log::info!("Populated thumbnail_key for {} games (migration)", rows.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_db(dir: &Path, name: &str) -> (std::path::PathBuf, Connection) {
        let path = dir.join(name);
        let conn = open(&path).unwrap();
        init(&conn).unwrap();
        (path, conn)
    }

    fn insert_game(
        conn: &Connection,
        title: &str,
        app_path: &str,
        torrent_index: i64,
        description: &str,
    ) -> i64 {
        conn.execute(
            "INSERT INTO games (title, language, application_path, game_torrent_index, description) \
             VALUES (?1, 'EN', ?2, ?3, ?4)",
            rusqlite::params![title, app_path, torrent_index, description],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn refresh_catalog_updates_in_place_and_preserves_user_state() {
        let dir = tempfile::tempdir().unwrap();

        // "Installed" DB: two games, user state on A, per-game config on A.
        let (_, mut installed) = mk_db(dir.path(), "installed.db");
        let id_a = insert_game(&installed, "Alpha", "eXo\\eXoDOS\\!dos\\AL\\dosbox.conf", 10, "old");
        let id_b = insert_game(&installed, "Beta", "eXo\\eXoDOS\\!dos\\BE\\dosbox.conf", 20, "old");
        installed
            .execute(
                "UPDATE games SET installed = 1, in_library = 1, favorited = 1, \
                 last_played = '2026-01-01' WHERE id = ?1",
                [id_a],
            )
            .unwrap();
        installed
            .execute(
                "INSERT INTO game_config (game_id, key, value) VALUES (?1, 'k', 'v')",
                [id_a],
            )
            .unwrap();

        // "Bundled" DB: A with corrected index + new description, D brand new.
        // B is gone from the catalog (must be kept in the installed DB).
        let (bundled_path, bundled) = mk_db(dir.path(), "bundled.db");
        insert_game(&bundled, "Alpha", "eXo\\eXoDOS\\!dos\\AL\\dosbox.conf", 99, "new");
        insert_game(&bundled, "Delta", "eXo\\eXoDOS\\!dos\\DE\\dosbox.conf", 30, "new");
        drop(bundled);

        assert_eq!(catalog_version(&installed), 0);
        let (updated, inserted) = refresh_catalog(&mut installed, &bundled_path).unwrap();
        assert_eq!((updated, inserted), (1, 1));
        assert_eq!(catalog_version(&installed), CATALOG_VERSION);

        // A: catalog columns refreshed, id + user state intact.
        let (id, idx, desc, inst, fav, last): (i64, i64, String, i64, i64, String) = installed
            .query_row(
                "SELECT id, game_torrent_index, description, installed, favorited, last_played \
                 FROM games WHERE title = 'Alpha'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(id, id_a);
        assert_eq!(idx, 99);
        assert_eq!(desc, "new");
        assert_eq!((inst, fav), (1, 1));
        assert_eq!(last, "2026-01-01");

        // game_config survived (id stable, no cascade).
        let cfg: String = installed
            .query_row(
                "SELECT value FROM game_config WHERE game_id = ?1 AND key = 'k'",
                [id_a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cfg, "v");

        // B kept even though gone from the catalog; D inserted with clean state.
        let b_count: i64 = installed
            .query_row("SELECT COUNT(*) FROM games WHERE id = ?1", [id_b], |r| r.get(0))
            .unwrap();
        assert_eq!(b_count, 1);
        let (d_inst, d_lib): (i64, i64) = installed
            .query_row(
                "SELECT installed, in_library FROM games WHERE title = 'Delta'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((d_inst, d_lib), (0, 0));

        // Idempotency: a second refresh updates the same rows again but must
        // not duplicate anything or clobber user state.
        let (updated2, inserted2) = refresh_catalog(&mut installed, &bundled_path).unwrap();
        assert_eq!(inserted2, 0);
        assert_eq!(updated2, 2); // Alpha + Delta both match now
        let total: i64 = installed
            .query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3); // Alpha, Beta (stale, kept), Delta
        let still_fav: i64 = installed
            .query_row("SELECT favorited FROM games WHERE id = ?1", [id_a], |r| r.get(0))
            .unwrap();
        assert_eq!(still_fav, 1);
    }

    #[test]
    fn refresh_catalog_syncs_curated_playlists_preserves_user_playlists() {
        let dir = tempfile::tempdir().unwrap();

        // Installed DB: game Alpha, a user playlist holding it, plus two
        // curated playlists from a previous catalog - one that still exists
        // (stale membership) and one that vanished.
        let (_, mut installed) = mk_db(dir.path(), "installed.db");
        let id_a = insert_game(&installed, "Alpha", "eXo\\eXoDOS\\!dos\\AL\\dosbox.conf", 10, "old");
        // 'Games with MT-32' as a USER playlist: with per-kind uniqueness the
        // incoming curated list of the same name must coexist, not collide.
        installed
            .execute_batch(
                "INSERT INTO playlists (name, kind) VALUES ('Mine', 'user');
                 INSERT INTO playlists (name, kind) VALUES ('Games with MT-32', 'user');
                 INSERT INTO playlists (name, kind, slug, description)
                     VALUES ('MT-32 (old name)', 'curated', 'mt-32', 'old desc');
                 INSERT INTO playlists (name, kind, slug)
                     VALUES ('Vanished', 'curated', 'vanished');",
            )
            .unwrap();
        let user_pid: i64 = installed
            .query_row("SELECT id FROM playlists WHERE name = 'Mine'", [], |r| r.get(0))
            .unwrap();
        installed
            .execute(
                "INSERT INTO playlist_games (playlist_id, game_id)
                 SELECT id, ?1 FROM playlists",
                [id_a],
            )
            .unwrap();

        // Bundled DB: Alpha + Delta; curated mt-32 now contains only Delta
        // and carries a fresh name + description.
        let (bundled_path, bundled) = mk_db(dir.path(), "bundled.db");
        insert_game(&bundled, "Alpha", "eXo\\eXoDOS\\!dos\\AL\\dosbox.conf", 99, "new");
        let cat_delta = insert_game(&bundled, "Delta", "eXo\\eXoDOS\\!dos\\DE\\dosbox.conf", 30, "new");
        bundled
            .execute_batch(
                "INSERT INTO playlists (name, kind, slug, description)
                     VALUES ('Games with MT-32', 'curated', 'mt-32', 'new desc');",
            )
            .unwrap();
        bundled
            .execute(
                "INSERT INTO playlist_games (playlist_id, game_id)
                 SELECT id, ?1 FROM playlists",
                [cat_delta],
            )
            .unwrap();
        drop(bundled);

        refresh_catalog(&mut installed, &bundled_path).unwrap();

        // User playlist untouched.
        let user_games: Vec<i64> = installed
            .prepare("SELECT game_id FROM playlist_games WHERE playlist_id = ?1")
            .unwrap()
            .query_map([user_pid], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(user_games, vec![id_a]);

        // Vanished curated playlist removed entirely.
        let vanished: i64 = installed
            .query_row("SELECT COUNT(*) FROM playlists WHERE slug = 'vanished'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vanished, 0);

        // mt-32: renamed, description updated, membership rebuilt and
        // remapped to the INSTALLED DB's Delta id via application_path -
        // even though a USER playlist owns the same display name.
        let (name, desc, pid): (String, String, i64) = installed
            .query_row(
                "SELECT name, description, id FROM playlists WHERE slug = 'mt-32'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Games with MT-32");
        assert_eq!(desc, "new desc");
        let same_name: i64 = installed
            .query_row(
                "SELECT COUNT(*) FROM playlists WHERE name = 'Games with MT-32'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(same_name, 2); // user + curated coexist
        let installed_delta: i64 = installed
            .query_row("SELECT id FROM games WHERE title = 'Delta'", [], |r| r.get(0))
            .unwrap();
        let members: Vec<i64> = installed
            .prepare("SELECT game_id FROM playlist_games WHERE playlist_id = ?1")
            .unwrap()
            .query_map([pid], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(members, vec![installed_delta]);
    }

    #[test]
    fn refresh_catalog_relinks_lp_thumbnail_keys() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut installed) = mk_db(dir.path(), "installed.db");
        installed
            .execute_batch(
                "INSERT INTO games (title, language, shortcode, torrent_source, thumbnail_key) VALUES
                   ('The Beast Within: A Gabriel Knight Mystery', 'EN', 'GK2', 'eXoDOS', 'en_dos_key'),
                   ('Gabriel Knight 2 - The Beast Within', 'DE', 'GK2', 'eXoDOS_GLP', 'en_dos_key');",
            )
            .unwrap();

        // The bundled DB carries a cross-family key for the DE row - the row
        // copy writes it into the installed DB, and the refresh has to repair
        // that itself or every catalog update breaks the LP covers again.
        let (bundled_path, bundled) = mk_db(dir.path(), "bundled.db");
        bundled
            .execute_batch(
                "INSERT INTO games (title, language, shortcode, torrent_source, thumbnail_key) VALUES
                   ('The Beast Within: A Gabriel Knight Mystery', 'EN', 'GK2', 'eXoDOS', 'en_dos_key'),
                   ('Gabriel Knight 2 - The Beast Within', 'DE', 'GK2', 'eXoDOS_GLP', 'win9x_key');",
            )
            .unwrap();
        drop(bundled);

        refresh_catalog(&mut installed, &bundled_path).unwrap();
        let de_key: String = installed
            .query_row(
                "SELECT thumbnail_key FROM games WHERE language = 'DE'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(de_key, "en_dos_key");
    }

    #[test]
    fn lp_thumbnail_key_propagation_is_family_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let (_, conn) = mk_db(dir.path(), "fam.db");
        // The Gabriel Knight 2 constellation: the DE row's title canonicalizes
        // to the Win9x EN row's title, while its shortcode links it to the DOS
        // family's EN row. Pass 2 used to overwrite pass 1 with the Win9x key,
        // whose file only exists in the Win9x poster pack.
        conn.execute_batch(
            "INSERT INTO games (title, language, shortcode, torrent_source, thumbnail_key) VALUES
               ('The Beast Within: A Gabriel Knight Mystery', 'EN', 'GK2', 'eXoDOS', 'en_dos_key'),
               ('Gabriel Knight 2: The Beast Within', 'EN', 'Gabriel Knight 2 - The Beast Within (1995)', 'eXoWin9x', 'win9x_key'),
               ('Gabriel Knight 2 - The Beast Within', 'DE', 'GK2', 'eXoDOS_GLP', 'own_de_key');",
        )
        .unwrap();

        let de_key = || -> String {
            conn.query_row(
                "SELECT thumbnail_key FROM games WHERE language = 'DE'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };

        propagate_lp_thumbnail_keys(&conn).unwrap();
        assert_eq!(de_key(), "en_dos_key");
        // Idempotent: the second run (every startup runs one) must not drift.
        propagate_lp_thumbnail_keys(&conn).unwrap();
        assert_eq!(de_key(), "en_dos_key");
    }
}
