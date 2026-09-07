//! E2E fixture: put the app into a configured, OFFLINE state without the
//! setup wizard, so `tauri-driver` can start straight into the library.
//!
//! Run from src-tauri/:  EXODIUM_E2E=1 cargo run --example e2e_seed -- <game data dir>
//!
//! Writes the REAL app data directory (Tauri derives it from the identifier;
//! it cannot be redirected), replacing whatever database is there. Hence the
//! `EXODIUM_E2E=1` guard: this is for throwaway machines, never a dev box.
//! An EXAMPLE for the same reason `generate_db` is one - a `[[bin]]` would
//! ship inside the installer.

use std::path::{Path, PathBuf};

use exodium_lib::db::queries::set_config;
use exodium_lib::install_bundled_db;

/// Tauri's `app_data_dir()` for the identifier, per platform.
fn app_data_dir() -> Result<PathBuf, String> {
    let base = if cfg!(target_os = "windows") {
        std::env::var("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var("XDG_DATA_HOME").map(PathBuf::from).or_else(|_| {
            std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share"))
        })
    }
    .map_err(|e| format!("cannot resolve the app data directory: {e}"))?;
    Ok(base.join("com.redfox.exodium"))
}

fn main() -> Result<(), String> {
    if std::env::var("EXODIUM_E2E").as_deref() != Ok("1") {
        return Err("refusing to replace the app database: set EXODIUM_E2E=1 on a throwaway machine".into());
    }
    let data_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: e2e_seed <game data dir>")?;
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let data_dir = strip_verbatim(&data_dir.canonicalize().map_err(|e| e.to_string())?);

    let app_dir = app_data_dir()?;
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    let db_path = app_dir.join("exodium.db");
    install_bundled_db(&db_path)?;

    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    // Every key the startup path reads, so no dialog can sit over the grid:
    // no wizard (data_dir + catalogue), no torrent session (offline), no
    // seeding-consent dialog (key present), no welcome modal, and an explicit
    // root so `repair_legacy_root` does not mistake the fixture for a
    // pre-single-root install (§2).
    let rows: [(&str, String); 6] = [
        ("data_dir", data_dir.to_string_lossy().into_owned()),
        ("root_folder", "eXoDOS".into()),
        ("network_mode", "offline".into()),
        ("seeding_enabled", "0".into()),
        ("welcome_seen", "1".into()),
        ("collections", "eXoDOS,eXoDOS_GLP,eXoDOS_PLP,eXoDOS_SLP,eXoWin3x,eXoWin9x,eXoScummVM".into()),
    ];
    for (key, value) in &rows {
        set_config(&conn, key, value).map_err(|e| e.to_string())?;
    }

    println!("db:       {}", db_path.display());
    println!("data_dir: {}", data_dir.display());
    Ok(())
}

/// `canonicalize` on Windows yields `\\?\C:\...`; store the plain form.
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}
