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

/// Open (or create) the Exodian database at the given path.
pub fn open(path: &Path) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    Ok(conn)
}

/// Initialize the database schema (idempotent).
pub fn init(conn: &Connection) -> DbResult<()> {
    schema::create_tables(conn)?;
    Ok(())
}
