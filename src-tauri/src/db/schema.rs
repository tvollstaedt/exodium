use rusqlite::Connection;

use super::DbResult;

pub fn create_tables(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS games (
            id                    INTEGER PRIMARY KEY,
            title                 TEXT NOT NULL,
            sort_title            TEXT,
            platform              TEXT NOT NULL DEFAULT 'MS-DOS',
            developer             TEXT,
            publisher             TEXT,
            release_date          TEXT,
            year                  INTEGER,
            genre                 TEXT,
            series                TEXT,
            play_mode             TEXT,
            rating                REAL,
            description           TEXT,
            notes                 TEXT,
            source                TEXT,
            application_path      TEXT,
            dosbox_conf           TEXT,
            status                TEXT,
            region                TEXT,
            max_players           INTEGER,
            language              TEXT NOT NULL DEFAULT 'EN',
            shortcode             TEXT,
            torrent_source        TEXT,
            in_library            INTEGER NOT NULL DEFAULT 0,
            installed             INTEGER NOT NULL DEFAULT 0,
            favorited             INTEGER NOT NULL DEFAULT 0,
            game_torrent_index    INTEGER,
            gamedata_torrent_index INTEGER,
            download_size         INTEGER,
            has_thumbnail         INTEGER NOT NULL DEFAULT 0,
            dosbox_variant        TEXT,
            thumbnail_key         TEXT,
            manual_path           TEXT,
            last_played           TEXT
        );

        CREATE TABLE IF NOT EXISTS downloads (
            id             INTEGER PRIMARY KEY,
            game_id        INTEGER REFERENCES games(id) ON DELETE CASCADE,
            torrent_index  INTEGER NOT NULL,
            file_type      TEXT NOT NULL,
            file_name      TEXT NOT NULL,
            file_size      INTEGER NOT NULL,
            status         TEXT NOT NULL DEFAULT 'pending',
            progress       REAL NOT NULL DEFAULT 0,
            error          TEXT
        );

        CREATE TABLE IF NOT EXISTS images (
            id      INTEGER PRIMARY KEY,
            game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
            type    TEXT NOT NULL,
            path    TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS playlists (
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS playlist_games (
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            game_id     INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
            PRIMARY KEY (playlist_id, game_id)
        );

        CREATE TABLE IF NOT EXISTS config (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_games_title ON games(title);
        CREATE INDEX IF NOT EXISTS idx_games_year ON games(year);
        CREATE INDEX IF NOT EXISTS idx_games_genre ON games(genre);
        CREATE INDEX IF NOT EXISTS idx_games_language ON games(language);
        CREATE INDEX IF NOT EXISTS idx_games_shortcode ON games(shortcode);
        CREATE INDEX IF NOT EXISTS idx_games_installed ON games(installed);
        CREATE TABLE IF NOT EXISTS game_config (
            game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
            key     TEXT NOT NULL,
            value   TEXT NOT NULL,
            PRIMARY KEY (game_id, key)
        );

        CREATE INDEX IF NOT EXISTS idx_images_game_id ON images(game_id);
        CREATE INDEX IF NOT EXISTS idx_downloads_game_id ON downloads(game_id);
        CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
        ",
    )?;
    Ok(())
}
