//! Where things live: the app's resource and log dirs, the single game root
//! (§2) and the per-launch conf dir.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tauri::AppHandle;

use crate::db::queries;

/// Tauri's resource_dir, set once at setup for callers without an AppHandle.
pub(crate) static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The log dir, set once at setup: `app_log_dir()` has failed when called
/// from a command on shipped Windows builds.
pub(crate) static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Called once from lib.rs' setup closure with the app's resource directory.
pub fn init_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

/// Called once from lib.rs' setup closure with the resolved app log directory.
pub fn init_log_dir(dir: PathBuf) {
    let _ = LOG_DIR.set(dir);
}

/// The one folder inside the data dir holding every collection, eXo's own
/// merged layout (§2). Cached: every path derivation needs it, and setup,
/// import and a data-dir change set it explicitly.
static ROOT_FOLDER: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Default when nothing was stored - what fresh installs have always used.
pub const DEFAULT_ROOT_FOLDER: &str = "eXoDOS";

/// `root_folder` value meaning "the data dir IS the root" - see
/// `repair_legacy_root`. A name can never express that, since
/// `<data>/<name>` is always one level down.
pub const ROOT_IS_DATA_DIR: &str = ".";

/// Remember the root folder name for this session (and this session only -
/// the value itself lives in the `root_folder` config key).
pub fn set_root_folder(name: &str) {
    if let Ok(mut guard) = ROOT_FOLDER.write() {
        *guard = Some(name.to_string());
    }
}

/// A pre-single-root install has its games AT the data dir; write the `.`
/// sentinel so `game_root` is the data dir itself instead of a second tree
/// inside it (§2). Keyed on `root_folder` being unset.
pub(crate) fn repair_legacy_root(conn: &rusqlite::Connection) {
    let has_root = queries::get_config(conn, "root_folder")
        .ok()
        .flatten()
        .is_some_and(|v| !v.trim().is_empty());
    if has_root {
        return;
    }
    let Ok(Some(data_dir)) = queries::get_config(conn, "data_dir") else {
        return;
    };
    let dir = PathBuf::from(&data_dir);
    // `eXo/` at the data-dir level IS the legacy layout - no other setup
    // produces it, and a fresh install's data dir never has one.
    if !dir.join("eXo").is_dir() {
        return;
    }
    log::warn!(
        "Legacy layout: {} holds the games itself - adopting it as the game root",
        data_dir
    );
    let _ = queries::set_config(conn, "root_folder", ROOT_IS_DATA_DIR);
}

/// Load `root_folder` into the cache, repairing a legacy layout first - the
/// one place every entry point passes before deriving a path.
pub fn load_root_folder(conn: &rusqlite::Connection) {
    repair_legacy_root(conn);
    let name = queries::get_config(conn, "root_folder")
        .ok()
        .flatten()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ROOT_FOLDER.to_string());
    set_root_folder(&name);
}

/// The single directory holding every collection's files (§2).
pub fn game_root(data_dir: &str) -> PathBuf {
    let name = ROOT_FOLDER
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| DEFAULT_ROOT_FOLDER.to_string());
    if name == ROOT_IS_DATA_DIR {
        return PathBuf::from(data_dir);
    }
    PathBuf::from(data_dir).join(name)
}

/// Files the OS drops into folders on its own; a merge must not count them
/// as content, or the emptied folder survives and the prompt returns.
pub(crate) fn is_os_metadata(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // `._x` is the AppleDouble sidecar a Mac writes on non-native filesystems.
    matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini") || name.starts_with("._")
}

/// Convert a PathBuf to a forward-slash string. Tauri's convertFileSrc on
/// the frontend expects consistent separators when we later join `${dir}/${file}`;
/// mixed Windows backslash + frontend forward slash produces broken asset URLs.
pub(crate) fn path_to_fwd_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The bundled `metadata/` dir: the repo tree in dev, `RESOURCE_DIR` in
/// production.
pub fn bundled_metadata_dir() -> Result<PathBuf, String> {
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("metadata"))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(dev_path);
    }

    if let Some(res_dir) = RESOURCE_DIR.get() {
        let prod_path = res_dir.join("metadata");
        if prod_path.exists() {
            return Ok(prod_path);
        }
        return Err(format!(
            "Bundled metadata not found in resource dir {} (dev path also missing: {})",
            res_dir.display(),
            dev_path.display()
        ));
    }

    Err(format!(
        "Bundled metadata not found: resource_dir uninitialized and dev path {} missing",
        dev_path.display()
    ))
}

/// Resolve bundled torrent file path.
pub(crate) fn bundled_torrent_path(filename: &str) -> Result<PathBuf, String> {
    // Dev mode reads from the repo tree.
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("torrents").join(filename))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(dev_path);
    }

    // Production reads from the Tauri resource_dir (cached at startup).
    if let Some(res_dir) = RESOURCE_DIR.get() {
        let prod_path = res_dir.join("torrents").join(filename);
        if prod_path.exists() {
            return Ok(prod_path);
        }
    }

    Err(format!(
        "Bundled torrent '{}' not found (dev path: {})",
        filename,
        dev_path.display()
    ))
}

/// Where the per-launch DOSBox conf fragments live: the app's own dir, not
/// the user's game folder (`sweep_legacy_launch_confs` removes the old ones).
pub(crate) fn launch_conf_dir(app: &AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("launch");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Remove `*.conf` files from `dir`, optionally only those with `prefix`.
/// Non-recursive and never touches subdirectories - both callers clean a flat
/// directory that also holds files belonging to someone else.
fn remove_conf_files(dir: impl AsRef<Path>, prefix: Option<&str>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".conf") {
            continue;
        }
        if prefix.is_some_and(|p| !name.starts_with(p)) {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Delete launch fragments left in the game data dir by earlier versions.
/// They are regenerated on demand, so removing them loses nothing.
pub fn sweep_legacy_launch_confs(data_dir: &str) {
    let removed = remove_conf_files(data_dir, Some("exodium_"));
    if removed > 0 {
        log::info!("Removed {} stray launch config(s) from the game folder", removed);
    }
}

/// Empty the launch-conf dir at startup, when no game of this instance runs.
pub fn prune_launch_confs(app: &AppHandle) {
    let Ok(dir) = launch_conf_dir(app) else { return };
    let removed = remove_conf_files(&dir, None);
    if removed > 0 {
        log::debug!("Cleared {} stale launch config(s)", removed);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A pre-single-root install keeps its games at the DATA DIR level, so the
    /// root has to be the folder itself - not a new one nested inside it.
    #[test]
    fn legacy_data_dir_holding_the_games_becomes_the_root() {
        let dir = std::env::temp_dir().join(format!("exodium_legacy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old_data = dir.join("eXoDOS");
        std::fs::create_dir_all(old_data.join("eXo/eXoDOS")).unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        queries::set_config(&conn, "data_dir", &old_data.to_string_lossy()).unwrap();

        repair_legacy_root(&conn);

        assert_eq!(
            queries::get_config(&conn, "data_dir").unwrap().unwrap(),
            old_data.to_string_lossy(),
            "the data dir stays put - content packs and caches hang off it"
        );
        assert_eq!(
            queries::get_config(&conn, "root_folder").unwrap().unwrap(),
            ROOT_IS_DATA_DIR
        );
        set_root_folder(ROOT_IS_DATA_DIR);
        assert_eq!(
            game_root(&old_data.to_string_lossy()),
            old_data,
            "and the root resolves to the folder the games are already in"
        );
        set_root_folder(DEFAULT_ROOT_FOLDER);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh install's data dir has no `eXo/` of its own - leave it alone.
    #[test]
    fn a_fresh_data_dir_is_not_repaired() {
        let dir = std::env::temp_dir().join(format!("exodium_fresh_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("eXoDOS/eXo")).unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        queries::set_config(&conn, "data_dir", &dir.to_string_lossy()).unwrap();

        repair_legacy_root(&conn);

        assert_eq!(
            queries::get_config(&conn, "data_dir").unwrap().unwrap(),
            dir.to_string_lossy()
        );
        assert!(queries::get_config(&conn, "root_folder").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Both cleanups run against directories that also hold the user's own
    // files, so "only ours, only .conf, never recurse" is the contract.
    #[test]
    fn remove_conf_files_respects_prefix_and_extension() {
        let dir = std::env::temp_dir().join(format!("exodium_conf_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["exodium_a.conf", "global_overrides_7.conf", "dosbox.conf", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::write(dir.join("sub").join("exodium_nested.conf"), b"x").unwrap();

        assert_eq!(super::remove_conf_files(&dir, Some("exodium_")), 1);
        assert!(!dir.join("exodium_a.conf").exists());
        // The game's own dosbox.conf must survive a prefixed sweep.
        assert!(dir.join("dosbox.conf").exists());
        assert!(dir.join("sub").join("exodium_nested.conf").exists());

        assert_eq!(super::remove_conf_files(&dir, None), 2);
        assert!(dir.join("notes.txt").exists());
        assert_eq!(super::remove_conf_files(&dir, None), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
