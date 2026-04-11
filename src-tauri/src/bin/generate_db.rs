//! Build tool: generates the pre-built exodium.db from bundled XML metadata + torrents.
//!
//! Run from src-tauri/:  cargo run --bin generate_db

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rusqlite::params;

use exodium_lib::db;
use exodium_lib::game_name_from_app_path;
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
    t = t.replace(':', " ").replace('-', " ").replace(',', " ").replace('&', " and ");
    t = t.replace('\'', "").replace('!', " ").replace('.', " ");

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
        .prepare("SELECT id, title, application_path FROM games WHERE game_torrent_index IS NULL")
        .unwrap();
    let games: Vec<(i64, String, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
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

        for (id, title, app_path) in &games {
            let search_name = app_path
                .as_deref()
                .and_then(game_name_from_app_path)
                .unwrap_or_else(|| title.clone());

            let (game_entry, gamedata_entry) = index.find_game_files(&search_name);

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
            let shared = if col.id != "eXoDOS" {
                torrent_indices.get("eXoDOS")
            } else {
                None
            };
            match_torrent_indices(&conn, index, col.id, shared);
        }
    }

    println!("\nTotal imported: {} games", total_imported);

    // Pass 1: Exact title match backfill (same SQL as runtime)
    conn.execute_batch(
        "UPDATE games SET shortcode = (
            SELECT en.shortcode FROM games en
            WHERE en.language = 'EN' AND en.shortcode IS NOT NULL AND en.title = games.title
            LIMIT 1
        ) WHERE shortcode IS NULL",
    )
    .unwrap();

    let null_after_pass1: usize = conn
        .query_row("SELECT COUNT(*) FROM games WHERE shortcode IS NULL", [], |r| r.get(0))
        .unwrap();
    println!("\nAfter exact title backfill: {} games still without shortcode", null_after_pass1);

    // Pass 2: Normalized title matching in Rust
    let mut en_lookup: HashMap<String, String> = HashMap::new();
    let mut en_ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT title, shortcode FROM games WHERE language = 'EN' AND shortcode IS NOT NULL")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();
        for row in rows.flatten() {
            let normalized = normalize_title(&row.0);
            if en_lookup.contains_key(&normalized) {
                en_ambiguous.insert(normalized);
            } else {
                en_lookup.insert(normalized, row.1);
            }
        }
    }
    // Remove ambiguous entries
    for key in &en_ambiguous {
        en_lookup.remove(key);
    }

    let orphans: Vec<(i64, String)>;
    {
        let mut stmt = conn
            .prepare("SELECT id, title FROM games WHERE shortcode IS NULL")
            .unwrap();
        orphans = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
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

        for (id, title) in &orphans {
            let normalized = normalize_title(title);
            if let Some(shortcode) = en_lookup.get(&normalized) {
                update.execute(params![shortcode, id]).unwrap();
                pass2_matched += 1;
            }
        }
        drop(update);
        tx.commit().unwrap();
    }

    println!("After normalized matching: {} more matched to EN shortcodes", pass2_matched);

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
            "UPDATE games SET shortcode = (
                SELECT en.shortcode FROM games en
                WHERE en.language = 'EN' AND en.shortcode IS NOT NULL
                  AND LOWER(en.shortcode) = LOWER(games.shortcode)
                LIMIT 1
            ) WHERE language != 'EN' AND shortcode IS NOT NULL
              AND EXISTS (
                SELECT 1 FROM games en
                WHERE en.language = 'EN' AND en.shortcode IS NOT NULL
                  AND LOWER(en.shortcode) = LOWER(games.shortcode)
                  AND en.shortcode != games.shortcode
            )",
            [],
        )
        .unwrap();
    println!("Fixed {} LP shortcodes to match EN case", case_fixed);

    // Fill missing dosbox_conf from EN counterparts (LP translations share the EN config)
    let dosbox_filled = conn
        .execute(
            "UPDATE games SET dosbox_conf = (
                SELECT en.dosbox_conf FROM games en
                WHERE en.shortcode = games.shortcode AND en.language = 'EN'
                  AND en.dosbox_conf IS NOT NULL AND en.dosbox_conf != ''
                LIMIT 1
            ) WHERE (dosbox_conf IS NULL OR dosbox_conf = '')
              AND shortcode IS NOT NULL",
            [],
        )
        .unwrap();
    println!("Filled dosbox_conf for {} LP games from EN counterparts", dosbox_filled);

    // Mark games that have thumbnails on disk
    let thumb_dir = root.join("thumbnails/eXoDOS");
    if thumb_dir.exists() {
        let mut thumb_count = 0usize;
        let mut stmt = conn
            .prepare("SELECT DISTINCT shortcode FROM games WHERE shortcode IS NOT NULL")
            .unwrap();
        let shortcodes: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let tx = conn.unchecked_transaction().unwrap();
        {
            let mut update = tx
                .prepare_cached("UPDATE games SET has_thumbnail = 1 WHERE shortcode = ?1")
                .unwrap();
            for sc in &shortcodes {
                if thumb_dir.join(format!("{}.jpg", sc)).exists() {
                    update.execute(params![sc]).unwrap();
                    thumb_count += 1;
                }
            }
        }
        tx.commit().unwrap();
        println!("Marked {} shortcodes with thumbnails ({} without)", thumb_count, shortcodes.len() - thumb_count);
    }

    // Populate dosbox_variant from metadata/dosbox.txt
    // Format: "Game Title (Year):variant\dosbox.exe"
    // We strip the "(Year)" suffix and normalize before matching against game titles.
    let dosbox_txt = root.join("metadata/dosbox.txt");
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
        println!("Loaded {} dosbox variant entries", variant_map.len());

        // Match by title and update
        let mut stmt = conn.prepare("SELECT id, title FROM games").unwrap();
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let tx = conn.unchecked_transaction().unwrap();
        {
            let mut update = tx
                .prepare_cached("UPDATE games SET dosbox_variant = ?1 WHERE id = ?2")
                .unwrap();
            let mut matched = 0usize;
            for (id, title) in &rows {
                if let Some(variant) = variant_map.get(&normalize_title(title)) {
                    update.execute(params![variant, id]).unwrap();
                    matched += 1;
                }
            }
            println!("Set dosbox_variant for {}/{} games", matched, rows.len());
        }
        tx.commit().unwrap();
    } else {
        println!("WARN: metadata/dosbox.txt not found, skipping variant mapping");
    }

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
    db::queries::set_config(&conn, "collections", "eXoDOS,eXoDOS_GLP,eXoDOS_SLP,eXoDOS_PLP")
        .unwrap();

    println!("\nDatabase written to {}", output_path.display());
    let size = std::fs::metadata(&output_path).unwrap().len();
    println!("Size: {:.1} MB", size as f64 / 1024.0 / 1024.0);
}
