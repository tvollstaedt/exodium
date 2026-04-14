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

    Ok(())
}
