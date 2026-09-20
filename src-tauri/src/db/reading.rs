//! Queries over the Lesesaal tables (§19). The catalogue is bundled, so every
//! one of these answers offline; only opening an issue needs the torrent.

use rusqlite::{params, Connection, Row};

use super::DbResult;
use crate::models::{Article, Issue, Publication};

const ISSUE_COLUMNS: &str = "i.id, i.key, i.publication_id, p.name, i.kind, i.title, i.sort_title, \
     i.year, i.release_date, i.publisher, i.developer, i.notes, i.zip_file, i.entry_path, \
     i.entry_kind, i.size_bytes, i.cover_key, i.runnable, i.launch_dir, i.issue_dir, \
     i.launch_bat, i.command_line, i.substitutions, \
     COALESCE(s.favorited, 0), COALESCE(s.installed, 0), \
     s.last_page, s.last_opened, \
     i.source, i.inner_zip, i.language, i.extras_count";

const ISSUE_FROM: &str = "FROM issues i \
     JOIN publications p ON p.id = i.publication_id \
     LEFT JOIN issue_state s ON s.issue_key = i.key";

fn row_to_issue(row: &Row) -> rusqlite::Result<Issue> {
    Ok(Issue {
        id: row.get(0)?,
        key: row.get(1)?,
        publication_id: row.get(2)?,
        publication: row.get(3)?,
        kind: row.get(4)?,
        title: row.get(5)?,
        sort_title: row.get(6)?,
        year: row.get(7)?,
        release_date: row.get(8)?,
        publisher: row.get(9)?,
        developer: row.get(10)?,
        notes: row.get(11)?,
        zip_file: row.get(12)?,
        entry_path: row.get(13)?,
        entry_kind: row.get(14)?,
        size_bytes: row.get(15)?,
        cover_key: row.get(16)?,
        runnable: row.get::<_, i64>(17)? != 0,
        launch_dir: row.get(18)?,
        issue_dir: row.get(19)?,
        launch_bat: row.get(20)?,
        command_line: row.get(21)?,
        substitutions: row.get(22)?,
        favorited: row.get::<_, i64>(23)? != 0,
        installed: row.get::<_, i64>(24)? != 0,
        last_page: row.get(25)?,
        last_opened: row.get(26)?,
        source: row.get(27)?,
        inner_zip: row.get(28)?,
        language: row.get(29)?,
        extras_count: row.get(30)?,
    })
}

pub fn fetch_publications(conn: &Connection, kind: Option<&str>) -> DbResult<Vec<Publication>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, name, issue_count, first_year, last_year, cover_key, language \
         FROM publications WHERE (?1 IS NULL OR kind = ?1) \
         ORDER BY kind, name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([kind], |row| {
        Ok(Publication {
            id: row.get(0)?,
            kind: row.get(1)?,
            name: row.get(2)?,
            issue_count: row.get(3)?,
            first_year: row.get(4)?,
            last_year: row.get(5)?,
            cover_key: row.get(6)?,
            language: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Issues, filtered the way the Lesesaal toolbar filters: by kind, by
/// publication, by free text over title and publication.
pub fn fetch_issues(
    conn: &Connection,
    kind: Option<&str>,
    publication_id: Option<i64>,
    query: Option<&str>,
) -> DbResult<Vec<Issue>> {
    // `%` and `_` are LIKE wildcards: typed into the search box they are
    // literal characters, so they are escaped rather than matched with.
    let pattern = query
        .map(|q| q.trim())
        .filter(|q| !q.is_empty())
        .map(|q| format!("%{}%", q.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_")));
    let sql = format!(
        "SELECT {ISSUE_COLUMNS} {ISSUE_FROM} \
         WHERE (?1 IS NULL OR i.kind = ?1) \
           AND (?2 IS NULL OR i.publication_id = ?2) \
           AND (?3 IS NULL OR i.title LIKE ?3 ESCAPE '\\' OR p.name LIKE ?3 ESCAPE '\\') \
         ORDER BY p.name COLLATE NOCASE, i.year, \
                  COALESCE(i.sort_title, i.title) COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![kind, publication_id, pattern], row_to_issue)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn fetch_issue_by_key(conn: &Connection, key: &str) -> DbResult<Option<Issue>> {
    let sql = format!("SELECT {ISSUE_COLUMNS} {ISSUE_FROM} WHERE i.key = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([key], row_to_issue)?;
    Ok(rows.next().transpose()?)
}

/// The reading state row, created on demand - most issues never have one.
fn ensure_state(conn: &Connection, key: &str) -> DbResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO issue_state (issue_key) VALUES (?1)",
        [key],
    )?;
    Ok(())
}

pub fn set_issue_flag(conn: &Connection, key: &str, flag: &str, value: bool) -> DbResult<()> {
    // The column name is never user input: the command maps a closed set.
    ensure_state(conn, key)?;
    conn.execute(
        &format!("UPDATE issue_state SET {flag} = ?2 WHERE issue_key = ?1"),
        params![key, value as i64],
    )?;
    Ok(())
}

/// A runnable issue's files are on disk and its launcher is ready to run.
pub fn set_issue_installed(conn: &Connection, key: &str, installed: bool) -> DbResult<()> {
    set_issue_flag(conn, key, "installed", installed)
}

/// Remember where the reader stopped. Also stamps `last_opened`, which is what
/// the "continue reading" shelf sorts on.
pub fn set_last_page(conn: &Connection, key: &str, page: i64) -> DbResult<()> {
    ensure_state(conn, key)?;
    conn.execute(
        "UPDATE issue_state SET last_page = ?2, last_opened = datetime('now') WHERE issue_key = ?1",
        params![key, page],
    )?;
    Ok(())
}

/// eXo's article index for one game, newest first. Scoped to the shortcode -
/// the links only ever name eXoDOS games - and matched case-insensitively:
/// the index takes its codes from eXo's directory names, where 18 of them
/// differ from the catalogue only in case (§19).
pub fn articles_for_shortcode(conn: &Connection, shortcode: &str) -> DbResult<Vec<Article>> {
    let mut stmt = conn.prepare(
        "SELECT a.kind, a.page, i.key, i.title, p.name, i.year, i.language \
         FROM game_articles a \
         JOIN issues i ON i.entry_path = a.entry_path \
         JOIN publications p ON p.id = i.publication_id \
         WHERE a.shortcode = ?1 COLLATE NOCASE \
         ORDER BY i.year, i.title, a.page",
    )?;
    let rows = stmt.query_map([shortcode], |row| {
        Ok(Article {
            kind: row.get(0)?,
            page: row.get(1)?,
            issue_key: row.get(2)?,
            issue_title: row.get(3)?,
            publication: row.get(4)?,
            year: row.get(5)?,
            language: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        super::super::schema::create_tables(&conn).unwrap();
        conn.execute(
            "INSERT INTO publications (id, kind, name, issue_count) VALUES (1, 'magazine', 'PC Gamer', 2)",
            [],
        )
        .unwrap();
        for (id, key, title, entry, year) in [
            (1, "mag:a", "PC Gamer 1995-02", "eXo/Magazines/PCGamerUS/PCGamer_1995_02.pdf", 1995),
            (2, "mag:b", "PC Gamer 1996-04", "eXo/Magazines/PCGamerUS/PCGamer_1996_04.pdf", 1996),
        ] {
            conn.execute(
                "INSERT INTO issues (id, key, publication_id, kind, title, year, zip_file, entry_path, entry_kind, size_bytes) \
                 VALUES (?1, ?2, 1, 'magazine', ?3, ?4, 'Content/DOSMagazines.zip', ?5, 'pdf', 1000)",
                params![id, key, title, year, entry],
            )
            .unwrap();
        }
        // The GLP addon: a second source, read through a STORED archive
        // inside the collection's own zip (§19).
        conn.execute(
            "INSERT INTO publications (id, kind, name, issue_count, language) \
             VALUES (2, 'magazine', 'ASM (DE)', 1, 'DE')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO issues (id, key, publication_id, kind, title, year, zip_file, entry_path, \
                 entry_kind, size_bytes, source, inner_zip, language, extras_count) \
             VALUES (3, 'mag:de', 2, 'magazine', 'ASM 1986-03', 1986, \
                 'Content/eXoDOS_GLP_Addonpack_MagazinesGLP.zip', \
                 'eXo/Magazines/!german/ASM/ASM 1986-03.pdf', 'pdf', 2000, \
                 'eXoDOS_GLP', 'Content/inner.zip', 'DE', 3)",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn issue_state_defaults_to_unread_and_survives_flags() {
        let conn = db();
        let issue = fetch_issue_by_key(&conn, "mag:a").unwrap().unwrap();
        assert!(!issue.favorited && issue.last_page.is_none());

        set_issue_flag(&conn, "mag:a", "favorited", true).unwrap();
        set_issue_installed(&conn, "mag:a", true).unwrap();
        set_last_page(&conn, "mag:a", 12).unwrap();
        let issue = fetch_issue_by_key(&conn, "mag:a").unwrap().unwrap();
        assert!(issue.favorited && issue.installed);
        assert_eq!(issue.last_page, Some(12));
    }

    #[test]
    fn articles_resolve_to_the_issue_that_holds_the_page() {
        let conn = db();
        conn.execute(
            "INSERT INTO game_articles (shortcode, entry_path, kind, page) \
             VALUES ('ultima8', 'eXo/Magazines/PCGamerUS/PCGamer_1996_04.pdf', 'Review', 143)",
            [],
        )
        .unwrap();
        let articles = articles_for_shortcode(&conn, "ultima8").unwrap();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].issue_key, "mag:b");
        assert_eq!(articles[0].page, 143);
        assert_eq!(articles[0].publication, "PC Gamer");
        assert!(articles_for_shortcode(&conn, "zork1").unwrap().is_empty());
    }

    /// The source and the window into it travel with the row: the reader has
    /// no other way to learn which torrent holds an issue (§19).
    #[test]
    fn a_second_source_round_trips_its_archive_and_language() {
        let conn = db();
        let en = fetch_issue_by_key(&conn, "mag:a").unwrap().unwrap();
        assert_eq!(en.source, "eXoMedia");
        assert_eq!(en.inner_zip, None);
        assert_eq!(en.language, "EN");

        let de = fetch_issue_by_key(&conn, "mag:de").unwrap().unwrap();
        assert_eq!(de.source, "eXoDOS_GLP");
        assert_eq!(de.inner_zip.as_deref(), Some("Content/inner.zip"));
        assert_eq!(de.language, "DE");
        assert_eq!(de.extras_count, 3);

        conn.execute(
            "INSERT INTO game_articles (shortcode, entry_path, kind, page) \
             VALUES ('ultima8', 'eXo/Magazines/!german/ASM/ASM 1986-03.pdf', 'Review', 61)",
            [],
        )
        .unwrap();
        let articles = articles_for_shortcode(&conn, "ultima8").unwrap();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].language, "DE");
        assert_eq!(articles[0].issue_key, "mag:de");
    }

    #[test]
    fn filters_match_kind_publication_and_free_text() {
        let conn = db();
        assert_eq!(fetch_issues(&conn, None, None, None).unwrap().len(), 3);
        assert_eq!(fetch_issues(&conn, Some("book"), None, None).unwrap().len(), 0);
        assert_eq!(fetch_issues(&conn, None, Some(1), None).unwrap().len(), 2);
        assert_eq!(fetch_issues(&conn, None, None, Some("1996")).unwrap().len(), 1);
        // A blank search is not a filter.
        assert_eq!(fetch_issues(&conn, None, None, Some("  ")).unwrap().len(), 3);
    }

    /// eXo's index takes its codes from directory names, whose casing is not
    /// the catalogue's: 18 codes differ in case alone.
    #[test]
    fn a_shortcode_matches_whatever_case_the_index_wrote_it_in() {
        let conn = db();
        conn.execute(
            "INSERT INTO game_articles (shortcode, entry_path, kind, page) \
             VALUES ('Zak', 'eXo/Magazines/PCGamerUS/PCGamer_1995_02.pdf', 'Review', 12)",
            [],
        )
        .unwrap();
        assert_eq!(articles_for_shortcode(&conn, "zak").unwrap().len(), 1);
        assert_eq!(articles_for_shortcode(&conn, "ZAK").unwrap().len(), 1);
        assert!(articles_for_shortcode(&conn, "zak2").unwrap().is_empty());
    }

    /// One page can carry two kinds of entry for the same game, and both are
    /// offered - the key has to tell them apart.
    #[test]
    fn two_kinds_on_one_page_are_two_articles() {
        let conn = db();
        for kind in ["Hints", "Review"] {
            conn.execute(
                "INSERT OR IGNORE INTO game_articles (shortcode, entry_path, kind, page) \
                 VALUES ('duskgod', 'eXo/Magazines/PCGamerUS/PCGamer_1996_04.pdf', ?1, 90)",
                [kind],
            )
            .unwrap();
        }
        let mut kinds: Vec<String> = articles_for_shortcode(&conn, "duskgod")
            .unwrap()
            .into_iter()
            .map(|a| a.kind)
            .collect();
        kinds.sort();
        assert_eq!(kinds, vec!["Hints".to_string(), "Review".to_string()]);
    }

    /// Typed into the search box, a LIKE wildcard is a character like any
    /// other - matching everything on `%` looks like the filter is broken.
    #[test]
    fn like_wildcards_in_the_query_are_literal() {
        let conn = db();
        assert!(fetch_issues(&conn, None, None, Some("%")).unwrap().is_empty());
        assert!(fetch_issues(&conn, None, None, Some("PC_Gamer")).unwrap().is_empty());
        conn.execute(
            "INSERT INTO issues (id, key, publication_id, kind, title, zip_file, entry_path, \
                 entry_kind, size_bytes) \
             VALUES (4, 'mag:pct', 1, 'magazine', '100% PC Gamer', 'Content/DOSMagazines.zip', \
                 'eXo/Magazines/PCGamerUS/pct.pdf', 'pdf', 1000)",
            [],
        )
        .unwrap();
        assert_eq!(fetch_issues(&conn, None, None, Some("0% PC")).unwrap().len(), 1);
    }
}
