//! Build tool: generates the pre-built exodium.db from bundled XML metadata + torrents.
//!
//! Run from src-tauri/:  cargo run --example generate_db
//!
//! An EXAMPLE, not a `[[bin]]`: the Tauri bundler copies every bin target of
//! the crate into the installer, so this 6 MB developer tool was shipping to
//! users in /usr/bin next to the app. `required-features` does not help - the
//! bundler still demands the file and fails the build when Cargo skips it.

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rusqlite::params;

use exodium_lib::db;
use exodium_lib::game_name_from_app_path;
use exodium_lib::torrent_search_names;
use exodium_lib::import::xml::parse_games_xml;
use exodium_lib::torrent::TorrentIndex;
use exodium_lib::COLLECTION_MAP;

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Normalize a title for fuzzy matching.
fn normalize_title(title: &str) -> String {
    let mut t = title.to_lowercase();

    // Move trailing ", The" / ", A" / ", An" to front
    for article in &[", the", ", a", ", an"] {
        if t.ends_with(article) {
            let prefix = &article[2..]; // "the " / "a " / "an "
            t = format!("{} {}", prefix, &t[..t.len() - article.len()]);
            break;
        }
    }

    // Normalize punctuation to spaces
    t = t.replace([':', '-', ','], " ").replace('&', " and ");
    t = t.replace('\'', "").replace(['!', '.'], " ");

    // Strip trailing year suffix like " (1993)"
    if let Some(idx) = t.rfind('(') {
        let suffix = &t[idx..];
        if suffix.len() <= 7 && suffix.ends_with(')') {
            t = t[..idx].to_string();
        }
    }

    // Strip trailing series number: "gobliiins 1" → "gobliiins"
    // Only strip if digits are preceded by a space (avoids mangling titles like "1942")
    let t = t.trim_end().to_string();
    let t = {
        let stripped = t.trim_end_matches(|c: char| c.is_ascii_digit());
        if stripped.ends_with(' ') {
            stripped.trim_end().to_string()
        } else {
            t
        }
    };

    // Strip common edition suffixes
    let t = t
        .replace(" deluxe edition", "")
        .replace(" gold edition", "")
        .replace(" special edition", "");

    // Collapse whitespace
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalize a title for thumbnail-key hashing: lowercase + keep only ASCII
/// alphanumerics. Must match the Python implementation in gen_thumbnails.py
/// and the lib implementation in src-tauri/src/db/mod.rs::title_thumbnail_key.
///
/// The stripped-alnum rule means "3-K Trivia", "3K Trivia", and "3 k trivia"
/// all hash to the same filename - punctuation and spacing drift across XML
/// vs zip vs image filenames no longer breaks lookup.
fn thumbnail_key(title: &str) -> String {
    use sha2::{Digest, Sha256};
    let norm: String = title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let hash = format!("{:x}", Sha256::digest(norm.as_bytes()));
    hash[..16].to_string()
}

/// Generate a unique shortcode from a game title.
/// Produces codes like "ACCEsina", "5razy5", "1939" matching the eXoDOS style.
fn generate_shortcode(
    title: &str,
    existing: &std::collections::HashSet<String>,
) -> String {
    // Keep only alphanumeric chars, take up to 8
    let base: String = title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();

    let base = if base.is_empty() {
        "game".to_string()
    } else {
        base
    };

    if !existing.contains(&base) {
        return base;
    }

    // Append incrementing suffix until unique
    for i in 2..=999 {
        let candidate = format!("{}{}", &base[..base.len().min(6)], i);
        if !existing.contains(&candidate) {
            return candidate;
        }
    }

    // Extremely unlikely fallback
    format!("g{}", existing.len())
}

/// Match imported games to their torrent file indices.
fn match_torrent_indices(
    conn: &rusqlite::Connection,
    index: &TorrentIndex,
    torrent_source: &str,
    shared_gamedata_index: Option<&TorrentIndex>,
) {
    let mut matched = 0usize;
    let mut unmatched = 0usize;

    let mut stmt = conn
        .prepare(
            "SELECT id, title, application_path, year FROM games WHERE game_torrent_index IS NULL",
        )
        .unwrap();
    let games: Vec<(i64, String, Option<String>, Option<i64>)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut update_stmt = tx
            .prepare_cached(
                "UPDATE games SET game_torrent_index = ?1, gamedata_torrent_index = ?2,
                 download_size = ?3, torrent_source = ?4 WHERE id = ?5",
            )
            .unwrap();

        for (id, title, app_path, year) in &games {
            let search_names = torrent_search_names(title, app_path.as_deref(), *year);
            let search_name = search_names[0].clone();
            let (game_entry, gamedata_entry) = search_names
                .iter()
                .map(|name| index.find_game_files(name))
                .find(|(game, _)| game.is_some())
                .unwrap_or((None, None));

            if let Some(game) = game_entry {
                let gamedata_idx = gamedata_entry.map(|g| g.index as i64);
                let mut size =
                    game.size as i64 + gamedata_entry.map(|g| g.size as i64).unwrap_or(0);

                // For LP games, add shared EN GameData size from eXoDOS torrent
                if let Some(shared_idx) = shared_gamedata_index {
                    let (_, shared_gd) = shared_idx.find_game_files(&search_name);
                    if let Some(gd) = shared_gd {
                        size += gd.size as i64;
                    }
                }

                update_stmt
                    .execute(params![game.index as i64, gamedata_idx, size, torrent_source, id])
                    .unwrap();
                matched += 1;
            } else {
                unmatched += 1;
            }
        }
    }
    tx.commit().unwrap();

    println!(
        "  Torrent match ({}): {} matched, {} unmatched",
        torrent_source, matched, unmatched
    );
}

/// First text content of `<tag>` in `block`, or None for missing/empty tags.
fn extract_tag<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    let val = block[start..end].trim();
    if val.is_empty() { None } else { Some(val) }
}

fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse metadata/Playlists.xml.gz and seed kind='curated' playlist rows.
///
/// The file is a concatenation of one LaunchBox XML document per playlist
/// (each with its own <?xml?> declaration), so a strict XML parser rejects
/// it - blocks are split on <LaunchBox> instead. Membership is the union of
/// explicit <PlaylistGame> title entries and, for auto-populate playlists,
/// games whose `series` field carries the playlist's "Playlist: X" tag
/// (LaunchBox stores auto-populate rules as filters on Series).
fn seed_curated_playlists(conn: &rusqlite::Connection, metadata_dir: &Path) {
    let path = metadata_dir.join("Playlists.xml.gz");
    let Ok(file) = std::fs::File::open(&path) else {
        println!("WARN: {} not found, skipping curated playlists", path.display());
        return;
    };
    let mut raw = String::new();
    use std::io::Read;
    if flate2::read::GzDecoder::new(file).read_to_string(&mut raw).is_err() {
        println!("WARN: failed to decompress {}", path.display());
        return;
    }

    println!("\nSeeding curated playlists:");
    for block in raw.split("<LaunchBox>").skip(1) {
        let Some(name) = extract_tag(block, "Name") else { continue };
        // Dynamic install-state list - redundant with the app's My Library.
        if name == "Installed eXoDOS Games" {
            continue;
        }

        let display_name = name.strip_prefix("eXoDOS ").unwrap_or(name);
        let description = extract_tag(block, "Notes").filter(|n| *n != ".");

        conn.execute(
            "INSERT INTO playlists (name, kind, slug, description)
             VALUES (?1, 'curated', ?2, ?3)",
            params![display_name, slugify(name), description],
        )
        .unwrap();
        let pid = conn.last_insert_rowid();

        // Auto-populate rules: every Series-keyed filter value.
        let mut member_count = 0usize;
        for filter_block in block.split("<PlaylistFilter>").skip(1) {
            if extract_tag(filter_block, "FieldKey") != Some("Series") {
                continue;
            }
            let Some(tag) = extract_tag(filter_block, "Value") else { continue };
            member_count += conn
                .execute(
                    "INSERT OR IGNORE INTO playlist_games (playlist_id, game_id)
                     SELECT ?1, id FROM games
                     WHERE '; ' || series || '; ' LIKE '%; ' || ?2 || '; %'",
                    params![pid, tag],
                )
                .unwrap();
        }

        // Explicit entries (manual playlists like Quality Freeware).
        let mut unmatched: Vec<&str> = Vec::new();
        for game_block in block.split("<PlaylistGame>").skip(1) {
            let Some(title) = extract_tag(game_block, "GameTitle") else { continue };
            let n = conn
                .execute(
                    "INSERT OR IGNORE INTO playlist_games (playlist_id, game_id)
                     SELECT ?1, id FROM games WHERE title = ?2",
                    params![pid, title],
                )
                .unwrap();
            if n == 0 {
                unmatched.push(title);
            } else {
                member_count += n;
            }
        }

        println!("  {}: {} games", display_name, member_count);
        if !unmatched.is_empty() {
            println!("    WARN unmatched titles: {:?}", unmatched);
        }
    }
}

fn main() {
    let root = project_root();
    let metadata_dir = root.join("metadata");
    let torrents_dir = root.join("torrents");
    let output_path = metadata_dir.join("exodium.db");

    // Remove old DB if it exists
    let _ = std::fs::remove_file(&output_path);

    println!("Generating pre-built database at {}", output_path.display());

    let conn = db::open(&output_path).expect("failed to create database");
    db::init(&conn).expect("failed to create schema");

    // Load all torrent indices upfront
    let mut torrent_indices: HashMap<String, TorrentIndex> = HashMap::new();
    for col in COLLECTION_MAP {
        let path = torrents_dir.join(col.torrent_file);
        if path.exists() {
            match TorrentIndex::from_file(&path) {
                Ok(idx) => {
                    println!("Loaded torrent {}: {} files", col.id, idx.files.len());
                    torrent_indices.insert(col.id.to_string(), idx);
                }
                Err(e) => eprintln!("Warning: failed to parse {}: {}", col.torrent_file, e),
            }
        } else {
            eprintln!("Warning: torrent not found: {}", path.display());
        }
    }

    // Import each collection's XML and match torrent indices
    let mut total_imported = 0usize;
    for col in COLLECTION_MAP {
        let meta_path = metadata_dir.join(col.metadata_file);
        if !meta_path.exists() {
            eprintln!("Warning: metadata not found: {}", meta_path.display());
            continue;
        }

        let file = std::fs::File::open(&meta_path).unwrap();
        let reader = BufReader::new(flate2::read::GzDecoder::new(file));
        let games = parse_games_xml(reader, col.shortcode_segment).unwrap();
        let count = games.len();

        db::queries::insert_games(&conn, &games).unwrap();
        println!("Imported {} games from {}", count, col.id);
        total_imported += count;

        // Match torrent indices for this collection
        if let Some(index) = torrent_indices.get(col.id) {
            // Only language packs draw on a base collection's GameData; a
            // collection with its own game tree (eXoWin3x) must not, or a
            // title it shares with eXoDOS inflates its download size by a
            // GameData archive that belongs to a different game.
            let shared = if col.lang_dir.is_some() {
                torrent_indices.get(exodium_lib::collection_base_id(col.id))
            } else {
                None
            };
            match_torrent_indices(&conn, index, col.id, shared);
        }
    }

    println!("\nTotal imported: {} games", total_imported);

    // The music hint only means something where a GameData archive exists to
    // hold the track. eXoWin3x carries `MissingMusic=false` on 1,120 rows and
    // ships no music at all - LaunchBox's default, not an inventory.
    conn.execute(
        "UPDATE games SET music_file = NULL WHERE gamedata_torrent_index IS NULL",
        [],
    )
    .unwrap();
    let music_hints: i64 = conn
        .query_row("SELECT COUNT(*) FROM games WHERE music_file IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    println!("Theme-track hints: {} games", music_hints);

    // One launcher, one row. eXoWin3x catalogues both Castle of the Winds games
    // against the same bat file (they ship as a single install), and two rows
    // sharing an application_path break refresh_catalog's matching key - it
    // would update an arbitrary one and insert duplicates on the next refresh.
    {
        let dupes: Vec<(i64, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT g.id, g.title, g.application_path FROM games g
                     WHERE g.application_path IS NOT NULL AND g.application_path != ''
                       AND EXISTS (
                         SELECT 1 FROM games o
                         WHERE o.application_path = g.application_path AND o.id < g.id
                       ) ORDER BY g.id",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for (id, title, path) in &dupes {
            println!("  dropping duplicate launcher row: \"{}\" ({})", title, path);
            conn.execute("DELETE FROM games WHERE id = ?1", params![id]).unwrap();
        }
        println!("Dropped {} rows sharing another row's launcher", dupes.len());
    }

    // eXo's own layout for the Spanish and Polish packs, read from the
    // bundled config archives: `!dos/<lang>/<code>/` holds the game's
    // dosbox.conf next to a launcher named after the game, and `<code>` is
    // also the directory the game ZIP extracts to. Those two XMLs carry no
    // shortcode and no RootFolder, so this file is the only link between a
    // catalogue row and the config it launches with - without it, 363
    // LP-exclusive games were downloadable but had nothing to run.
    for (pack, lang) in [("SLP", "!spanish"), ("PLP", "!polish")] {
        let map_path = metadata_dir.join(format!("{pack}_confdirs.txt"));
        let Ok(content) = std::fs::read_to_string(&map_path) else {
            eprintln!("Warning: {} not found - skipping", map_path.display());
            continue;
        };
        let by_bat: HashMap<&str, &str> = content
            .lines()
            .filter_map(|l| l.split_once(':'))
            .collect();
        let source = format!("eXoDOS_{pack}");
        let rows: Vec<(i64, Option<String>)> = {
            let mut stmt = conn
                .prepare("SELECT id, application_path FROM games WHERE torrent_source = ?1")
                .unwrap();
            stmt.query_map(params![&source], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        let tx = conn.unchecked_transaction().unwrap();
        let mut linked = 0usize;
        {
            let mut update = tx
                .prepare_cached("UPDATE games SET shortcode = ?1, dosbox_conf = ?2 WHERE id = ?3")
                .unwrap();
            for (id, app_path) in &rows {
                let Some(stem) = app_path.as_deref().and_then(game_name_from_app_path) else {
                    continue;
                };
                let Some(code) = by_bat.get(stem.as_str()) else {
                    continue;
                };
                // Windows separators, like every other row's stored path.
                let conf = format!("eXo\\eXoDOS\\!dos\\{lang}\\{code}/dosbox.conf");
                update.execute(params![code, conf, id]).unwrap();
                linked += 1;
            }
        }
        tx.commit().unwrap();
        println!(
            "{pack}: {linked}/{} rows linked to eXo's config directory",
            rows.len()
        );
    }

    // Every LP↔EN match below is scoped to the pack family: titles AND
    // shortcodes repeat across families (eXoWin9x catalogues "Gabriel Knight 2
    // - The Beast Within", eXoDOS/eXoWin3x both use "EarthQue"), and an
    // unscoped match hands an LP row another family's shortcode or key -
    // which orphans it from its variant group and 404s its cover.
    let fam_en = db::queries::family_expr("en");
    let fam_g = db::queries::family_expr("games");

    // Pass 1: Exact title match backfill (same SQL as runtime)
    conn.execute_batch(&format!(
        "UPDATE games SET shortcode = (
            SELECT en.shortcode FROM games en
            WHERE en.language = 'EN' AND en.shortcode IS NOT NULL AND en.title = games.title
              AND {fam_en} = {fam_g}
            LIMIT 1
        ) WHERE shortcode IS NULL",
    ))
    .unwrap();

    let null_after_pass1: usize = conn
        .query_row("SELECT COUNT(*) FROM games WHERE shortcode IS NULL", [], |r| r.get(0))
        .unwrap();
    println!("\nAfter exact title backfill: {} games still without shortcode", null_after_pass1);

    // Pass 2: Normalized title matching in Rust, keyed per (family, title)
    let family_of = |source: Option<&str>| -> String {
        exodium_lib::collection_base_id(source.unwrap_or("eXoDOS")).to_string()
    };
    let mut en_lookup: HashMap<(String, String), String> = HashMap::new();
    let mut en_ambiguous: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT title, shortcode, torrent_source FROM games WHERE language = 'EN' AND shortcode IS NOT NULL")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap();
        for (title, shortcode, source) in rows.flatten() {
            let key = (family_of(source.as_deref()), normalize_title(&title));
            match en_lookup.entry(key) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    en_ambiguous.insert(e.key().clone());
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(shortcode);
                }
            }
        }
    }
    // Remove ambiguous entries
    for key in &en_ambiguous {
        en_lookup.remove(key);
    }

    let orphans: Vec<(i64, String, Option<String>)>;
    {
        let mut stmt = conn
            .prepare("SELECT id, title, torrent_source FROM games WHERE shortcode IS NULL")
            .unwrap();
        orphans = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
    }

    let mut pass2_matched = 0usize;
    {
        let tx = conn.unchecked_transaction().unwrap();
        let mut update = tx
            .prepare_cached("UPDATE games SET shortcode = ?1 WHERE id = ?2")
            .unwrap();

        for (id, title, source) in &orphans {
            let key = (family_of(source.as_deref()), normalize_title(title));
            if let Some(shortcode) = en_lookup.get(&key) {
                update.execute(params![shortcode, id]).unwrap();
                pass2_matched += 1;
            }
        }
        drop(update);
        tx.commit().unwrap();
    }

    println!("After normalized matching: {} more matched to EN shortcodes", pass2_matched);

    // Pass 2b: canonical-title matching, the same fingerprint the cover
    // propagation uses (article/numeral/punctuation drift). `normalize_title`
    // above keeps LaunchBox's sort form ("Elder Scrolls, The - Arena") apart
    // from eXo's natural one ("The Elder Scrolls: Arena"), which left 23 LP
    // games without an EN link - and therefore without the EN dosbox.conf they
    // launch with. Ambiguous fingerprints are skipped: a wrong link merges two
    // games into one card and hides one of them.
    let mut canon_lookup: HashMap<(String, String), String> = HashMap::new();
    let mut canon_ambiguous: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT title, shortcode, torrent_source FROM games
                 WHERE language = 'EN' AND shortcode IS NOT NULL
                   AND dosbox_conf IS NOT NULL AND dosbox_conf != ''",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap();
        for (title, shortcode, source) in rows.flatten() {
            let key = (family_of(source.as_deref()), db::title_canonical(&title));
            match canon_lookup.entry(key) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    canon_ambiguous.insert(e.key().clone());
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(shortcode);
                }
            }
        }
    }
    for key in &canon_ambiguous {
        canon_lookup.remove(key);
    }

    // One language may occupy a group only once - a group is "one game per
    // language" and the UI shows a single chip per language. Spanish ships
    // three rows that all canonicalize onto King's Quest I (AGI original, SCI
    // remake, and a duplicate); linking them all would leave two of them
    // unreachable behind one chip. First row by id wins, the rest keep their
    // own generated shortcode and stay separate cards.
    let mut occupied: std::collections::HashSet<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT torrent_source, shortcode, language FROM games WHERE shortcode IS NOT NULL",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((
                family_of(row.get::<_, Option<String>>(0)?.as_deref()),
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    };

    let canon_orphans: Vec<(i64, String, Option<String>, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, torrent_source, language FROM games
                 WHERE shortcode IS NULL ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    };
    let mut pass2b_matched = 0usize;
    let mut pass2b_skipped = 0usize;
    {
        let tx = conn.unchecked_transaction().unwrap();
        let mut update = tx
            .prepare_cached("UPDATE games SET shortcode = ?1 WHERE id = ?2")
            .unwrap();
        for (id, title, source, language) in &canon_orphans {
            let family = family_of(source.as_deref());
            let Some(shortcode) = canon_lookup.get(&(family.clone(), db::title_canonical(title)))
            else {
                continue;
            };
            let slot = (family, shortcode.clone(), language.clone());
            if occupied.contains(&slot) {
                println!("  canonical link skipped (slot taken): \"{title}\" -> {shortcode}");
                pass2b_skipped += 1;
                continue;
            }
            update.execute(params![shortcode, id]).unwrap();
            occupied.insert(slot);
            pass2b_matched += 1;
        }
        drop(update);
        tx.commit().unwrap();
    }
    println!(
        "After canonical matching: {pass2b_matched} more matched to EN shortcodes \
         ({pass2b_skipped} skipped, language slot already taken)"
    );

    // Pass 3: Generate shortcodes for remaining LP-exclusive games
    // These have no EN counterpart, so they get a new unique shortcode derived from their title
    let existing_shortcodes: std::collections::HashSet<String> = {
        let mut stmt = conn
            .prepare("SELECT DISTINCT shortcode FROM games WHERE shortcode IS NOT NULL")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    let remaining: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, title FROM games WHERE shortcode IS NULL")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    let mut used_shortcodes = existing_shortcodes;
    let mut pass3_count = 0usize;
    {
        let tx = conn.unchecked_transaction().unwrap();
        let mut update = tx
            .prepare_cached("UPDATE games SET shortcode = ?1 WHERE id = ?2")
            .unwrap();

        for (id, title) in &remaining {
            let shortcode = generate_shortcode(title, &used_shortcodes);
            update.execute(params![&shortcode, id]).unwrap();
            used_shortcodes.insert(shortcode);
            pass3_count += 1;
        }
        drop(update);
        tx.commit().unwrap();
    }

    println!("Generated {} new shortcodes for LP-exclusive games", pass3_count);

    // Normalize LP shortcodes to match EN case (e.g., DE "abanplac" → EN "Abanplac")
    // This ensures thumbnails (which are named by EN shortcode) work for LP games
    let case_fixed = conn
        .execute(
            &format!(
                "UPDATE games SET shortcode = (
                    SELECT en.shortcode FROM games en
                    WHERE en.language = 'EN' AND en.shortcode IS NOT NULL
                      AND LOWER(en.shortcode) = LOWER(games.shortcode)
                      AND {fam_en} = {fam_g}
                    LIMIT 1
                ) WHERE language != 'EN' AND shortcode IS NOT NULL
                  AND EXISTS (
                    SELECT 1 FROM games en
                    WHERE en.language = 'EN' AND en.shortcode IS NOT NULL
                      AND LOWER(en.shortcode) = LOWER(games.shortcode)
                      AND {fam_en} = {fam_g}
                      AND en.shortcode != games.shortcode
                )",
            ),
            [],
        )
        .unwrap();
    println!("Fixed {} LP shortcodes to match EN case", case_fixed);

    // Fill missing dosbox_conf from EN counterparts (LP translations share the EN config)
    let dosbox_filled = conn
        .execute(
            &format!(
                "UPDATE games SET dosbox_conf = (
                    SELECT en.dosbox_conf FROM games en
                    WHERE {same} AND en.language = 'EN'
                      AND en.dosbox_conf IS NOT NULL AND en.dosbox_conf != ''
                    LIMIT 1
                ) WHERE (dosbox_conf IS NULL OR dosbox_conf = '')
                  AND shortcode IS NOT NULL",
                same = db::queries::same_group("en", "games"),
            ),
            [],
        )
        .unwrap();
    println!("Filled dosbox_conf for {} LP games from EN counterparts", dosbox_filled);

    // Populate thumbnail_key in three passes:
    //   1. Every EN game hashes its own title.
    //   2. Every LP variant that shares an EN shortcode copies its EN primary's
    //      hash, so language variants render the same cover art.
    //   3. Any game still without a key (LP-exclusive) hashes its own title.
    {
        let tx = conn.unchecked_transaction().unwrap();

        // Pass 1: EN games - hash own title.
        let en_titles: Vec<(i64, String)> = {
            let mut stmt = tx
                .prepare("SELECT id, title FROM games WHERE language = 'EN'")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        {
            let mut update = tx
                .prepare_cached("UPDATE games SET thumbnail_key = ?1 WHERE id = ?2")
                .unwrap();
            for (id, title) in &en_titles {
                update.execute(params![thumbnail_key(title), id]).unwrap();
            }
        }
        println!("  thumbnail_key pass 1 (EN own-title): {} games", en_titles.len());

        // Pass 2: LP variants - copy EN's hash where the group matches.
        let pass2 = tx
            .execute(
                &format!(
                    "UPDATE games
                     SET thumbnail_key = (
                         SELECT en.thumbnail_key FROM games en
                         WHERE en.language = 'EN'
                           AND {same}
                           AND en.thumbnail_key IS NOT NULL
                         LIMIT 1
                     )
                     WHERE thumbnail_key IS NULL
                       AND shortcode IS NOT NULL
                       AND language != 'EN'",
                    same = db::queries::same_group("en", "games"),
                ),
                [],
            )
            .unwrap_or(0);
        println!("  thumbnail_key pass 2 (LP shared with EN): {} games", pass2);

        // Pass 3: LP-exclusive - hash own title.
        let residual: Vec<(i64, String)> = {
            let mut stmt = tx
                .prepare("SELECT id, title FROM games WHERE thumbnail_key IS NULL")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        {
            let mut update = tx
                .prepare_cached("UPDATE games SET thumbnail_key = ?1 WHERE id = ?2")
                .unwrap();
            for (id, title) in &residual {
                update.execute(params![thumbnail_key(title), id]).unwrap();
            }
        }
        println!("  thumbnail_key pass 3 (LP-exclusive own-title): {} games", residual.len());

        tx.commit().unwrap();
    }

    // Pass 4: LP↔EN canonical title matching for cases where shortcode-based
    // sharing missed. Handles article-stripped + word/Roman-numeral folding
    // (e.g. PL "Legend of Kyrandia Book 2" → EN "The Legend of Kyrandia: Book
    // Two"). Overwrites LP thumbnail_key with EN's so variants share cover art.
    if let Err(e) = exodium_lib::db::propagate_lp_thumbnail_keys(&conn) {
        log::warn!("Pass 4 (LP canonical-title propagation) failed: {}", e);
    }

    // Mark games whose thumbnail file actually exists on disk (bundled pack).
    // has_thumbnail is now secondary to thumbnail_key but kept so the frontend
    // can skip image loads for known-absent files.
    // Each collection keeps its own thumbnail dir; language packs share the
    // base collection's, since their variants hash to the EN title's key.
    let thumb_root = root.join("thumbnails");
    if thumb_root.exists() {
        let tx = conn.unchecked_transaction().unwrap();
        let keyed_games: Vec<(i64, String, Option<String>)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, thumbnail_key, torrent_source FROM games
                     WHERE thumbnail_key IS NOT NULL",
                )
                .unwrap();
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        };
        let mut thumb_count = 0usize;
        let mut missing: Vec<(String, i64)> = Vec::new();
        {
            let mut update = tx
                .prepare_cached("UPDATE games SET has_thumbnail = 1 WHERE id = ?1")
                .unwrap();
            for (id, key, source) in &keyed_games {
                let thumb_dir = thumb_root.join(exodium_lib::collection_base_id(
                    source.as_deref().unwrap_or("eXoDOS"),
                ));
                if thumb_dir.join(format!("{}.jpg", key)).exists() {
                    update.execute(params![id]).unwrap();
                    thumb_count += 1;
                } else {
                    missing.push((key.clone(), *id));
                }
            }
        }
        tx.commit().unwrap();
        println!(
            "Marked {} of {} games with thumbnails on disk",
            thumb_count,
            keyed_games.len()
        );
        if !missing.is_empty() {
            // Log a sample of missing hashes so CI build output helps diagnose
            // coverage gaps (title mismatch between gen_thumbnails.py and XML).
            let sample: Vec<&(String, i64)> = missing.iter().take(10).collect();
            println!(
                "  Sample of thumbnail_keys without a file on disk ({} of {}): {:?}",
                sample.len(),
                missing.len(),
                sample
            );
        }
    }

    // Populate dosbox_variant from the per-family variant indices.
    // Format: "Game Title (Year):variant\dosbox.exe"
    // We strip the "(Year)" suffix and normalize before matching against game titles.
    //
    // Matching is scoped to the family the index belongs to: eXoDOS and eXoWin3x
    // share 100+ titles (Myst, SimCity, Civilization), and an unscoped title
    // match handed Win3x games the DOS build's variant.
    for (index_file, family) in [
        ("dosbox.txt", "eXoDOS"),
        ("dosbox3x.txt", "eXoWin3x"),
        ("dosbox9x.txt", "eXoWin9x"),
    ] {
        let dosbox_txt = root.join("metadata").join(index_file);
        if dosbox_txt.exists() {
            let content = std::fs::read_to_string(&dosbox_txt).unwrap_or_default();
            // Build map: normalized_title → variant_slug
            let mut variant_map: HashMap<String, String> = HashMap::new();
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() { continue; }
                let Some(colon) = line.rfind(':') else { continue };
                let title_raw = &line[..colon];
                let path_raw = &line[colon + 1..]; // e.g. "ece4230\dosbox.exe" or "dosbox.exe"
                // Extract slug: first path component before '\', or "dosbox" for bare "dosbox.exe"
                let slug = if let Some(sep) = path_raw.find('\\') {
                    path_raw[..sep].to_string()
                } else {
                    "dosbox".to_string() // bare dosbox.exe = classic 0.74
                };
                variant_map.insert(normalize_title(title_raw), slug);
            }
            println!("Loaded {} dosbox variant entries from {}", variant_map.len(), index_file);

            // Match within this family and update. The index keys are the
            // launcher BAT stems, and application_path carries that exact
            // stem - so match on it first and fall back to the title (the
            // title-only match left 88 Win9x games unmatched where the bat
            // name and the XML title disagree).
            let mut stmt = conn
                .prepare(
                    "SELECT id, title, application_path FROM games
                     WHERE COALESCE(torrent_source, 'eXoDOS') LIKE ?1 || '%'",
                )
                .unwrap();
            let rows: Vec<(i64, String, Option<String>)> = stmt
                .query_map(params![family], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            let bat_stem = |app_path: &str| -> Option<String> {
                let norm = app_path.replace('\\', "/");
                let file = norm.rsplit('/').next()?;
                Some(file.strip_suffix(".bat").unwrap_or(file).to_string())
            };

            let tx = conn.unchecked_transaction().unwrap();
            {
                let mut update = tx
                    .prepare_cached("UPDATE games SET dosbox_variant = ?1 WHERE id = ?2")
                    .unwrap();
                let mut matched = 0usize;
                for (id, title, app_path) in &rows {
                    let by_stem = app_path
                        .as_deref()
                        .and_then(&bat_stem)
                        .and_then(|s| variant_map.get(&normalize_title(&s)));
                    if let Some(variant) = by_stem.or_else(|| variant_map.get(&normalize_title(title))) {
                        update.execute(params![variant, id]).unwrap();
                        matched += 1;
                    }
                }
                println!("Set dosbox_variant for {}/{} {} games", matched, rows.len(), family);
            }
            tx.commit().unwrap();
        } else {
            println!("WARN: metadata/{} not found, skipping variant mapping", index_file);
        }
    }

    // Seed curated playlists from the bundled LaunchBox playlist metadata.
    seed_curated_playlists(&conn, &metadata_dir);

    // Final stats
    println!("\n--- Final Stats ---");
    let mut stmt = conn
        .prepare(
            "SELECT language, COUNT(*), SUM(CASE WHEN shortcode IS NULL THEN 1 ELSE 0 END)
             FROM games GROUP BY language ORDER BY language",
        )
        .unwrap();
    let stats = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, usize>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })
        .unwrap();
    for row in stats.flatten() {
        println!(
            "  {}: {} games, {} without shortcode ({:.0}% coverage)",
            row.0,
            row.1,
            row.2,
            (1.0 - row.2 as f64 / row.1 as f64) * 100.0
        );
    }

    // Save default collections config
    let all_collections: Vec<&str> = COLLECTION_MAP.iter().map(|c| c.id).collect();
    db::queries::set_config(&conn, "collections", &all_collections.join(","))
        .unwrap();
    // Stamp the catalog version so fresh installs skip the startup refresh.
    db::queries::set_config(&conn, "catalog_version", &db::CATALOG_VERSION.to_string())
        .unwrap();

    // refresh_catalog matches rows on application_path (title+language for
    // empty paths) - duplicates would make the in-place update pick an
    // arbitrary winner and the insert duplicate rows. Fail the build instead.
    let dup_paths: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (SELECT application_path FROM games \
             WHERE application_path IS NOT NULL AND application_path != '' \
             GROUP BY application_path HAVING COUNT(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let dup_fallback: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (SELECT title, language FROM games \
             WHERE application_path IS NULL OR application_path = '' \
             GROUP BY title, language HAVING COUNT(*) > 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        dup_paths == 0 && dup_fallback == 0,
        "catalog refresh keys are not unique: {} duplicate application_paths, \
         {} duplicate title+language among empty-path rows",
        dup_paths,
        dup_fallback
    );

    println!("\nDatabase written to {}", output_path.display());
    let size = std::fs::metadata(&output_path).unwrap().len();
    println!("Size: {:.1} MB", size as f64 / 1024.0 / 1024.0);
}
