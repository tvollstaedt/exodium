//! What the user owns: matching catalogue rows to torrent files, deciding
//! which archives on disk are installs (§4), uninstall with save backup and
//! reset (§5).

use std::path::{Path, PathBuf};

use tauri::State;

use crate::db;
use crate::db::queries;
use crate::torrent::TorrentIndex;

use super::collections::{collection_rel_game_dir, collection_rel_zip, COLLECTION_MAP};
use super::games::{configured_data_dir, game_op_lock, running_game_key, running_games};
use super::install::{copy_dir_recursive, extract_game_zip};
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

/// Zip names to look for in a torrent, best first: the launcher bat's name
/// (`<Title> (<Year>)`), else the eXo name rebuilt from title + year (three
/// GLP rows have no path, issue #26). Shared with generate_db.
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

/// Files that can only have arrived as a side effect of `requested`: every
/// piece they occupy belongs to a requested file too (§4). Decided from the
/// torrent's geometry - the archives themselves are complete and intact.
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

/// Drop library entries whose archive is collateral (§4). Runs between the
/// scan passes: after pass 1's "extracted = installed" verdict, before pass 2
/// would confirm the rows. Support archives count as requested.
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
            // Bytes on disk are enough: a util.zip still downloading is
            // exactly when its neighbours appear.
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

/// `eXo/eXoWin9x/<year>/`: the one directory shape that holds zips a level down.
fn is_year_dir(p: &Path) -> bool {
    p.file_name()
        .map(|n| n.to_string_lossy())
        .is_some_and(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Mark games whose files are on disk as installed; returns rows updated.
/// `adopt_from_disk` lets a bare archive CREATE a library entry - true for
/// import, Rescan and a data-dir change, never for the startup scan (§4).
/// `in_flight`: torrent-relative paths of archives the live session is still
/// downloading. librqbit writes them sparse, so they reach full length long
/// before they are complete - size alone would confirm a half-fetched game.
pub(crate) fn scan_installed_games_with_db(
    db: &std::sync::Mutex<rusqlite::Connection>,
    data_dir: &str,
    adopt_from_disk: bool,
    in_flight: &std::collections::HashSet<String>,
) -> Result<usize, String> {
    // Game dirs: eXo/eXoDOS/[<lang>/]<shortcode>, eXo/eXoWin3x/<shortcode>,
    // eXo/eXoWin9x/<year>/<title dir>. The `!`-prefixed config dirs always
    // exist and never count.
    let game_base = game_root(data_dir).join("eXo").join("eXoDOS");

    // No collection tree at all (unmounted drive, typo): refuse, or the
    // flag reset below reports the whole library gone.
    if !COLLECTION_MAP.iter().any(|c| {
        game_root(data_dir)
            .join(c.game_prefix)
            .is_dir()
    }) {
        return Err(format!(
            "No game folders found under {data_dir}. Check the data directory in Settings."
        ));
    }

    // Reset installed (a removed dir must flip back); in_library is sticky
    // and holds the progress card of a download in flight.
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
    // An imported installation IS its library: every archive there reads
    // as collateral, so the cleanup never runs on it.
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

    // Pass 2: zips not yet extracted (LaunchBox unpacks on first launch).
    // Base collections only - an LP row sharing the EN title would otherwise
    // be marked installed by the EN zip; LP installs need their dir (pass 1).
    if game_base.is_dir() {
        // Build lookup: zip_stem → game_id, restricted to non-LP collections.
        let lp_sources: Vec<String> = COLLECTION_MAP
            .iter()
            .filter(|c| c.lang_dir.is_some())
            .map(|c| c.id.to_string())
            .collect();
        let lp_placeholders = lp_sources.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        // Every not-yet-matched base-collection row; filtering on in_library
        // here made the scan non-idempotent (test: rescanning_is_idempotent).
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
                        // No torrent to measure against, or an import that
                        // may hold repacked archives: size cannot decide.
                        if lenient_sizes {
                            return len >= 1024;
                        }
                        let Ok(rel) = path.strip_prefix(&torrent_root) else {
                            return false;
                        };
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if in_flight.contains(&rel) {
                            return false;
                        }
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
                .filter(|e| e.path().is_dir() && is_year_dir(&e.path()))
            {
                zip_ids.extend(collect_zip_ids(&year_dir.path()));
            }
        }

        // A `<zip>.extracting` lock older than the poll's own stale limit
        // marks an extraction that died with the process; its half game dir
        // passed pass 1. Unmark the row and drop the lock - the tracker the
        // frontend re-arms extracts again from the complete archive.
        let mut crashed: Vec<i64> = Vec::new();
        let mut scan_dirs = vec![game_base.clone()];
        for col in COLLECTION_MAP.iter().filter(|c| c.year_subdirs) {
            if let Ok(entries) = std::fs::read_dir(game_root(data_dir).join(col.game_prefix)) {
                scan_dirs.extend(
                    entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir() && is_year_dir(p)),
                );
            }
        }
        for dir in &scan_dirs {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            for e in entries.filter_map(|e| e.ok()) {
                let path = e.path();
                if path.extension().is_none_or(|x| x != "extracting") {
                    continue;
                }
                let stale = e.metadata().and_then(|m| m.modified()).ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 300);
                if !stale {
                    continue;
                }
                if let Some(id) = path.file_stem().and_then(|s| name_to_id.get(&*s.to_string_lossy())) {
                    crashed.push(*id);
                }
                let _ = std::fs::remove_file(&path);
            }
        }
        zip_ids.retain(|id| !crashed.contains(id));
        if !crashed.is_empty() {
            let placeholders = crashed.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let conn = db.lock().map_err(|e| e.to_string())?;
            let _ = conn.execute(
                &format!("UPDATE games SET installed = 0 WHERE id IN ({})", placeholders),
                rusqlite::params_from_iter(crashed.iter()),
            );
            log::warn!("scan_installed_games: {} extractions died mid-way - unmarked, will extract again", crashed.len());
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

    // Library entries without files: after a folder move these are games
    // left behind, and the count feeds the warning.
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

/// Re-scan the game root. `adopt` (Rescan button, data-dir change) lets the
/// disk add games to the library; the startup scan passes false.
#[tauri::command]
pub async fn scan_installed_games(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, super::TorrentState>,
    adopt: Option<bool>,
) -> Result<usize, String> {
    let data_dir = {
        let conn = db_state.lock()?;
        crate::commands::games::configured_data_dir(&conn)?
    };
    let pending = crate::commands::install::pending_downloads(&db_state.0, &torrent_state).await?;
    // Rows an earlier scan confirmed off a sparse, half-fetched archive.
    let false_installs: Vec<i64> = pending.iter().filter(|d| d.installed && !d.complete).map(|d| d.id).collect();
    if !false_installs.is_empty() {
        let placeholders = false_installs.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let conn = db_state.lock()?;
        conn.execute(
            &format!("UPDATE games SET installed = 0 WHERE id IN ({})", placeholders),
            rusqlite::params_from_iter(false_installs.iter()),
        )
        .map_err(|e| e.to_string())?;
        log::info!("scan_installed_games: {} rows reset - archive still downloading", false_installs.len());
    }
    let in_flight: std::collections::HashSet<String> =
        pending.into_iter().filter(|d| !d.complete).map(|d| d.rel_path).collect();
    scan_installed_games_with_db(&db_state.0, &data_dir, adopt.unwrap_or(false), &in_flight)
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


    /// A second scan must report the same games as the first.
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
        assert!(scan_installed_games_with_db(&db, "/nonexistent/exodium", true, &Default::default()).is_err());

        let first = scan_installed_games_with_db(&db, &data_dir, true, &Default::default()).unwrap();
        let second = scan_installed_games_with_db(&db, &data_dir, true, &Default::default()).unwrap();
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

    /// A file with a piece of its own was asked for; only a file whose every
    /// piece is shared is collateral.
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

        scan_installed_games_with_db(&db, &data_dir, false, &Default::default()).unwrap();
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
        scan_installed_games_with_db(&db, &data_dir, true, &Default::default()).unwrap();
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

    /// An imported installation is never cleaned: every archive there would
    /// read as collateral.
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
        scan_installed_games_with_db(&db, &dir.to_string_lossy(), false, &Default::default()).unwrap();
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
            scan_installed_games_with_db(&db, &dir.to_string_lossy(), adopt, &Default::default()).unwrap();
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

    /// A full-length archive the session is still fetching is sparse: the
    /// size test passes long before the pieces are in.
    #[test]
    fn an_archive_still_downloading_is_not_an_install() {
        let dir = std::env::temp_dir().join(format!("exodium_inflight_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("eXoDOS/eXo/eXoDOS");
        std::fs::create_dir_all(&base).unwrap();
        write_sparse_zip(&base.join("Wolf (1994).zip"), "eXo/eXoDOS/Wolf (1994).zip");

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
        let installed = |in_flight: &std::collections::HashSet<String>| -> i64 {
            scan_installed_games_with_db(&db, &dir.to_string_lossy(), false, in_flight).unwrap();
            db.lock()
                .unwrap()
                .query_row("SELECT installed FROM games WHERE shortcode = 'WOLF'", [], |r| r.get(0))
                .unwrap()
        };
        let in_flight: std::collections::HashSet<String> =
            ["eXo/eXoDOS/Wolf (1994).zip".to_string()].into_iter().collect();
        assert_eq!(installed(&in_flight), 0, "a download in flight is not an install, whatever its length");
        assert_eq!(installed(&Default::default()), 1, "the same file counts once the session is done with it");
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
        // Half-states (incomplete download, failed extraction) offer
        // Uninstall with clear flags: clean whatever is there.
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

    // THIS variant's dir only - probing other language dirs deleted the EN
    // install when uninstalling DE.
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
                // Whole-dir backup (§5), lang-scoped so variants cannot
                // clobber each other's; `extract_game_zip` restores it.
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

        // Only THIS variant's zip; deleted paths feed the ledger reset below.
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

    // Reset the ledger of every torrent that tracked a deleted zip (a GLP
    // uninstall also removes the EN zip) - see `invalidate_after_file_delete`.
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

/// Back to the freshly-installed state: delete the game dir AND its `!save`
/// backup, then unpack again. Uninstall keeps user data (§5), so this is the
/// only clean slate - and the only way to a pristine Win9x VHD. The zip is
/// validated BEFORE anything is deleted.
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

    // An overlay variant is a patch: re-extracting its archive alone leaves a
    // two-file directory marked installed. The English tree goes back down
    // first, exactly as the install path does.
    let base_src = {
        let conn = db_state.lock()?;
        match crate::commands::lp_overlay::base_for(&conn, &game) {
            Some(base) => {
                let dir = crate::commands::lp_overlay::base_game_dir(&torrent_root, &base)
                    .ok_or_else(|| {
                        format!(
                            "'{}' is a translation of '{}', which is not installed any more - reinstall the English version first.",
                            title, base.title
                        )
                    })?;
                Some((dir, base.id))
            }
            None => None,
        }
    };
    // The English tree must not be uninstalled while it is being copied.
    let _base_guard = match base_src.as_ref().and_then(|(_, id)| *id) {
        Some(base_id) => Some(game_op_lock(base_id).lock_owned().await),
        None => None,
    };

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

        if let Some((src, _)) = base_src {
            crate::commands::lp_overlay::clone_tree(&src, &game_dir).map_err(|e| {
                format!("Could not restore the English base at {}: {e}", game_dir.display())
            })?;
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
