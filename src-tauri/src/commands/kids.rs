//! Kids mode (#44, §26): with it on, every start shows only the games of one
//! user playlist, and everything else waits behind a PIN. A child lock, not a
//! security boundary - whoever reaches the database can turn it off - so the
//! PIN is hashed to keep it out of plain sight and a wait after wrong guesses
//! makes trying them pointless.
//!
//! The lock lives here, not only in the UI: the commands that download, delete
//! or write settings refuse while it is set, so a control the kids screen
//! forgot to hide still does nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;

use super::DbState;
use crate::db::queries;
use crate::models::Game;

/// Start option that turns kids mode off (a forgotten PIN); the playlist stays.
pub const RESET_FLAG: &str = "--reset-kids-mode";

const MODE: &str = "kids_mode";
const PLAYLIST: &str = "kids_playlist";
const LANGUAGE: &str = "kids_language";
const PIN: &str = "kids_pin";
const FAILURES: &str = "kids_failures";
const RETRY_AT: &str = "kids_retry_at";
const KEYS: [&str; 6] = [MODE, PLAYLIST, LANGUAGE, PIN, FAILURES, RETRY_AT];

/// Name of the playlist kids mode creates when none is chosen.
const DEFAULT_PLAYLIST: &str = "Kids";

static LOCKED: AtomicBool = AtomicBool::new(false);

pub fn is_locked() -> bool {
    LOCKED.load(Ordering::SeqCst)
}

/// The guard every command that downloads, deletes or writes settings calls.
pub fn ensure_unlocked() -> Result<(), String> {
    if is_locked() {
        return Err("Exodium is locked in kids mode - unlock it first.".into());
    }
    Ok(())
}

/// Kids keys are written by this module only; `set_config` refuses them.
pub fn is_kids_key(key: &str) -> bool {
    key.starts_with("kids_")
}

fn config(conn: &Connection, key: &str) -> Option<String> {
    queries::get_config(conn, key).ok().flatten()
}

fn enabled(conn: &Connection) -> bool {
    config(conn, MODE).as_deref() == Some("1")
}

fn playlist_id(conn: &Connection) -> Option<i64> {
    config(conn, PLAYLIST).and_then(|v| v.parse().ok())
}

/// At startup, before any command runs: every start with kids mode on is
/// locked. `reset` is the start option.
pub(crate) fn init(conn: &Connection, reset: bool) {
    if reset {
        clear(conn);
        log::info!("Kids mode turned off by {RESET_FLAG}");
    }
    LOCKED.store(enabled(conn), Ordering::SeqCst);
}

/// Kids mode off: the keys go, the playlist stays.
pub(crate) fn clear(conn: &Connection) {
    for key in KEYS {
        let _ = conn.execute("DELETE FROM config WHERE key = ?1", params![key]);
    }
    LOCKED.store(false, Ordering::SeqCst);
}

/// Deleting the kids playlist leaves kids mode nothing to show; it goes too.
pub(crate) fn on_playlist_deleted(conn: &Connection, id: i64) {
    if playlist_id(conn) == Some(id) {
        clear(conn);
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(salt: &str, pin: &str) -> String {
    hex(&Sha256::new().chain_update(salt.as_bytes()).chain_update(pin.as_bytes()).finalize())
}

fn valid_pin(pin: &str) -> bool {
    (4..=8).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

/// `salt$sha256(salt || pin)`, hex.
fn pin_record(pin: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| e.to_string())?;
    let salt = hex(&salt);
    let hash = digest(&salt, pin);
    Ok(format!("{salt}${hash}"))
}

fn pin_matches(conn: &Connection, pin: &str) -> bool {
    config(conn, PIN)
        .and_then(|r| r.split_once('$').map(|(salt, hash)| digest(salt, pin) == hash))
        .unwrap_or(false)
}

/// Seconds to wait after `failures` wrong PINs in a row: none for the first
/// four, then 30 s, doubling with every further miss, at most 15 minutes.
pub(crate) fn backoff_secs(failures: u32) -> u64 {
    if failures < 5 {
        return 0;
    }
    (30u64 << (failures - 5).min(5)).min(900)
}

fn retry_in(conn: &Connection) -> u64 {
    config(conn, RETRY_AT).and_then(|v| v.parse::<u64>().ok()).map(|at| at.saturating_sub(now())).unwrap_or(0)
}

#[derive(Debug, Serialize)]
pub struct KidsStatus {
    pub enabled: bool,
    pub locked: bool,
    pub playlist_id: Option<i64>,
    pub language: String,
    /// Seconds until the next PIN may be tried.
    pub retry_in: u64,
}

fn status(conn: &Connection) -> KidsStatus {
    KidsStatus {
        enabled: enabled(conn),
        locked: is_locked(),
        playlist_id: playlist_id(conn),
        language: config(conn, LANGUAGE).unwrap_or_else(|| "EN".into()),
        retry_in: retry_in(conn),
    }
}

fn set(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    queries::set_config(conn, key, value).map_err(|e| e.to_string())
}

/// The chosen user playlist, or the "Kids" one - found or created.
fn resolve_playlist(conn: &Connection, id: Option<i64>) -> Result<i64, String> {
    let kind_of = |id: i64| -> Option<String> {
        conn.query_row("SELECT kind FROM playlists WHERE id = ?1", params![id], |r| r.get(0)).ok()
    };
    match id {
        Some(id) if kind_of(id).as_deref() == Some("user") => Ok(id),
        Some(_) => Err("Kids mode needs one of your own playlists".into()),
        None => {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM playlists WHERE kind = 'user' AND name = ?1",
                    params![DEFAULT_PLAYLIST],
                    |r| r.get(0),
                )
                .ok();
            match existing {
                Some(id) => Ok(id),
                None => queries::create_playlist(conn, DEFAULT_PLAYLIST).map_err(|e| e.to_string()),
            }
        }
    }
}

#[tauri::command]
pub async fn kids_status(state: State<'_, DbState>) -> Result<KidsStatus, String> {
    let conn = state.lock()?;
    Ok(status(&conn))
}

/// Turn kids mode on. It locks at the next start, or when the lock is pressed.
#[tauri::command]
pub async fn kids_enable(
    state: State<'_, DbState>,
    pin: String,
    playlist_id: Option<i64>,
    language: String,
) -> Result<KidsStatus, String> {
    ensure_unlocked()?;
    if !valid_pin(&pin) {
        return Err("The PIN must be 4 to 8 digits".into());
    }
    let conn = state.lock()?;
    let playlist = resolve_playlist(&conn, playlist_id)?;
    set(&conn, PIN, &pin_record(&pin)?)?;
    set(&conn, PLAYLIST, &playlist.to_string())?;
    set(&conn, LANGUAGE, &language)?;
    set(&conn, MODE, "1")?;
    let _ = conn.execute("DELETE FROM config WHERE key IN (?1, ?2)", params![FAILURES, RETRY_AT]);
    Ok(status(&conn))
}

/// Playlist and language; reachable only unlocked, so no PIN is asked.
#[tauri::command]
pub async fn kids_update(
    state: State<'_, DbState>,
    playlist_id: i64,
    language: String,
) -> Result<KidsStatus, String> {
    ensure_unlocked()?;
    let conn = state.lock()?;
    if !enabled(&conn) {
        return Err("Kids mode is off".into());
    }
    let playlist = resolve_playlist(&conn, Some(playlist_id))?;
    set(&conn, PLAYLIST, &playlist.to_string())?;
    set(&conn, LANGUAGE, &language)?;
    Ok(status(&conn))
}

#[tauri::command]
pub async fn kids_change_pin(state: State<'_, DbState>, old_pin: String, new_pin: String) -> Result<(), String> {
    ensure_unlocked()?;
    let conn = state.lock()?;
    if !pin_matches(&conn, &old_pin) {
        return Err("Wrong PIN".into());
    }
    if !valid_pin(&new_pin) {
        return Err("The PIN must be 4 to 8 digits".into());
    }
    set(&conn, PIN, &pin_record(&new_pin)?)
}

#[tauri::command]
pub async fn kids_disable(state: State<'_, DbState>, pin: String) -> Result<KidsStatus, String> {
    ensure_unlocked()?;
    let conn = state.lock()?;
    if !pin_matches(&conn, &pin) {
        return Err("Wrong PIN".into());
    }
    clear(&conn);
    Ok(status(&conn))
}

#[derive(Debug, Serialize)]
pub struct UnlockResult {
    pub ok: bool,
    pub retry_in: u64,
}

#[tauri::command]
pub async fn kids_unlock(state: State<'_, DbState>, pin: String) -> Result<UnlockResult, String> {
    let conn = state.lock()?;
    let wait = retry_in(&conn);
    if wait > 0 {
        return Ok(UnlockResult { ok: false, retry_in: wait });
    }
    if pin_matches(&conn, &pin) {
        let _ = conn.execute("DELETE FROM config WHERE key IN (?1, ?2)", params![FAILURES, RETRY_AT]);
        LOCKED.store(false, Ordering::SeqCst);
        return Ok(UnlockResult { ok: true, retry_in: 0 });
    }
    let failures = config(&conn, FAILURES).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) + 1;
    set(&conn, FAILURES, &failures.to_string())?;
    let wait = backoff_secs(failures);
    if wait > 0 {
        set(&conn, RETRY_AT, &(now() + wait).to_string())?;
    }
    Ok(UnlockResult { ok: false, retry_in: wait })
}

#[tauri::command]
pub async fn kids_lock(state: State<'_, DbState>) -> Result<KidsStatus, String> {
    let conn = state.lock()?;
    if enabled(&conn) {
        LOCKED.store(true, Ordering::SeqCst);
    }
    Ok(status(&conn))
}

/// The variant a child's click starts: the kids language if installed, else
/// English, else any installed one. None when nothing of the group is.
fn launch_variant(variants: &[Game], language: &str) -> Option<Game> {
    let installed = |lang: Option<&str>| {
        variants.iter().find(|v| v.installed && lang.is_none_or(|l| v.language.eq_ignore_ascii_case(l)))
    };
    installed(Some(language)).or_else(|| installed(Some("EN"))).or_else(|| installed(None)).cloned()
}

/// What the kids screen shows: the playlist's installed games, one per group,
/// in the order they were added.
#[tauri::command]
pub async fn kids_games(state: State<'_, DbState>) -> Result<Vec<Game>, String> {
    let conn = state.lock()?;
    let Some(playlist) = playlist_id(&conn) else { return Ok(Vec::new()) };
    let language = config(&conn, LANGUAGE).unwrap_or_else(|| "EN".into());
    Ok(super::playlist_io::playlist_groups(&conn, playlist, false)?
        .iter()
        .filter_map(|group| launch_variant(group, &language))
        .collect())
}

/// While locked, only the kids playlist's games start.
pub(crate) fn ensure_launch_allowed(conn: &Connection, game_id: i64) -> Result<(), String> {
    if !is_locked() {
        return Ok(());
    }
    let allowed = playlist_id(conn)
        .is_some_and(|p| queries::fetch_game_playlist_ids(conn, game_id).unwrap_or_default().contains(&p));
    if allowed {
        Ok(())
    } else {
        Err("This game is not in the kids playlist.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        conn
    }

    #[test]
    fn the_pin_is_stored_hashed_and_checked() {
        let conn = db();
        set(&conn, PIN, &pin_record("4711").unwrap()).unwrap();
        let stored = config(&conn, PIN).unwrap();
        assert!(!stored.contains("4711"));
        assert!(pin_matches(&conn, "4711"));
        assert!(!pin_matches(&conn, "4712"));
        assert_ne!(pin_record("4711").unwrap(), stored, "salted: the same PIN hashes differently");
    }

    #[test]
    fn pins_are_four_to_eight_digits() {
        assert!(valid_pin("0000") && valid_pin("12345678"));
        assert!(!valid_pin("123") && !valid_pin("123456789") && !valid_pin("12a4"));
    }

    #[test]
    fn wrong_pins_cost_a_growing_wait() {
        assert_eq!((1..=4).map(backoff_secs).collect::<Vec<_>>(), [0, 0, 0, 0]);
        assert_eq!(backoff_secs(5), 30);
        assert_eq!(backoff_secs(6), 60);
        assert_eq!(backoff_secs(20), 900, "capped at 15 minutes");
    }

    #[test]
    fn a_click_starts_the_kids_language_then_english_then_anything_installed() {
        let v = |lang: &str, installed: bool| Game { language: lang.into(), installed, ..Game::default() };
        let pick = |vs: &[Game], l: &str| launch_variant(vs, l).map(|g| g.language);
        assert_eq!(pick(&[v("EN", true), v("DE", true)], "DE").as_deref(), Some("DE"));
        assert_eq!(pick(&[v("EN", true), v("DE", false)], "DE").as_deref(), Some("EN"));
        assert_eq!(pick(&[v("EN", false), v("ES", true)], "DE").as_deref(), Some("ES"));
        assert_eq!(pick(&[v("EN", false)], "EN"), None, "nothing installed: not shown");
    }

    #[test]
    fn deleting_the_kids_playlist_turns_kids_mode_off() {
        let conn = db();
        let id = resolve_playlist(&conn, None).unwrap();
        assert_eq!(resolve_playlist(&conn, None).unwrap(), id, "the Kids playlist is found, not duplicated");
        set(&conn, MODE, "1").unwrap();
        set(&conn, PLAYLIST, &id.to_string()).unwrap();
        on_playlist_deleted(&conn, id + 1);
        assert!(enabled(&conn));
        on_playlist_deleted(&conn, id);
        assert!(!enabled(&conn) && config(&conn, PLAYLIST).is_none());
    }
}
