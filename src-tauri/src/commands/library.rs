//! What the user owns: matching catalogue rows to torrent files, deciding
//! which archives on disk are installs (§4), uninstall with save backup and
//! reset (§5).

use std::path::PathBuf;

use tauri::State;

use crate::db;
use crate::db::queries;
use crate::torrent::TorrentIndex;

use super::collections::{collection_rel_game_dir, collection_rel_zip, COLLECTION_MAP};
use super::games::{configured_data_dir, copy_dir_recursive, extract_game_zip, game_op_lock, running_game_key, running_games};
use super::paths::{bundled_torrent_path, game_root};
use super::{DbState, TorrentState};

/// Extract the game name (with year) from the application_path.
/// e.g. "eXo\eXoDOS\!dos\captlsm\Capitalism (1995).bat" -> "Capitalism (1995)"
pub fn game_name_from_app_path(app_path: &str) -> Option<String> {
    let normalized = app_path.replace('\\', "/");
    let filename = normalized.rsplit('/').next()?;
    let name = filename.strip_suffix(".bat")?;
    Some(name.to_string())
}

/// Names to look for in a torrent, best first.
///
/// eXo names every zip `<Title> (<Year>).zip`, and the launcher bat repeats
/// that name - so the path is the reliable source. Three GLP games are
/// catalogued with no ApplicationPath at all (issue #26); for those the plain
/// title misses the year suffix, leaving them unmatched and therefore
/// undownloadable, so reconstruct the eXo name from title + year as a
/// fallback. Both matchers (runtime and generate_db) use this.
pub fn torrent_search_names(
    title: &str,
    app_path: Option<&str>,
    year: Option<i64>,
) -> Vec<String> {
    if let Some(from_path) = app_path.and_then(game_name_from_app_path) {
        return vec![from_path];
    }
    let mut names = Vec::with_capacity(2);
    if let Some(y) = year {
        names.push(format!("{title} ({y})"));
    }
    names.push(title.to_string());
    names
}

/// Match imported games to their torrent file indices.
/// `torrent_source` identifies which torrent file this is.
pub(crate) fn match_torrent_indices(
    conn: &rusqlite::Connection,
    index: &TorrentIndex,
    torrent_source: &str,
) -> Result<(), String> {
    let mut matched = 0;
    let mut unmatched = 0;

    // Only match games that don't already have a torrent index
    let mut stmt = conn
        .prepare(
            "SELECT id, title, application_path, year FROM games WHERE game_torrent_index IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let games: Vec<(i64, String, Option<String>, Option<i64>)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    {
        let mut update_stmt = tx
            .prepare_cached(
                "UPDATE games SET game_torrent_index = ?1, gamedata_torrent_index = ?2,
                 download_size = ?3, torrent_source = ?4 WHERE id = ?5",
            )
            .map_err(|e| e.to_string())?;

        for (id, title, app_path, year) in &games {
            let (game_entry, gamedata_entry) = torrent_search_names(title, app_path.as_deref(), *year)
                .iter()
                .map(|name| index.find_game_files(name))
                .find(|(game, _)| game.is_some())
                .unwrap_or((None, None));

            if let Some(game) = game_entry {
                let gamedata_idx = gamedata_entry.map(|g| g.index as i64);
                let size = game.size as i64
                    + gamedata_entry.map(|g| g.size as i64).unwrap_or(0);

                update_stmt
                    .execute(rusqlite::params![
                        game.index as i64,
                        gamedata_idx,
                        size,
                        torrent_source,
                        id,
                    ])
                    .map_err(|e| e.to_string())?;
                matched += 1;
            } else {
                unmatched += 1;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;

    log::info!(
        "Torrent index matching: {} matched, {} unmatched out of {} games",
        matched,
        unmatched,
        games.len()
    );
    Ok(())
}

/// Torrent files that can ONLY have arrived as a side effect of downloading
/// `requested`: every piece they occupy is also occupied by a requested file.
///
/// A piece is the smallest unit a torrent transfers (8 MiB for eXoDOS), and
/// most eXoDOS archives are far smaller than that, so fetching one game
/// physically delivers whichever neighbours share its pieces - complete and
/// intact, not as fragments. That is why no integrity check can tell the two
/// apart, and why this is decided from the torrent's geometry instead:
/// a file none of whose pieces were worth fetching on their own was never
/// asked for. Files sit in the piece space back to back, so this only ever
/// catches immediate neighbours.
fn collateral_file_indices(
    index: &TorrentIndex,
    requested: &std::collections::HashSet<usize>,
) -> std::collections::HashSet<usize> {
    let piece_len = index.piece_length;
    if piece_len == 0 {
        return Default::default();
    }
    let pieces_of = |f: &crate::torrent::TorrentFileEntry| -> (u64, u64) {
        let last = f.offset + f.size.saturating_sub(1);
        (f.offset / piece_len, last / piece_len)
    };

    let mut covered: std::collections::HashSet<u64> = Default::default();
    for idx in requested {
        if let Some(f) = index.files.get(*idx) {
            let (first, last) = pieces_of(f);
            covered.extend(first..=last);
        }
    }

    index
        .files
        .iter()
        .filter(|f| !requested.contains(&f.index) && f.size > 0)
        .filter(|f| {
            let (first, last) = pieces_of(f);
            (first..=last).all(|p| covered.contains(&p))
        })
        .map(|f| f.index)
        .collect()
}

/// Drop library entries for games that were never asked for: their archive is
/// on disk only because it shared pieces with something that was.
///
/// Runs between the two scan passes, because it needs pass 1's verdict (an
/// extracted directory is proof the user installed it) and has to be done
/// before pass 2 would confirm the very rows it removes. Support archives
/// Exodium fetches on the user's behalf count as requested - util.zip alone
/// carries the last four eXoDOS games along with it.
fn clear_collateral_library_entries(
    db: &std::sync::Mutex<rusqlite::Connection>,
    data_dir: &str,
    indices: &std::collections::HashMap<&'static str, TorrentIndex>,
) -> usize {
    let torrent_root = game_root(data_dir);
    let mut cleared = 0usize;

    for (col_id, index) in indices {
        let mut requested: std::collections::HashSet<usize> = Default::default();

        // Everything pass 1 found extracted on disk, plus the shared GameData
        // archives those installs pulled in.
        {
            let Ok(conn) = db.lock() else { continue };
            let Ok(mut stmt) = conn.prepare(
                "SELECT game_torrent_index, gamedata_torrent_index FROM games \
                 WHERE installed = 1 AND torrent_source = ?1",
            ) else {
                continue;
            };
            let rows = stmt.query_map(rusqlite::params![col_id], |r| {
                Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
            });
            if let Ok(rows) = rows {
                for (game, gamedata) in rows.flatten() {
                    requested.extend(game.map(|i| i as usize));
                    requested.extend(gamedata.map(|i| i as usize));
                }
            }
        }

        // Archives Exodium downloads by itself, recognised by the traces their
        // extraction leaves. Without these, their neighbours look unexplained.
        let support: [(&str, &str); 3] = [
            ("util/util.zip", "eXo/mt32"),
            ("!DOSmetadata.zip", "eXo/eXoDOS/!dos"),
            ("util/utilWin9x.zip", "eXo/emulators/86Box98"),
        ];
        for (suffix, trace) in support {
            let Some(f) = index.find_by_suffix(suffix) else { continue };
            // Extracted already, or still arriving: bytes on disk are enough.
            // Waiting for the trace would leave the neighbours of a util.zip
            // that is still downloading unexplained - which is exactly when
            // they show up. These archives are far too large to be collateral
            // themselves.
            let started = std::fs::metadata(torrent_root.join(&f.path))
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            if started || torrent_root.join(trace).exists() {
                requested.insert(f.index);
            }
        }

        if requested.is_empty() {
            continue;
        }

        let collateral = collateral_file_indices(index, &requested);
        if collateral.is_empty() {
            continue;
        }

        // Only rows whose archive is REALLY on disk and complete. A missing or
        // half-written file means the entry describes a download the user
        // started and Exodium never finished - that one stays.
        let candidates: Vec<(i64, usize)> = {
            let Ok(conn) = db.lock() else { continue };
            let Ok(mut stmt) = conn.prepare(
                "SELECT id, game_torrent_index FROM games \
                 WHERE in_library = 1 AND installed = 0 AND torrent_source = ?1 \
                   AND game_torrent_index IS NOT NULL",
            ) else {
                continue;
            };
            let rows = stmt.query_map(rusqlite::params![col_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as usize))
            });
            match rows {
                Ok(rows) => rows.flatten().filter(|(_, ti)| collateral.contains(ti)).collect(),
                Err(_) => continue,
            }
        };

        let stale: Vec<i64> = candidates
            .into_iter()
            .filter(|(_, ti)| {
                index.files.get(*ti).is_some_and(|f| {
                    std::fs::metadata(torrent_root.join(&f.path))
                        .map(|m| m.len() == f.size)
                        .unwrap_or(false)
                })
            })
            .map(|(id, _)| id)
            .collect();

        if stale.is_empty() {
            continue;
        }

        let placeholders = stale.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!("UPDATE games SET in_library = 0 WHERE id IN ({})", placeholders);
        if let Ok(conn) = db.lock() {
            match conn.execute(&sql, rusqlite::params_from_iter(stale.iter())) {
                Ok(n) => {
                    log::info!(
                        "scan_installed_games: dropped {} {} library entries whose archive \
                         only arrived as a side effect of another download",
                        n,
                        col_id
                    );
                    cleared += n;
                }
                Err(e) => log::warn!("Failed to clear collateral entries: {}", e),
            }
        }
    }

    cleared
}

/// The bundled torrent index of every collection that keeps its own archives
/// (the language packs overlay eXoDOS's paths and are matched by directory).
fn scan_torrent_indices() -> std::collections::HashMap<&'static str, TorrentIndex> {
    let mut out = std::collections::HashMap::new();
    for col in COLLECTION_MAP.iter().filter(|c| c.lang_dir.is_none()) {
        let Ok(path) = bundled_torrent_path(col.torrent_file) else { continue };
        match TorrentIndex::from_file(&path) {
            Ok(index) => {
                out.insert(col.id, index);
            }
            Err(e) => log::warn!("scan: cannot parse {}: {}", col.torrent_file, e),
        }
    }
    out
}

/// Scan the eXoDOS directory tree and mark games whose files exist on disk as
/// installed.  Returns the number of rows updated.
///
/// `adopt_from_disk` decides whether a bare archive may CREATE a library
/// entry. It must be false for the automatic scan at startup: a download
/// delivers whichever neighbours share its pieces, so "an archive is here"
/// is not the same as "the user wanted this game", and treating it as such
/// filled libraries with games nobody asked for. It is true where the disk
/// IS the answer the user asked for - importing an existing eXo installation,
/// pointing Exodium at another folder, or pressing Rescan after copying games
/// in by hand.
pub(crate) fn scan_installed_games_with_db(
    db: &std::sync::Mutex<rusqlite::Connection>,
    data_dir: &str,
    adopt_from_disk: bool,
) -> Result<usize, String> {
    // Each collection's extracted game data lives under its own tree
    // (<data_dir>/<inner_folder>/<game_prefix>):
    //   eXo/eXoDOS/<shortcode>/           - English (eXoDOS)
    //   eXo/eXoDOS/!german/<shortcode>/   - German LP (GLP; !polish/!spanish alike)
    //   eXo/eXoWin3x/<shortcode>/         - eXoWin3x
    //   eXo/eXoWin9x/<year>/<title dir>/  - eXoWin9x (year_subdirs)
    //
    // Note: the shortcode_segment dirs (eXo/eXoDOS/!dos/ etc.) contain only
    // config/script files and are ALWAYS present - the '!' filter below keeps
    // them from counting as installs.
    let game_base = game_root(data_dir).join("eXo").join("eXoDOS");

    // Refuse to scan a data dir that holds no collection tree at all. The
    // scan starts by clearing every installed flag, so running it against a
    // missing folder - an unmounted external drive, a path typo, a move that
    // has not finished - would report the whole library as gone and invite a
    // re-download of hundreds of gigabytes.
    if !COLLECTION_MAP.iter().any(|c| {
        game_root(data_dir)
            .join(c.game_prefix)
            .is_dir()
    }) {
        return Err(format!(
            "No game folders found under {data_dir}. Check the data directory in Settings."
        ));
    }

    // Reset installed flags before the scan so that games whose extracted
    // directory was removed are correctly flipped back to "not installed".
    // in_library is left alone: it is sticky by design (set on download
    // start, cleared on uninstall/cancel) - wiping it here removed the
    // "My Games" progress card of any download that was still in flight
    // when the user triggered a rescan.
    {
        let conn = db.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("UPDATE games SET installed = 0")
            .map_err(|e| e.to_string())?;
    }

    let mut total = 0usize;

    for col in COLLECTION_MAP {
        let col_base = game_root(data_dir).join(col.game_prefix);
        let list_game_dirs = |dir: &PathBuf| -> Option<Vec<String>> {
            match std::fs::read_dir(dir) {
                Ok(entries) => Some(
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|name| !name.starts_with('!') && !name.starts_with('.'))
                        .collect(),
                ),
                Err(e) => {
                    log::warn!("scan_installed_games: cannot read {}: {}", dir.display(), e);
                    None
                }
            }
        };
        let shortcodes: Vec<String> = if let Some(lang_dir) = col.lang_dir {
            // LP collection: extracted game data is at <col_base>/<lang_dir>/<shortcode>/
            let seg_dir = col_base.join(lang_dir);
            if !seg_dir.is_dir() {
                continue;
            }
            match list_game_dirs(&seg_dir) {
                Some(dirs) => dirs,
                None => continue,
            }
        } else if col.year_subdirs {
            // eXoWin9x layout: title dirs nested under 4-digit year dirs.
            // The title dir IS the shortcode; `!save/` backups are filtered
            // out by the '!' rule inside list_game_dirs.
            if !col_base.is_dir() {
                continue;
            }
            let year_dirs: Vec<PathBuf> = match std::fs::read_dir(&col_base) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter(|e| {
                        let n = e.file_name();
                        let n = n.to_string_lossy();
                        n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit())
                    })
                    .map(|e| e.path())
                    .collect(),
                Err(e) => {
                    log::warn!("scan_installed_games: cannot read {}: {}", col_base.display(), e);
                    continue;
                }
            };
            year_dirs
                .iter()
                .filter_map(list_game_dirs)
                .flatten()
                .collect()
        } else {
            // Base EN collection: extracted game data is directly at <col_base>/<shortcode>/
            // (the '!'/'.' filter drops the always-present config and lang dirs).
            if !col_base.is_dir() {
                continue;
            }
            match list_game_dirs(&col_base) {
                Some(dirs) => dirs,
                None => continue,
            }
        };

        if shortcodes.is_empty() {
            continue;
        }

        // Build "UPDATE … WHERE shortcode IN (?, ?, …) AND torrent_source = ?"
        let placeholders = shortcodes.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE games SET installed = 1, in_library = 1 WHERE shortcode IN ({}) AND torrent_source = ?",
            placeholders
        );

        // Append torrent_source as the last bind value so we can use params_from_iter.
        let mut all_params: Vec<String> = shortcodes.clone();
        all_params.push(col.id.to_string());

        let conn = db.lock().map_err(|e| e.to_string())?;
        let rows = conn
            .execute(&sql, rusqlite::params_from_iter(all_params.iter()))
            .map_err(|e| e.to_string())?;

        log::info!(
            "scan_installed_games: {} of {} dirs matched in DB for {}",
            rows,
            shortcodes.len(),
            col.id
        );
        total += rows;
    }

    // Collateral archives are dropped from the library BEFORE pass 2, which
    // would otherwise confirm the very rows this removes.
    let indices = scan_torrent_indices();
    // An imported eXo installation IS its library: the games arrived as
    // archives, not as downloads, so "no piece of this was worth fetching on
    // its own" describes every one of them. Running the cleanup there took
    // games the user owns out of My Games on the first start after the import,
    // and no later automatic scan could put them back.
    let disk_is_library = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "library_from_disk")
            .ok()
            .flatten()
            .as_deref()
            == Some("1")
    };
    if !adopt_from_disk && !disk_is_library {
        clear_collateral_library_entries(db, data_dir, &indices);
    }

    // Sizes to measure the files on disk against. A torrent entry is 0 bytes
    // until it is fetched, and a neighbouring download leaves piece-sized
    // partials, so "the file exists" says nothing about it being downloaded.
    let expected_sizes: std::collections::HashMap<String, u64> = indices
        .values()
        .flat_map(|i| i.files.iter())
        .map(|f| (f.path.clone(), f.size))
        .collect();
    let torrent_root = game_root(data_dir);
    let lenient_sizes = adopt_from_disk || expected_sizes.is_empty();
    if expected_sizes.is_empty() {
        log::warn!(
            "scan_installed_games: no bundled torrent could be parsed - falling back to the \
             old size floor, so piece-sized partials may read as installs"
        );
    }

    // Pass 2: detect downloaded-but-not-extracted game ZIPs → mark as installed + in_library.
    // All eXoDOS game ZIPs live at game_base/<title with year>.zip regardless of collection.
    // This mirrors LaunchBox behavior where games stay as ZIPs until first launch.
    //
    // IMPORTANT: Only match ZIPs to non-LP (base) collections.  LP collections (GLP, PLP, SLP)
    // may share English titles with eXoDOS games; including them in the HashMap would cause
    // title collisions where an EN ZIP incorrectly marks an LP game as installed.
    // LP games are only considered installed when their extracted directory exists (Pass 1).
    if game_base.is_dir() {
        // Build lookup: zip_stem → game_id, restricted to non-LP collections.
        let lp_sources: Vec<String> = COLLECTION_MAP
            .iter()
            .filter(|c| c.lang_dir.is_some())
            .map(|c| c.id.to_string())
            .collect();
        let lp_placeholders = lp_sources.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        // Candidates are every not-yet-matched non-LP game. Filtering on
        // in_library too made the scan NON-IDEMPOTENT: the first run sets it,
        // so the second run no longer considered those rows and reported a
        // smaller number - and any game that exists only as a ZIP quietly lost
        // its installed flag (observed: 112, then 67, then 63).
        let intent_filter = if adopt_from_disk { "" } else { "AND in_library = 1 " };
        let zip_query = format!(
            "SELECT id, title, application_path FROM games \
             WHERE installed = 0 {intent_filter}\
             AND torrent_source NOT IN ({})",
            lp_placeholders
        );

        let name_to_id: std::collections::HashMap<String, i64> = {
            let conn = db.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare(&zip_query)
                .map_err(|e| e.to_string())?;
            // Collect eagerly so stmt and conn can be dropped before the HashMap build.
            let rows: Vec<(i64, String, Option<String>)> = stmt
                .query_map(rusqlite::params_from_iter(lp_sources.iter()), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            rows.into_iter()
                .map(|(id, title, app_path)| {
                    let name = app_path
                        .as_deref()
                        .and_then(game_name_from_app_path)
                        .unwrap_or(title);
                    (name, id)
                })
                .collect()
        };

        let collect_zip_ids = |dir: &PathBuf| -> Vec<i64> {
            match std::fs::read_dir(dir) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        let path = e.path();
                        if !path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                        {
                            return false;
                        }
                        // Complete, measured against the torrent: a 1 KB floor
                        // let piece-sized partials of a neighbouring download
                        // pass as installs (a 376 MB game arriving as 8 MB).
                        let len = match e.metadata() {
                            Ok(m) => m.len(),
                            Err(_) => return false,
                        };
                        // Lenient where measuring is impossible or wrong:
                        // without a parsed torrent there is nothing to compare
                        // against (and flipping every ZIP-only game to "not
                        // installed" would invite a re-download), and an
                        // imported tree may legitimately hold repacked
                        // archives the bundled torrent never described.
                        if lenient_sizes {
                            return len >= 1024;
                        }
                        let Ok(rel) = path.strip_prefix(&torrent_root) else {
                            return false;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        expected_sizes.get(&rel).is_some_and(|expected| len == *expected)
                    })
                    .filter_map(|e| {
                        let stem = e.path().file_stem()?.to_string_lossy().into_owned();
                        name_to_id.get(&stem).copied()
                    })
                    .collect(),
                Err(e) => {
                    log::warn!(
                        "scan_installed_games: cannot scan {} for ZIPs: {}",
                        dir.display(),
                        e
                    );
                    vec![]
                }
            }
        };
        let mut zip_ids: Vec<i64> = collect_zip_ids(&game_base);
        // year_subdirs collections keep their ZIPs under year folders
        // (eXo/eXoWin9x/<year>/<Title (Year)>.zip) - scan those too.
        for col in COLLECTION_MAP.iter().filter(|c| c.year_subdirs) {
            let col_base = game_root(data_dir).join(col.game_prefix);
            let Ok(entries) = std::fs::read_dir(&col_base) else { continue };
            for year_dir in entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit())
                })
            {
                zip_ids.extend(collect_zip_ids(&year_dir.path()));
            }
        }

        if !zip_ids.is_empty() {
            let placeholders = zip_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!(
                "UPDATE games SET installed = 1, in_library = 1 WHERE id IN ({})",
                placeholders
            );
            let conn = db.lock().map_err(|e| e.to_string())?;
            let rows = conn
                .execute(&sql, rusqlite::params_from_iter(zip_ids.iter()))
                .map_err(|e| e.to_string())?;
            log::info!(
                "scan_installed_games: {} games marked installed from ZIP scan ({} ZIPs found)",
                rows,
                zip_ids.len()
            );
            total += rows;
        }
    }

    // Library entries whose files are not here. Sticky in_library is right
    // for a game you uninstalled, but after a folder move it also describes
    // games whose data was left behind - and a screen of unexplained
    // "Incomplete" badges reads as a bug in the app rather than a
    // half-finished move.
    let orphaned: i64 = {
        let conn = db.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT COUNT(*) FROM games WHERE in_library = 1 AND installed = 0",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
    };
    if orphaned > 0 {
        log::info!(
            "scan_installed_games: {} library entries have no files under {} - left behind \
             by a move, or never finished downloading",
            orphaned,
            data_dir
        );
    }

    Ok(total)
}

/// Re-scan the eXoDOS directory tree to detect games already downloaded to disk.
/// Returns the count of games updated.
///
/// `adopt` (default false) hands the disk the authority to add games to the
/// library - see `scan_installed_games_with_db`. The startup scan omits it;
/// the Rescan button and a data-directory change pass true, because there the
/// user is asking what is in the folder.
#[tauri::command]
pub async fn scan_installed_games(
    db_state: State<'_, DbState>,
    adopt: Option<bool>,
) -> Result<usize, String> {
    let data_dir = {
        let conn = db_state.lock()?;
        crate::commands::games::configured_data_dir(&conn)?
    };
    scan_installed_games_with_db(&db_state.0, &data_dir, adopt.unwrap_or(false))
}

#[cfg(test)]
mod search_name_tests {
    use super::*;

    #[test]
    fn app_path_wins_and_is_the_only_candidate() {
        let names = torrent_search_names(
            "Capitalism",
            Some(r"eXo\eXoDOS\!dos\captlsm\Capitalism (1995).bat"),
            Some(1995),
        );
        assert_eq!(names, vec!["Capitalism (1995)".to_string()]);
    }

    #[test]
    fn pathless_row_reconstructs_the_exo_zip_name_from_title_and_year() {
        // The three GLP games without an ApplicationPath (issue #26): their
        // zip is "<Title> (<Year>).zip", so the bare title never matches.
        let names = torrent_search_names("Kathedrale, Die", None, Some(1991));
        assert_eq!(names[0], "Kathedrale, Die (1991)");
        assert_eq!(names[1], "Kathedrale, Die");
    }

    #[test]
    fn pathless_row_without_a_year_still_tries_the_title() {
        assert_eq!(
            torrent_search_names("Some Game", None, None),
            vec!["Some Game".to_string()]
        );
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;


    /// Rescanning must answer the same thing every time. It did not: the ZIP
    /// pass skipped rows whose in_library flag the previous run had set, so
    /// repeated scans reported ever fewer games (112, 67, 63) and ZIP-only
    /// installs silently lost their installed flag.
    #[test]
    fn rescanning_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("exodium_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // One extracted game and one that only exists as a ZIP.
        let base = dir.join("eXoDOS/eXo/eXoDOS");
        std::fs::create_dir_all(base.join("SQ5")).unwrap();
        // Sized like the torrent says, sparse - a ZIP now counts as an install
        // only at its full length.
        write_sparse_zip(&base.join("Capitalism (1995).zip"), "eXo/eXoDOS/Capitalism (1995).zip");

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn.execute_batch(
            r"INSERT INTO games (title, platform, language, shortcode, torrent_source,
                                 application_path, installed, in_library)
              VALUES ('Space Quest V', 'MS-DOS', 'EN', 'SQ5', 'eXoDOS',
                      'eXo\eXoDOS\!dos\SQ5\Space Quest V.bat', 0, 0),
                     ('Capitalism', 'MS-DOS', 'EN', 'captlsm', 'eXoDOS',
                      'eXo\eXoDOS\!dos\captlsm\Capitalism (1995).bat', 0, 0)",
        )
        .unwrap();
        let db = std::sync::Mutex::new(conn);
        let data_dir = dir.to_string_lossy().to_string();

        // A data dir without any collection tree must not clear the library.
        assert!(scan_installed_games_with_db(&db, "/nonexistent/exodium", true).is_err());

        let first = scan_installed_games_with_db(&db, &data_dir, true).unwrap();
        let second = scan_installed_games_with_db(&db, &data_dir, true).unwrap();
        assert_eq!(first, 2, "one extracted dir + one ZIP");
        assert_eq!(second, first, "a second scan must not shrink");

        // And the ZIP-only game keeps its flag rather than losing it.
        let installed: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT installed FROM games WHERE shortcode = 'captlsm'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(installed, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Create `path` with the length the bundled eXoDOS torrent declares for
    /// `rel`, without writing that many bytes.
    fn write_sparse_zip(path: &std::path::Path, rel: &str) -> u64 {
        let indices = scan_torrent_indices();
        let size = indices
            .get("eXoDOS")
            .and_then(|i| i.find_by_path(rel))
            .unwrap_or_else(|| panic!("{rel} missing from the bundled torrent"))
            .size;
        let f = std::fs::File::create(path).unwrap();
        f.set_len(size).unwrap();
        size
    }

    fn torrent_index_of(rel: &str) -> i64 {
        scan_torrent_indices()
            .get("eXoDOS")
            .and_then(|i| i.find_by_path(rel))
            .unwrap_or_else(|| panic!("{rel} missing from the bundled torrent"))
            .index as i64
    }

    /// The geometry that produces phantom installs: a piece is 8 MiB and most
    /// eXoDOS archives are smaller, so fetching one file delivers whichever
    /// neighbours share its pieces. A file with a piece of its own was asked
    /// for and must never be mistaken for a side effect.
    #[test]
    fn only_files_sharing_every_piece_count_as_collateral() {
        let piece = 1000u64;
        let mk = |index: usize, size: u64, offset: u64| crate::torrent::TorrentFileEntry {
            index,
            path: format!("f{index}.zip"),
            size,
            offset,
        };
        let index = TorrentIndex {
            name: "t".into(),
            // 0: 0..500, 1: 500..900 (both inside piece 0), 2: 900..3000
            // (pieces 0-2, so pieces 1 and 2 are its own).
            files: vec![mk(0, 500, 0), mk(1, 400, 500), mk(2, 2100, 900)],
            total_size: 3000,
            piece_length: piece,
        };

        let collateral = collateral_file_indices(&index, &[0usize].into_iter().collect());
        assert!(collateral.contains(&1), "a neighbour inside the same piece came along");
        assert!(
            !collateral.contains(&2),
            "a file with pieces of its own was downloaded on purpose"
        );

        // Nothing requested, nothing collateral - the guard against an empty
        // library wiping itself.
        assert!(collateral_file_indices(&index, &Default::default()).is_empty());
    }

    /// The reported bug: downloading Wolfenstein 3D put Wolfendoom in the
    /// library, because the scan read every archive on disk as an install.
    #[test]
    fn collateral_neighbours_do_not_enter_the_library() {
        let dir = std::env::temp_dir().join(format!("exodium_collateral_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("eXoDOS/eXo/eXoDOS");
        std::fs::create_dir_all(base.join("WOLF3D")).unwrap();
        write_sparse_zip(&base.join("Wolfendoom (2000).zip"), "eXo/eXoDOS/Wolfendoom (2000).zip");

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn.execute(
            r"INSERT INTO games (title, platform, language, shortcode, torrent_source,
                                 application_path, game_torrent_index, installed, in_library)
              VALUES ('Wolfenstein 3D', 'MS-DOS', 'EN', 'WOLF3D', 'eXoDOS',
                      'eXo\eXoDOS\!dos\WOLF3D\Wolfenstein 3D (1992).bat', ?1, 0, 1)",
            rusqlite::params![torrent_index_of("eXo/eXoDOS/Wolfenstein 3D (1992).zip")],
        )
        .unwrap();
        conn.execute(
            r"INSERT INTO games (title, platform, language, shortcode, torrent_source,
                                 application_path, game_torrent_index, installed, in_library)
              VALUES ('Wolfendoom', 'MS-DOS', 'EN', 'WOLFDOOM', 'eXoDOS',
                      'eXo\eXoDOS\!dos\WOLFDOOM\Wolfendoom (2000).bat', ?1, 0, 1)",
            rusqlite::params![torrent_index_of("eXo/eXoDOS/Wolfendoom (2000).zip")],
        )
        .unwrap();
        let db = std::sync::Mutex::new(conn);
        let data_dir = dir.to_string_lossy().to_string();

        scan_installed_games_with_db(&db, &data_dir, false).unwrap();
        let (installed, in_library): (i64, i64) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT installed, in_library FROM games WHERE shortcode = 'WOLFDOOM'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(in_library, 0, "an archive nobody asked for leaves the library");
        assert_eq!(installed, 0, "and it is not an install either");

        // Asking the disk directly is the one case where it may add games.
        scan_installed_games_with_db(&db, &data_dir, true).unwrap();
        let adopted: i64 = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT installed FROM games WHERE shortcode = 'WOLFDOOM'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(adopted, 1, "Rescan and import still adopt what is on disk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An imported eXo installation is its own library: the games arrived as
    /// archives, so "no piece of this was worth fetching" describes all of
    /// them. Cleaning up there took games the user owns out of My Games on the
    /// first start after the import.
    #[test]
    fn an_imported_library_is_never_treated_as_collateral() {
        let dir = std::env::temp_dir().join(format!("exodium_imported_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("eXoDOS/eXo/eXoDOS");
        std::fs::create_dir_all(base.join("WOLF3D")).unwrap();
        write_sparse_zip(&base.join("Wolfendoom (2000).zip"), "eXo/eXoDOS/Wolfendoom (2000).zip");

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn.execute(
            "INSERT INTO config (key, value) VALUES ('library_from_disk', '1')",
            [],
        )
        .unwrap();
        conn.execute(
            r"INSERT INTO games (title, platform, language, shortcode, torrent_source,
                                 application_path, game_torrent_index, installed, in_library)
              VALUES ('Wolfenstein 3D', 'MS-DOS', 'EN', 'WOLF3D', 'eXoDOS',
                      'eXo\eXoDOS\!dos\WOLF3D\Wolfenstein 3D (1992).bat', ?1, 0, 1)",
            rusqlite::params![torrent_index_of("eXo/eXoDOS/Wolfenstein 3D (1992).zip")],
        )
        .unwrap();
        conn.execute(
            r"INSERT INTO games (title, platform, language, shortcode, torrent_source,
                                 application_path, game_torrent_index, installed, in_library)
              VALUES ('Wolfendoom', 'MS-DOS', 'EN', 'WOLFDOOM', 'eXoDOS',
                      'eXo\eXoDOS\!dos\WOLFDOOM\Wolfendoom (2000).bat', ?1, 0, 1)",
            rusqlite::params![torrent_index_of("eXo/eXoDOS/Wolfendoom (2000).zip")],
        )
        .unwrap();
        let db = std::sync::Mutex::new(conn);

        // The automatic scan, the one that would have dropped it.
        scan_installed_games_with_db(&db, &dir.to_string_lossy(), false).unwrap();
        let (installed, in_library): (i64, i64) = db
            .lock()
            .unwrap()
            .query_row(
                "SELECT installed, in_library FROM games WHERE shortcode = 'WOLFDOOM'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(in_library, 1, "an imported game stays in the library");
        assert_eq!(installed, 1, "and keeps reading as installed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A neighbouring download leaves piece-sized partials behind. Those are
    /// not installs, and the old 1 KB floor let them through.
    #[test]
    fn a_partial_archive_is_not_an_install() {
        let dir = std::env::temp_dir().join(format!("exodium_partial_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("eXoDOS/eXo/eXoDOS");
        std::fs::create_dir_all(&base).unwrap();
        let zip = base.join("Wolf (1994).zip");
        let full = write_sparse_zip(&zip, "eXo/eXoDOS/Wolf (1994).zip");
        // 8 MiB of a 376 MB archive: one piece of a neighbour's download.
        std::fs::File::create(&zip).unwrap().set_len(8 * 1024 * 1024).unwrap();
        assert!(full > 8 * 1024 * 1024);

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn.execute(
            r"INSERT INTO games (title, platform, language, shortcode, torrent_source,
                                 application_path, game_torrent_index, installed, in_library)
              VALUES ('Wolf', 'MS-DOS', 'EN', 'WOLF', 'eXoDOS',
                      'eXo\eXoDOS\!dos\WOLF\Wolf (1994).bat', ?1, 0, 1)",
            rusqlite::params![torrent_index_of("eXo/eXoDOS/Wolf (1994).zip")],
        )
        .unwrap();
        let db = std::sync::Mutex::new(conn);

        let installed_after = |adopt: bool| -> i64 {
            scan_installed_games_with_db(&db, &dir.to_string_lossy(), adopt).unwrap();
            db.lock()
                .unwrap()
                .query_row("SELECT installed FROM games WHERE shortcode = 'WOLF'", [], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        assert_eq!(installed_after(false), 0, "a partial archive is not a playable game");
        // Asking the disk directly is deliberately lenient: an imported tree
        // may hold repacked archives the bundled torrent never described, and
        // refusing those would report "no games found" on a full installation.
        assert_eq!(installed_after(true), 1, "an explicit import trusts what it finds");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Uninstall a game: back up saves, delete game files, free disk space.
#[tauri::command]
pub async fn uninstall_game(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    id: i64,
) -> Result<String, String> {
    let (game, data_dir) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        let data_dir = configured_data_dir(&conn)?;
        (game, data_dir)
    };

    if !game.installed && !game.in_library {
        // Idempotent cleanup: the UI legitimately offers Uninstall for
        // half-states (incomplete download, failed extraction) where the
        // flags are already clear but files may exist on disk. Proceed and
        // clean whatever is there instead of erroring.
        log::info!(
            "uninstall_game: {} not marked installed - cleaning up leftovers anyway",
            game.title
        );
    }

    if running_games()
        .lock()
        .map(|s| s.contains(&running_game_key(&game)))
        .unwrap_or(false)
    {
        return Err(format!(
            "'{}' is currently running - quit DOSBox before uninstalling.",
            game.title
        ));
    }

    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;

    let shortcode = game.shortcode.as_deref()
        .ok_or("Game has no shortcode")?
        .to_string();

    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let torrent_root = crate::commands::paths::game_root(&data_dir);

    // Get game name from bat filename for ZIP deletion
    let game_name = game.application_path.as_deref()
        .and_then(crate::commands::library::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());

    // Determine THIS variant's game directory:
    // EN: <game_prefix>/<shortcode>/   LP: <game_prefix>/<lang_dir>/<shortcode>/
    // eXoWin9x: <game_prefix>/<year>/<title dir>/
    // Never probe other languages' dirs - the old first-existing probe over
    // all lang dirs made "uninstall the DE variant" back up and delete the
    // EN install when both were on disk.
    let rel_game_dir =
        collection_rel_game_dir(source, &shortcode, game.application_path.as_deref());
    // Save backup lives NEXT TO the game dir (`.../!save/<shortcode>`), which
    // keeps it lang-scoped for LP variants and year-scoped for eXoWin9x -
    // exactly where extract_game_zip's restore probe looks.
    let rel_save_dir = match rel_game_dir.rsplit_once('/') {
        Some((parent, _)) => format!("{}/!save/{}", parent, shortcode),
        None => format!("!save/{}", shortcode),
    };
    let game_dir: Option<PathBuf> = Some(torrent_root.join(&rel_game_dir)).filter(|d| d.exists());
    let rel_zip = collection_rel_zip(source, &game_name, game.application_path.as_deref());

    let db_path = {
        let conn = db_state.lock()?;
        conn.path().map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    let deleted_rels: Vec<String> = tauri::async_runtime::spawn_blocking(move || {
        if let Some(ref dir) = game_dir {
            if dir.exists() {
                // Back up the entire game directory (preserves saves, configs,
                // etc.). LP backups live under the language dir so uninstalling
                // one variant can't clobber another's backup; extract_game_zip's
                // restore probes both the lang-scoped and the legacy shared
                // location. For eXoWin9x the whole dir includes the game's own
                // VHD, which is where its saves live.
                let save_dir = torrent_root.join(&rel_save_dir);
                if save_dir.exists() {
                    let _ = std::fs::remove_dir_all(&save_dir);
                }
                // Rename is the fastest way to "back up" - atomic move
                if let Err(e) = std::fs::rename(dir, &save_dir) {
                    // Rename failed (cross-device?), fall back to copy + delete
                    log::warn!("Rename to save dir failed ({}), falling back to copy", e);
                    if let Err(e) = copy_dir_recursive(dir, &save_dir) {
                        log::error!(
                            "Failed to back up game directory '{}': {} - keeping originals in place",
                            dir.display(), e
                        );
                        // Don't delete the source if backup failed
                    } else {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                }
                log::info!("Backed up saves to {}", save_dir.display());
            }
        }

        // Track which ZIPs actually got deleted (torrent-relative paths) so
        // the caller can reset piece bookkeeping in exactly the torrents
        // that tracked them. Only THIS variant's ZIP - the old all-languages
        // sweep deleted neighbor variants' downloads too.
        let zip_rels = vec![rel_zip];
        let mut deleted_rels: Vec<String> = Vec::new();
        for rel in &zip_rels {
            let zip = torrent_root.join(rel);
            if zip.exists() && std::fs::remove_file(&zip).is_ok() {
                deleted_rels.push(rel.clone());
            }
        }

        if let Ok(conn) = db::open(&db_path) {
            if let Err(e) = queries::set_game_installed(&conn, id, false) {
                log::error!("Failed to update uninstall status: {}", e);
            }
            // Also clear in_library
            let _ = conn.execute(
                "UPDATE games SET in_library = 0 WHERE id = ?1",
                rusqlite::params![id],
            );
        } else {
            log::error!("Failed to open DB for uninstall update");
        }

        deleted_rels
    })
    .await
    .map_err(|e| e.to_string())?;

    // Deleted ZIPs' pieces are still marked "had" in librqbit's fastresume
    // state; a later re-download would report 100% instantly with no file on
    // disk (stuck-download loop). All collections overlay one root, so a
    // deleted ZIP may be tracked by a torrent OTHER than this game's source
    // (e.g. a GLP uninstall also removes the EN ZIP). Reset exactly the
    // torrents that tracked a deleted path.
    let managers: Vec<(String, std::sync::Arc<crate::torrent::manager::DownloadManager>)> = {
        let guard = torrent_state.0.read().await;
        guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };

    // Only deselect this game's shared GameData when no other variant still
    // wants it (mirrors cancel_download).
    let gamedata_drop: Option<usize> = match game.gamedata_torrent_index {
        Some(gd) => {
            let conn = db_state.lock()?;
            let still_needed: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM games \
                     WHERE gamedata_torrent_index = ?1 AND id != ?2 AND in_library = 1",
                    rusqlite::params![gd, id],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if still_needed > 0 { None } else { Some(gd as usize) }
        }
        None => None,
    };

    for (col_id, mgr) in managers {
        // Files of this torrent that were genuinely deleted from disk -
        // only these get their ledger pieces cleared.
        let deleted_indices: Vec<usize> = deleted_rels
            .iter()
            .filter_map(|rel| mgr.index().find_by_path(rel).map(|f| f.index))
            .collect();
        let mut drop_indices = deleted_indices.clone();
        let is_source = col_id == source;
        if is_source {
            if let Some(gi) = game.game_torrent_index {
                let gi = gi as usize;
                if !drop_indices.contains(&gi) {
                    drop_indices.push(gi);
                }
            }
            // The shared GameData ZIP stays ON DISK - deselect it (when no
            // sibling needs it) but never clear its ledger pieces, or the
            // next install re-downloads gigabytes it already has.
            if let Some(gd) = gamedata_drop {
                if !drop_indices.contains(&gd) {
                    drop_indices.push(gd);
                }
            }
        }

        let tracked_deleted = !deleted_indices.is_empty();
        if tracked_deleted {
            // Disk state changed under this torrent - full invalidation.
            if let Err(e) = mgr
                .invalidate_after_file_delete(&drop_indices, &deleted_indices)
                .await
            {
                log::warn!("Failed to reset {} torrent state after uninstall: {}", col_id, e);
            }
        } else if is_source {
            // Nothing deleted from this torrent's files; just drop the
            // selection so the re-add doesn't fetch the uninstalled game.
            for idx in drop_indices {
                mgr.deselect_file(idx).await;
            }
        }
    }

    Ok(format!("Uninstalled: {}", game.title))
}

/// Put a game back into the state it had right after installing: drop the
/// extracted directory AND its save backup, then unpack the ZIP again.
///
/// Uninstall deliberately KEEPS user data (§5) - it renames the game dir to
/// `!save/<shortcode>` and `extract_game_zip` restores it on the next
/// install - so "uninstall, then reinstall" cannot produce a clean slate.
/// This is the operation that can.
///
/// It matters beyond savegames for eXoWin9x, where the game's own VHD is
/// mounted as D: and holds the guest's filesystem: Windows clears that
/// volume's FAT "clean shutdown" bit on the first write and only restores it
/// on a proper shutdown, so a session ended by closing the emulator window
/// leaves it dirty and the next boot runs ScanDisk. Replacing the VHD with
/// the one from the ZIP is the only way back to a pristine volume.
///
/// The ZIP is validated BEFORE anything is deleted - a torrent placeholder
/// or a half-downloaded archive must not cost the user their install.
#[tauri::command]
pub async fn reset_game_data(db_state: State<'_, DbState>, id: i64) -> Result<String, String> {
    let (game, data_dir) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game {} not found", id))?;
        let data_dir = configured_data_dir(&conn)?;
        (game, data_dir)
    };

    if running_games()
        .lock()
        .map(|s| s.contains(&running_game_key(&game)))
        .unwrap_or(false)
    {
        return Err(format!(
            "'{}' is currently running - quit the emulator before resetting it.",
            game.title
        ));
    }

    let op_lock = game_op_lock(id);
    let _op_guard = op_lock.lock().await;

    let shortcode = game.shortcode.as_deref().ok_or("Game has no shortcode")?.to_string();
    let source = game.torrent_source.as_deref().unwrap_or("eXoDOS");
    let torrent_root = crate::commands::paths::game_root(&data_dir);
    let game_name = game
        .application_path
        .as_deref()
        .and_then(crate::commands::library::game_name_from_app_path)
        .unwrap_or_else(|| game.title.clone());

    let rel_game_dir =
        collection_rel_game_dir(source, &shortcode, game.application_path.as_deref());
    let rel_save_dir = match rel_game_dir.rsplit_once('/') {
        Some((parent, _)) => format!("{}/!save/{}", parent, shortcode),
        None => format!("!save/{}", shortcode),
    };
    let zip = torrent_root.join(collection_rel_zip(
        source,
        &game_name,
        game.application_path.as_deref(),
    ));
    let game_dir = torrent_root.join(&rel_game_dir);
    let save_dir = torrent_root.join(&rel_save_dir);
    let title = game.title.clone();

    tauri::async_runtime::spawn_blocking(move || {
        // Validate first: opening the archive reads its central directory,
        // which is exactly what distinguishes a real ZIP from librqbit's
        // 0-byte placeholder or a piece-sized fragment.
        let file = std::fs::File::open(&zip).map_err(|_| {
            format!(
                "The ZIP for '{}' is not on disk, so there is nothing to restore from. \
                 Re-download the game instead.",
                title
            )
        })?;
        zip::ZipArchive::new(file).map_err(|_| {
            format!(
                "The ZIP for '{}' is incomplete or corrupted (torrent placeholder), \
                 so there is nothing to restore from. Re-download the game instead.",
                title
            )
        })?;

        if game_dir.exists() {
            std::fs::remove_dir_all(&game_dir)
                .map_err(|e| format!("Failed to remove {}: {e}", game_dir.display()))?;
        }
        // Must go too, or extract_game_zip restores the very data this is
        // meant to discard.
        if save_dir.exists() {
            let _ = std::fs::remove_dir_all(&save_dir);
        }

        let dest = game_dir.parent().map(PathBuf::from).unwrap_or_else(|| torrent_root.clone());
        extract_game_zip(&zip, &dest).map_err(String::from)
    })
    .await
    .map_err(|e| format!("reset task failed: {e}"))??;

    {
        let conn = db_state.lock()?;
        let _ = queries::set_game_installed(&conn, id, true);
    }

    log::info!("Reset game data: {}", game.title);
    Ok(format!("Reset {} to its original state", game.title))
}
