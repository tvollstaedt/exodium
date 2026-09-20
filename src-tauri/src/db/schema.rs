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
            last_played           TEXT,
            rating_votes          INTEGER,
            music_file            TEXT
        );

        CREATE TABLE IF NOT EXISTS playlists (
            id          INTEGER PRIMARY KEY,
            name        TEXT NOT NULL,
            kind        TEXT NOT NULL DEFAULT 'user',
            slug        TEXT,
            description TEXT,
            UNIQUE (kind, name)
        );

        CREATE TABLE IF NOT EXISTS playlist_games (
            playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            game_id     INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
            position    INTEGER,
            PRIMARY KEY (playlist_id, game_id)
        );

        CREATE INDEX IF NOT EXISTS idx_playlist_games_game ON playlist_games(game_id);

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

        CREATE TABLE IF NOT EXISTS publications (
            id          INTEGER PRIMARY KEY,
            kind        TEXT NOT NULL,
            name        TEXT NOT NULL,
            issue_count INTEGER NOT NULL DEFAULT 0,
            first_year  INTEGER,
            last_year   INTEGER,
            cover_key   TEXT,
            language    TEXT NOT NULL DEFAULT 'EN',
            UNIQUE (kind, name)
        );

        CREATE TABLE IF NOT EXISTS issues (
            id             INTEGER PRIMARY KEY,
            key            TEXT NOT NULL UNIQUE,
            publication_id INTEGER NOT NULL REFERENCES publications(id) ON DELETE CASCADE,
            kind           TEXT NOT NULL,
            title          TEXT NOT NULL,
            sort_title     TEXT,
            year           INTEGER,
            release_date   TEXT,
            publisher      TEXT,
            developer      TEXT,
            notes          TEXT,
            zip_file       TEXT NOT NULL,
            entry_path     TEXT,
            entry_kind     TEXT,
            size_bytes     INTEGER NOT NULL DEFAULT 0,
            cover_key      TEXT,
            runnable       INTEGER NOT NULL DEFAULT 0,
            launch_dir     TEXT,
            issue_dir      TEXT,
            launch_bat     TEXT,
            command_line   TEXT,
            -- eXo's run.bat placeholders for THIS issue, as a JSON object:
            -- a handful of keys on 241 rows, read only by the launcher.
            substitutions  TEXT,
            -- The torrent this issue's archive lives in: a media source id or
            -- a collection id (§19). `inner_zip` names the STORED archive
            -- inside it, if any - the reader windows onto that one.
            source         TEXT NOT NULL DEFAULT 'eXoMedia',
            inner_zip      TEXT,
            language       TEXT NOT NULL DEFAULT 'EN',
            extras_count   INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_issues_publication ON issues(publication_id);
        CREATE INDEX IF NOT EXISTS idx_issues_year ON issues(year);

        -- eXo's per-game article index. Keyed by shortcode and entry path, not
        -- by row id: both sides are replaced wholesale on a catalog refresh.
        CREATE TABLE IF NOT EXISTS game_articles (
            shortcode  TEXT NOT NULL,
            entry_path TEXT NOT NULL,
            kind       TEXT NOT NULL,
            page       INTEGER NOT NULL,
            -- `kind` belongs in the key: eXo indexes one page of one issue
            -- under two kinds for a handful of games (Hints and Review on the
            -- same page), and without it one of the two is dropped.
            PRIMARY KEY (shortcode, entry_path, page, kind)
        );

        -- Per-user reading state. Keyed by issues.key so it survives the
        -- wholesale replace of the catalog tables (the role USER_COLS plays
        -- for games).
        CREATE TABLE IF NOT EXISTS issue_state (
            issue_key   TEXT PRIMARY KEY,
            favorited   INTEGER NOT NULL DEFAULT 0,
            installed   INTEGER NOT NULL DEFAULT 0,
            last_page   INTEGER,
            last_opened TEXT
        );

        ",
    )?;
    Ok(())
}
