/// Playability smoke tests — run with:
///
///   cargo test playability -- --ignored --nocapture
///
/// Requires at least one game to be installed (installed = 1 in the DB).
/// The test skips gracefully if no games are installed or the DB is absent.
///
/// For each installed game the test:
///   1. Builds the DOSBox command that `launch_game` would use.
///   2. Spawns the process.
///   3. Waits 5 s — if still alive → "started".
///      If it exited before the window with a non-zero code → "immediate_crash".
///   4. Kills the process.
///   5. Appends results to playability_report.json next to the DB.
use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use exodian_lib::collection_data_dir;
use rusqlite::Connection;
use serde::Serialize;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Minimal game info we need from the DB.
struct InstalledGame {
    id: i64,
    title: String,
    shortcode: Option<String>,
    torrent_source: String,
    dosbox_conf: Option<String>,
}

fn open_db(data_dir: &Path) -> Option<Connection> {
    let db_path = data_dir.join("exodian.db");
    if !db_path.exists() {
        return None;
    }
    Connection::open(&db_path).ok()
}

fn get_data_dir(conn: &Connection) -> Option<PathBuf> {
    conn.query_row(
        "SELECT value FROM config WHERE key = 'data_dir'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(PathBuf::from)
}

fn get_installed_games(conn: &Connection) -> Vec<InstalledGame> {
    let total: usize = conn
        .query_row("SELECT COUNT(*) FROM games WHERE installed = 1", [], |r| r.get(0))
        .unwrap_or(0);

    let mut stmt = match conn.prepare(
        "SELECT id, title, shortcode, COALESCE(torrent_source,'eXoDOS'), dosbox_conf
         FROM games WHERE installed = 1 ORDER BY title",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let games: Vec<InstalledGame> = stmt
        .query_map([], |row| {
            Ok(InstalledGame {
                id: row.get(0)?,
                title: row.get(1)?,
                shortcode: row.get(2)?,
                torrent_source: row.get(3)?,
                dosbox_conf: row.get(4)?,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    if games.len() < total {
        eprintln!(
            "[playability] Warning: only testing {} of {} installed games (query returned fewer rows)",
            games.len(), total
        );
    }

    games
}

/// Find the dosbox-staging binary: look in src-tauri/binaries/ relative to
/// the Cargo workspace, then fall back to the system PATH.
fn find_dosbox() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let binaries_dir = manifest_dir.join("binaries");
    if let Ok(entries) = std::fs::read_dir(&binaries_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("dosbox-staging") {
                return entry.path();
            }
        }
    }
    PathBuf::from(if cfg!(windows) { "dosbox-staging.exe" } else { "dosbox-staging" })
}

/// Resolve the dosbox.conf path for a game.
///
/// This is a simplified version of the path resolution in `launch_game` — it
/// covers the common EN and LP-fallback cases but does not handle language-dir
/// conf variants or the macOS shader override. Changes to `launch_game`'s path
/// logic will not automatically be reflected here.
fn resolve_conf(game: &InstalledGame, data_dir: &Path) -> Option<PathBuf> {
    let conf_rel = game.dosbox_conf.as_deref()?.replace('\\', "/");
    let data_dir_str = data_dir.to_str().unwrap_or_default();

    // Primary: collection-specific subdirectory + inner "eXoDOS" folder
    let coll_root = collection_data_dir(data_dir_str, &game.torrent_source).join("eXoDOS");
    let path = coll_root.join(&conf_rel);
    if path.exists() {
        return Some(path);
    }

    // Fallback: LP games share the EN conf in the main eXoDOS collection
    let main_path = data_dir.join("eXoDOS").join(&conf_rel);
    if main_path.exists() {
        return Some(main_path);
    }

    None
}

// ── result types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PlayResult {
    Started,
    ImmediateCrash { exit_code: Option<i32>, elapsed_ms: u64 },
    NoConfig,
    SpawnFailed { error: String },
}

#[derive(Debug, Serialize)]
struct GameReport {
    id: i64,
    title: String,
    shortcode: Option<String>,
    result: PlayResult,
}

#[derive(Debug, Serialize)]
struct PlayabilityReport {
    timestamp: u64,
    platform: &'static str,
    dosbox_bin: String,
    games: Vec<GameReport>,
}

// ── test ─────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn playability_check_installed_games() {
    // Locate the DB: first try $EXODIAN_DATA_DIR, then ~/eXoDOS
    let data_dir = std::env::var("EXODIAN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_from_home());

    let conn = match open_db(&data_dir) {
        Some(c) => c,
        None => {
            eprintln!("[playability] No database found at {}/exodian.db — skipping.", data_dir.display());
            return;
        }
    };

    // Prefer the data_dir stored in config over the env guess
    let data_dir = get_data_dir(&conn).unwrap_or(data_dir);

    let games = get_installed_games(&conn);
    if games.is_empty() {
        eprintln!("[playability] No installed games found — skipping.");
        return;
    }

    let dosbox_bin = find_dosbox();
    eprintln!("[playability] Using DOSBox: {}", dosbox_bin.display());
    eprintln!("[playability] Testing {} installed game(s)…\n", games.len());

    let mut reports: Vec<GameReport> = Vec::new();

    for game in &games {
        let conf = match resolve_conf(game, &data_dir) {
            Some(c) => c,
            None => {
                eprintln!("  [SKIP  ] {} — no conf found", game.title);
                reports.push(GameReport {
                    id: game.id,
                    title: game.title.clone(),
                    shortcode: game.shortcode.clone(),
                    result: PlayResult::NoConfig,
                });
                continue;
            }
        };

        // Build minimal DOSBox command (no -exit so DOSBox stays open long enough to probe)
        let mut child: Child = match Command::new(&dosbox_bin)
            .arg("-conf")
            .arg(&conf)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  [ERROR ] {} — spawn failed: {}", game.title, e);
                reports.push(GameReport {
                    id: game.id,
                    title: game.title.clone(),
                    shortcode: game.shortcode.clone(),
                    result: PlayResult::SpawnFailed { error: e.to_string() },
                });
                continue;
            }
        };

        let start = Instant::now();
        let verdict = loop {
            std::thread::sleep(Duration::from_millis(250));
            let elapsed = start.elapsed();

            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    let elapsed_ms = elapsed.as_millis() as u64;
                    break PlayResult::ImmediateCrash { exit_code: code, elapsed_ms };
                }
                Ok(None) => {
                    if elapsed >= Duration::from_secs(5) {
                        // Still running after 5 s — DOSBox opened successfully
                        let _ = child.kill();
                        break PlayResult::Started;
                    }
                }
                Err(_) => {
                    break PlayResult::SpawnFailed { error: "try_wait error".to_string() };
                }
            }
        };

        let symbol = match &verdict {
            PlayResult::Started => "OK    ",
            PlayResult::ImmediateCrash { .. } => "CRASH ",
            _ => "?     ",
        };
        eprintln!("  [{}] {}", symbol, game.title);

        reports.push(GameReport {
            id: game.id,
            title: game.title.clone(),
            shortcode: game.shortcode.clone(),
            result: verdict,
        });
    }

    // ── summary ──────────────────────────────────────────────────────────────
    let started = reports.iter().filter(|r| matches!(r.result, PlayResult::Started)).count();
    let crashed = reports.iter().filter(|r| matches!(r.result, PlayResult::ImmediateCrash { .. })).count();
    let no_conf = reports.iter().filter(|r| matches!(r.result, PlayResult::NoConfig)).count();
    eprintln!(
        "\n[playability] Results: {} started, {} immediate crash, {} no-config, {} total",
        started, crashed, no_conf, reports.len()
    );

    // ── write report ─────────────────────────────────────────────────────────
    let report = PlayabilityReport {
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        platform: std::env::consts::OS,
        dosbox_bin: dosbox_bin.to_string_lossy().to_string(),
        games: reports,
    };

    let report_path = data_dir.join("playability_report.json");
    let mut existing: Vec<serde_json::Value> = std::fs::read_to_string(&report_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    existing.push(serde_json::to_value(&report).unwrap());
    if let Ok(json) = serde_json::to_string_pretty(&existing) {
        let _ = std::fs::write(&report_path, json);
        eprintln!("[playability] Report written to {}", report_path.display());
    }
}

fn dirs_from_home() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("eXoDOS"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/exodian"))
}
