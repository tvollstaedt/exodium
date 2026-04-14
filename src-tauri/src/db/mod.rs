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
}

pub type DbResult<T> = Result<T, DbError>;

/// Open (or create) the Exodium database at the given path.
pub fn open(path: &Path) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    Ok(conn)
}

/// Initialize the database schema (idempotent).
pub fn init(conn: &Connection) -> DbResult<()> {
    schema::create_tables(conn)?;
    migrate(conn)?;
    Ok(())
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

    // Force thumbnail_key recomputation whenever the hash or canonical-matcher
    // algorithms change. Bumped on every release that alters:
    //   - title_thumbnail_key() (the hash function itself), or
    //   - title_canonical() (LP↔EN propagation rule)
    // Bump history:
    //   v1 — initial content-addressed (title.trim().lowercase().whitespace-collapse)
    //   v2 — stripped-alnum hash + basic article drop
    //   v3 — marketing-modifier drop + British/American spelling folds
    //   v4 — stop-word prepositions + standalone "1"/"i" dropped
    //
    // Without this check, existing users keep their old thumbnail_key values
    // and the new canonical matcher never runs against them — bundled files
    // use current hashes, DB rows use old hashes, every card 404s.
    const CURRENT_HASH_VERSION: &str = "4";
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

    Ok(())
}

/// SHA-256(alnum-only lowercase title)[:16] — must match the Python and
/// generate_db.rs binary implementations exactly, or filename lookup misses.
///
/// The normalization is deliberately aggressive: lowercase, then keep only
/// ASCII alphanumerics. This means "3-K Trivia" and "3K Trivia" and
/// "3, K. Trivia!" all hash to the same filename — punctuation variants
/// across XML / zip / image filenames merge automatically.
fn title_thumbnail_key(title: &str) -> String {
    use sha2::{Digest, Sha256};
    let norm: String = title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
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
fn title_canonical(title: &str) -> String {
    let t = title.trim().to_lowercase();

    // Word-number and Roman-numeral substitutions. Applied token-by-token so
    // we never replace substrings inside words (e.g. "two" inside "twofold").
    const SUBSTITUTIONS: &[(&str, &str)] = &[
        ("one", "1"), ("two", "2"), ("three", "3"), ("four", "4"), ("five", "5"),
        ("six", "6"), ("seven", "7"), ("eight", "8"), ("nine", "9"), ("ten", "10"),
        ("ii", "2"), ("iii", "3"), ("iv", "4"), ("vi", "6"), ("vii", "7"),
        ("viii", "8"), ("ix", "9"),
    ];

    // Tokens to drop entirely — they're noise when matching LP↔EN titles:
    //   - articles: "the", "a", "an" (LaunchBox's ", The" suffix convention)
    //   - stop-word prepositions/conjunctions that LP packs include or omit
    //       inconsistently: "in", "of", "and", "to", "for", "on"
    //   - "first in series" markers — LP packs often add "1"/"i" where EN
    //       has no number (the first game's sequel is "2" but the first
    //       itself is unnumbered). Standalone "1" and "i" drops, higher
    //       numbers stay (they distinguish "Larry 2" from "Larry 3").
    //   - structural connectors: "part", "book", "chapter", "volume", "episode"
    //   - marketing modifiers: "enhanced", "version", "edition", "gold",
    //       "deluxe", "special", "cd", "cdrom", "vga", "ega", "collectors",
    //       "limited", "talkie", "sci", "remake"
    //   - British/American spelling noise is folded below (not dropped)
    //
    // Dropping these is deliberately aggressive — we accept the occasional
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

    // Cross-spelling token substitutions (bidirectional — whichever variant
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
    // Build canonical→thumbnail_key map from EN games with a hash.
    let mut en_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT title, thumbnail_key FROM games
             WHERE language = 'EN' AND thumbnail_key IS NOT NULL",
        )?;
        let iter = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in iter.flatten() {
            en_map.entry(title_canonical(&r.0)).or_insert(r.1);
        }
    }

    // For each LP game, look up canonical title and overwrite thumbnail_key
    // when an EN match exists and differs.
    let lp_rows: Vec<(i64, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, title, thumbnail_key FROM games WHERE language != 'EN'",
        )?;
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        iter.flatten().collect()
    };

    let tx = conn.unchecked_transaction()?;
    let mut updated = 0usize;
    {
        let mut upd = tx.prepare_cached("UPDATE games SET thumbnail_key = ?1 WHERE id = ?2")?;
        for (id, title, current) in &lp_rows {
            if let Some(en_hash) = en_map.get(&title_canonical(title)) {
                if current.as_deref() != Some(en_hash) {
                    upd.execute(rusqlite::params![en_hash, id])?;
                    updated += 1;
                }
            }
        }
    }
    tx.commit()?;
    log::info!("propagate_lp_thumbnail_keys: updated {} LP games to share EN cover art", updated);
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
