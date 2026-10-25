//! Playlists as files (#43): a list of titles matched against the catalogue,
//! a playlist written back out in the same CSV shape, and the variants
//! "Download all" would fetch.
//!
//! Membership is per GROUP (all language variants of one game) and sits on the
//! group's primary row, so every function here speaks in groups.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::db::queries::{self, family_expr};
use crate::db::DbResult;
use crate::models::Game;

/// One title from an imported list; `line` is 1-based for the preview.
#[derive(Debug, Clone, PartialEq)]
pub struct ListEntry {
    pub line: usize,
    pub title: String,
    pub collection: Option<String>,
    pub year: Option<i32>,
}

/// A game the entry could mean: the group's primary row.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Candidate {
    pub game_id: i64,
    pub title: String,
    /// The pack family (`eXoDOS` for the language packs too).
    pub collection: String,
    pub year: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct ImportRow {
    pub line: usize,
    pub title: String,
    /// None found, one, or several for the user to pick from (eXoDOS first).
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize)]
pub struct ImportPreview {
    /// The file's name, proposed as the playlist's.
    pub name: String,
    pub rows: Vec<ImportRow>,
}

#[derive(Debug, Serialize)]
pub struct PlanItem {
    pub game_id: i64,
    pub title: String,
    pub size_bytes: i64,
    pub torrent_source: Option<String>,
    pub thumbnail_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DownloadPlan {
    pub items: Vec<PlanItem>,
    pub total_bytes: i64,
    /// Groups skipped as already installed / already downloading.
    pub present: usize,
    pub in_progress: usize,
    /// The languages the playlist's games come in, EN first.
    pub languages: Vec<String>,
    pub free_bytes: Option<u64>,
}

// ── Reading a list ───────────────────────────────────────────────────────────

/// UTF-8 (BOM tolerated), else Windows-1252: what Notepad and Excel save.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes
            .iter()
            .map(|&b| match b {
                0x91 | 0x92 => '\'',
                0x93 | 0x94 => '"',
                0x96 | 0x97 => '-',
                _ => b as char,
            })
            .collect(),
    }
}

/// A `.csv` (`title`, optional `collection`, `year`; header optional, `;`
/// as Excel writes it in many locales) or one title per line.
pub fn parse_list(bytes: &[u8], is_csv: bool) -> Vec<ListEntry> {
    let text = decode(bytes);
    if is_csv { parse_csv(&text) } else { parse_lines(&text) }
}

fn parse_lines(text: &str) -> Vec<ListEntry> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let title = line.trim();
            (!title.is_empty() && !title.starts_with('#')).then(|| ListEntry {
                line: i + 1,
                title: title.to_string(),
                collection: None,
                year: None,
            })
        })
        .collect()
}

/// Excel writes `;` in many locales, and a quoted title may carry commas of
/// its own - only separators outside double quotes count.
fn csv_delimiter(first_line: &str) -> u8 {
    let (mut semicolons, mut commas, mut quoted) = (0, 0, false);
    for c in first_line.chars() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => semicolons += 1,
            ',' if !quoted => commas += 1,
            _ => {}
        }
    }
    if semicolons > commas { b';' } else { b',' }
}

fn parse_csv(text: &str) -> Vec<ListEntry> {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let delimiter = csv_delimiter(first);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let (mut title_col, mut collection_col, mut year_col) = (0, Some(1), Some(2));
    let mut out = Vec::new();
    for (i, record) in reader.records().enumerate() {
        let Ok(record) = record else { continue };
        let field = |col: Option<usize>| {
            col.and_then(|c| record.get(c)).map(str::trim).filter(|s| !s.is_empty())
        };
        if i == 0 && field(Some(0)).is_some_and(|f| f.eq_ignore_ascii_case("title")) {
            let find = |name: &str| record.iter().position(|h| h.trim().eq_ignore_ascii_case(name));
            title_col = find("title").unwrap_or(0);
            collection_col = find("collection");
            year_col = find("year");
            continue;
        }
        let Some(title) = field(Some(title_col)).filter(|t| !t.starts_with('#')) else { continue };
        out.push(ListEntry {
            line: record.position().map_or(i + 1, |p| p.line() as usize),
            title: title.to_string(),
            collection: field(collection_col).map(str::to_string),
            year: field(year_col).and_then(|y| y.parse().ok()),
        });
    }
    out
}

/// "Title (1994)" → ("Title", 1994). Only tried when the full text matches
/// nothing: some catalogue titles end in parentheses themselves.
fn split_trailing_year(title: &str) -> Option<(&str, i32)> {
    let open = title.strip_suffix(')')?.rfind('(')?;
    let year: i32 = title[open + 1..title.len() - 1].trim().parse().ok()?;
    (1970..=2100).contains(&year).then(|| (title[..open].trim_end(), year))
}

/// Case, typographic quotes, runs of whitespace and an article placed either
/// way ("The Oregon Trail", "Oregon Trail, The") do not count.
fn normalize(title: &str) -> String {
    let mapped: String = title
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '`' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            _ => c,
        })
        .collect();
    let joined = mapped.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let bare = joined.strip_prefix("the ").unwrap_or(&joined);
    bare.strip_suffix(", the").unwrap_or(bare).to_string()
}

// ── Matching ─────────────────────────────────────────────────────────────────

struct Group {
    primary: Candidate,
    years: Vec<i32>,
}

/// Every group of the catalogue, findable by any of its variants' titles -
/// and, when nothing has that title, by the part before a subtitle, so
/// "Commander Keen 4" finds "Commander Keen 4: Secret of the Oracle".
pub struct CatalogIndex {
    groups: Vec<Group>,
    by_title: HashMap<String, Vec<usize>>,
    by_main_title: HashMap<String, Vec<usize>>,
}

fn add_to(map: &mut HashMap<String, Vec<usize>>, key: String, group: usize) {
    let slot = map.entry(key).or_default();
    if !slot.contains(&group) {
        slot.push(group);
    }
}

fn lookup(map: &HashMap<String, Vec<usize>>, title: &str) -> Vec<usize> {
    map.get(&normalize(title)).cloned().unwrap_or_default()
}

impl CatalogIndex {
    pub fn load(conn: &Connection) -> DbResult<Self> {
        let sql = format!(
            "SELECT g.id, g.title, {}, g.shortcode, g.year FROM games g \
             ORDER BY CASE WHEN g.language = 'EN' THEN 0 ELSE 1 END, g.id",
            family_expr("g")
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i32>>(4)?,
            ))
        })?;
        let mut index = CatalogIndex { groups: Vec::new(), by_title: HashMap::new(), by_main_title: HashMap::new() };
        let mut by_key: HashMap<(String, String), usize> = HashMap::new();
        for row in rows {
            let (id, title, family, shortcode, year) = row?;
            // EN rows come first, so the first row of a group is its primary.
            let key = (family.clone(), shortcode.unwrap_or_else(|| format!("#{id}")));
            let gi = *by_key.entry(key).or_insert_with(|| {
                index.groups.push(Group {
                    primary: Candidate { game_id: id, title: title.clone(), collection: family, year },
                    years: Vec::new(),
                });
                index.groups.len() - 1
            });
            if let Some(y) = year {
                index.groups[gi].years.push(y);
            }
            if let Some((main, _)) = title.split_once(':') {
                add_to(&mut index.by_main_title, normalize(main), gi);
            }
            add_to(&mut index.by_title, normalize(&title), gi);
        }
        Ok(index)
    }

    /// The groups `entry` can mean. A named collection that has none of them
    /// leaves nothing; a year narrows only when it leaves something.
    pub fn candidates(&self, entry: &ListEntry) -> Vec<Candidate> {
        let split = split_trailing_year(&entry.title);
        let tries = std::iter::once((entry.title.as_str(), None)).chain(split.map(|(t, y)| (t, Some(y))));
        let mut year = entry.year;
        let mut found = Vec::new();
        for (title, y) in tries {
            found = lookup(&self.by_title, title);
            if found.is_empty() {
                found = lookup(&self.by_main_title, title);
            }
            if !found.is_empty() {
                year = year.or(y);
                break;
            }
        }
        if let Some(family) = entry.collection.as_deref().and_then(resolve_family) {
            found.retain(|&g| self.groups[g].primary.collection == family);
        }
        if let Some(y) = year {
            let narrowed: Vec<usize> =
                found.iter().copied().filter(|&g| self.groups[g].years.contains(&y)).collect();
            if !narrowed.is_empty() {
                found = narrowed;
            }
        }
        let mut out: Vec<Candidate> = found.into_iter().map(|g| self.groups[g].primary.clone()).collect();
        out.sort_by_key(|c| (family_rank(&c.collection), c.year, c.game_id));
        out
    }
}

/// The badge labels the UI shows, beside ids, display names and platforms.
const FAMILY_ALIASES: [(&str, &str); 4] =
    [("DOS", "eXoDOS"), ("Win3x", "eXoWin3x"), ("Win9x", "eXoWin9x"), ("IIGS", "eXoAppleIIGS")];

/// A collection as a list names it, as its pack family; None if unknown.
fn resolve_family(hint: &str) -> Option<&'static str> {
    let hint = hint.trim();
    if let Some((_, family)) = FAMILY_ALIASES.iter().find(|(a, _)| a.eq_ignore_ascii_case(hint)) {
        return Some(family);
    }
    crate::COLLECTION_MAP
        .iter()
        .find(|c| {
            [c.id, c.display_name, c.platform].iter().any(|n| n.eq_ignore_ascii_case(hint))
        })
        .map(|c| crate::collection_base_id(c.id))
}

/// Families in COLLECTION_MAP order, which puts eXoDOS first.
fn family_rank(family: &str) -> usize {
    crate::COLLECTION_MAP
        .iter()
        .position(|c| crate::collection_base_id(c.id) == family)
        .unwrap_or(usize::MAX)
}

pub fn preview(index: &CatalogIndex, entries: &[ListEntry]) -> Vec<ImportRow> {
    entries
        .iter()
        .map(|e| ImportRow { line: e.line, title: e.title.clone(), candidates: index.candidates(e) })
        .collect()
}

// ── Writing a list ───────────────────────────────────────────────────────────

/// The playlist as `title,collection,year`, one row per group, in the order
/// the games were added - `parse_list` reads it back to the same groups.
pub fn export_csv(conn: &Connection, playlist_id: i64) -> Result<Vec<u8>, String> {
    let sql = format!(
        "SELECT m.title, {}, m.year, m.shortcode FROM playlist_games pg \
         JOIN games m ON m.id = pg.game_id WHERE pg.playlist_id = ?1 \
         ORDER BY pg.position, m.id",
        family_expr("m")
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![playlist_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i32>>(2)?, r.get::<_, Option<String>>(3)?))
        })
        .map_err(|e| e.to_string())?;
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(["title", "collection", "year"]).map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let (title, family, year, shortcode) = row.map_err(|e| e.to_string())?;
        if shortcode.is_some_and(|s| !seen.insert((family.clone(), s))) {
            continue;
        }
        let year = year.map(|y| y.to_string()).unwrap_or_default();
        writer.write_record([title.as_str(), family.as_str(), year.as_str()]).map_err(|e| e.to_string())?;
    }
    writer.into_inner().map_err(|e| e.to_string())
}

// ── Download all ─────────────────────────────────────────────────────────────

/// Per group of the playlist: the variant to fetch, or why there is none.
pub enum Pick {
    Fetch(Box<Game>),
    Present,
    InProgress,
}

/// `language` names the version wanted; EN means "whichever the user has",
/// so a game installed in any language counts as present. Any other language
/// counts only its own variant - that is what asking for it means - and
/// falls back to EN where the game has no such version.
pub fn pick_variant(variants: &[Game], language: &str) -> Option<Pick> {
    let wanted = (!language.eq_ignore_ascii_case("EN"))
        .then(|| variants.iter().find(|v| v.language.eq_ignore_ascii_case(language)))
        .flatten();
    let target = wanted.or_else(|| variants.first())?;
    let (present, active) = match wanted {
        Some(v) => (v.installed, v.in_library),
        None => (variants.iter().any(|v| v.installed), variants.iter().any(|v| v.in_library)),
    };
    Some(if present {
        Pick::Present
    } else if active {
        Pick::InProgress
    } else {
        Pick::Fetch(Box::new(target.clone()))
    })
}

/// The playlist's groups as variant lists, EN first; `priced` adds what the
/// panel's language picker charges (the overlay base), at a cost per group.
pub fn playlist_groups(conn: &Connection, playlist_id: i64, priced: bool) -> Result<Vec<Vec<Game>>, String> {
    let ids: Vec<i64> = conn
        .prepare_cached("SELECT game_id FROM playlist_games WHERE playlist_id = ?1 ORDER BY position")
        .and_then(|mut stmt| stmt.query_map(params![playlist_id], |r| r.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut groups = Vec::new();
    for id in ids {
        let Some(member) = queries::fetch_game_by_id(conn, id).map_err(|e| e.to_string())? else { continue };
        let variants = match member.shortcode.as_deref() {
            Some(sc) => {
                let source = member.torrent_source.as_deref().unwrap_or("eXoDOS");
                if !seen.insert((crate::collection_base_id(source), sc.to_string())) {
                    continue;
                }
                if priced {
                    super::games::variants_with_overlay_prices(conn, sc, source)?
                } else {
                    queries::fetch_game_variants(conn, sc, source).map_err(|e| e.to_string())?
                }
            }
            None => vec![member],
        };
        groups.push(variants);
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> ListEntry {
        ListEntry { line: 1, title: title.into(), collection: None, year: None }
    }

    #[test]
    fn text_lists_skip_blanks_and_comments() {
        let parsed = parse_list(b"\xEF\xBB\xBFDoom\n\n# kids\n  Commander Keen  \r\n", false);
        assert_eq!(parsed.iter().map(|e| (e.line, e.title.as_str())).collect::<Vec<_>>(), [(1, "Doom"), (4, "Commander Keen")]);
    }

    #[test]
    fn csv_reads_quoted_titles_headers_and_excel_semicolons() {
        let parsed = parse_list(b"title,year,collection\n\"Monkey Island 2: LeChuck's Revenge, The\",1991,eXoScummVM\n", true);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Monkey Island 2: LeChuck's Revenge, The");
        assert_eq!(parsed[0].collection.as_deref(), Some("eXoScummVM"));
        assert_eq!(parsed[0].year, Some(1991));

        let excel = parse_list(b"Doom;DOS\nMyst;Win3x;1993\n", true);
        assert_eq!(excel[1].title, "Myst");
        assert_eq!(excel[1].collection.as_deref(), Some("Win3x"));
        assert_eq!(excel[1].year, Some(1993));
    }

    #[test]
    fn a_comma_inside_a_quoted_title_does_not_pick_the_delimiter() {
        let parsed = parse_list(b"\"Secret of Monkey Island, The\";DOS\nDoom;DOS;1993\n", true);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].title, "Secret of Monkey Island, The");
        assert_eq!(parsed[0].collection.as_deref(), Some("DOS"));
        assert_eq!(parsed[1].title, "Doom");
        assert_eq!(parsed[1].year, Some(1993));
        assert_eq!(csv_delimiter("\"A, B\",1994"), b',');
    }

    #[test]
    fn a_windows_1252_list_decodes() {
        let parsed = parse_list(b"Die Siedler \xE4\nTorin\x92s Passage\n", false);
        assert_eq!(parsed[0].title, "Die Siedler ä");
        assert_eq!(parsed[1].title, "Torin's Passage");
    }

    #[test]
    fn a_trailing_year_is_split_only_when_it_is_one() {
        assert_eq!(split_trailing_year("Doom (1993)"), Some(("Doom", 1993)));
        assert_eq!(split_trailing_year("Ultima VII (Serpent Isle)"), None);
        assert_eq!(split_trailing_year("Doom"), None);
    }

    #[test]
    fn collections_resolve_by_id_name_platform_and_badge() {
        assert_eq!(resolve_family("eXoWin3x"), Some("eXoWin3x"));
        assert_eq!(resolve_family("German Language Pack"), Some("eXoDOS"));
        assert_eq!(resolve_family("ms-dos"), Some("eXoDOS"));
        assert_eq!(resolve_family("IIGS"), Some("eXoAppleIIGS"));
        assert_eq!(resolve_family("Amiga"), None);
    }

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        let rows = [
            ("Phantasmagoria", "EN", "eXoDOS", "Phantas", 1995),
            ("Phantasmagoria", "EN", "eXoWin3x", "Phantas", 1995),
            ("Phantasmagoria", "EN", "eXoScummVM", "Phantasmagoria (Multi-Platform)", 1995),
            ("The Settlers", "EN", "eXoDOS", "Settlers", 1993),
            ("Die Siedler", "DE", "eXoDOS_GLP", "Settlers", 1993),
            ("Out of This World", "EN", "eXoDOS", "OOTW", 1991),
            ("Out of This World", "EN", "eXoDOS", "OOTW2", 1992),
            ("Commander Keen 4: Secret of the Oracle", "EN", "eXoDOS", "Keen4", 1991),
            ("The Oregon Trail", "EN", "eXoDOS", "Oregon", 1990),
        ];
        for (title, lang, source, sc, year) in rows {
            conn.execute(
                "INSERT INTO games (title, platform, language, torrent_source, shortcode, year) \
                 VALUES (?1, 'MS-DOS', ?2, ?3, ?4, ?5)",
                params![title, lang, source, sc, year],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn titles_match_across_languages_and_collections() {
        let conn = catalog();
        let index = CatalogIndex::load(&conn).unwrap();

        let siedler = index.candidates(&entry("die  SIEDLER"));
        assert_eq!(siedler.len(), 1, "a localized title finds its group");
        assert_eq!(siedler[0].title, "The Settlers", "and hands back the primary row");

        let phantas = index.candidates(&entry("Phantasmagoria"));
        let families: Vec<&str> = phantas.iter().map(|c| c.collection.as_str()).collect();
        assert_eq!(families, ["eXoDOS", "eXoWin3x", "eXoScummVM"], "eXoDOS first");

        let named = ListEntry { collection: Some("ScummVM".into()), ..entry("Phantasmagoria") };
        assert_eq!(index.candidates(&named).len(), 1);
        let elsewhere = ListEntry { collection: Some("eXoWin9x".into()), ..entry("Phantasmagoria") };
        assert!(index.candidates(&elsewhere).is_empty(), "a named collection without it finds nothing");

        assert_eq!(index.candidates(&entry("Out of This World (1992)"))[0].year, Some(1992));
        let wrong_year = ListEntry { year: Some(2001), ..entry("The Settlers") };
        assert_eq!(index.candidates(&wrong_year).len(), 1, "a year that fits nothing is ignored");
        assert!(index.candidates(&entry("Not A Game")).is_empty());

        assert_eq!(index.candidates(&entry("Commander Keen 4")).len(), 1, "the title before its subtitle");
        assert_eq!(index.candidates(&entry("Oregon Trail, The")).len(), 1, "the article either way");
        assert_eq!(index.candidates(&entry("oregon trail")).len(), 1);
    }

    #[test]
    fn an_export_reads_back_to_the_same_games() {
        let conn = catalog();
        let pid = queries::create_playlist(&conn, "Mix").unwrap();
        for title in ["Phantasmagoria", "The Settlers"] {
            let id: i64 = conn
                .query_row("SELECT id FROM games WHERE title = ?1 ORDER BY id DESC LIMIT 1", params![title], |r| r.get(0))
                .unwrap();
            queries::set_playlist_membership(&conn, pid, id, true).unwrap();
        }
        let csv = export_csv(&conn, pid).unwrap();
        let index = CatalogIndex::load(&conn).unwrap();
        let back: Vec<Vec<Candidate>> = parse_list(&csv, true).iter().map(|e| index.candidates(e)).collect();
        assert!(back.iter().all(|c| c.len() == 1), "every exported row is unambiguous: {back:?}");
        assert_eq!(back[0][0].collection, "eXoScummVM");
        assert_eq!(back[1][0].title, "The Settlers");
    }

    fn variant(lang: &str, installed: bool, in_library: bool) -> Game {
        Game { language: lang.into(), installed, in_library, ..Game::default() }
    }

    #[test]
    fn download_all_fetches_the_asked_language_and_falls_back_to_en() {
        let fetched = |p: Option<Pick>| match p {
            Some(Pick::Fetch(g)) => Some(g.language),
            _ => None,
        };
        let en_de = [variant("EN", false, false), variant("DE", false, false)];
        assert_eq!(fetched(pick_variant(&en_de, "DE")).as_deref(), Some("DE"));
        assert_eq!(fetched(pick_variant(&en_de, "ES")).as_deref(), Some("EN"), "no Spanish version: EN");

        let de_installed = [variant("EN", false, false), variant("DE", true, false)];
        assert!(matches!(pick_variant(&de_installed, "EN"), Some(Pick::Present)), "EN means any version");
        let en_installed = [variant("EN", true, false), variant("DE", false, false)];
        assert_eq!(fetched(pick_variant(&en_installed, "DE")).as_deref(), Some("DE"), "asking for DE wants DE");

        let running = [variant("EN", false, false), variant("DE", false, true)];
        assert!(matches!(pick_variant(&running, "EN"), Some(Pick::InProgress)), "a sibling download counts");
        assert_eq!(fetched(pick_variant(&running, "ES")).as_deref(), None);
    }
}
