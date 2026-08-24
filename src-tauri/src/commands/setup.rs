use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::db::normalize_alnum;

/// Cached Tauri resource_dir(), set once during app setup. Needed because
/// sync helpers (bundled_metadata_dir, bundled_torrent_path) are called from
/// contexts that don't carry an AppHandle - and without this cache they'd
/// have to be plumbed everywhere.
pub(crate) static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Cached log directory. Set once during app setup from `app_log_dir()`.
/// Cache rather than re-resolving in the command because we observed
/// `app.path().app_log_dir()` returning errors when called from a command
/// invocation in shipped Windows builds (where setup-time resolution worked).
pub(crate) static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Called once from lib.rs' setup closure with the app's resource directory.
pub fn init_resource_dir(dir: PathBuf) {
    let _ = RESOURCE_DIR.set(dir);
}

/// Called once from lib.rs' setup closure with the resolved app log directory.
pub fn init_log_dir(dir: PathBuf) {
    let _ = LOG_DIR.set(dir);
}

use crate::db;
use crate::db::queries;
use crate::import;
use crate::torrent::manager::{fastresume_dir, DownloadManager, DownloadProgress};
use crate::torrent::TorrentIndex;

use super::DbState;

/// Metadata describing a single eXo collection.
/// All path conventions for a collection are captured here so that game
/// launch / install / uninstall code does not need to hard-code any
/// collection-specific strings.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionDef {
    /// Internal collection ID (e.g. "eXoDOS", "eXoDOS_GLP").
    pub id: &'static str,
    /// Human-readable name shown in the UI.
    pub display_name: &'static str,
    /// Bundled metadata XML gz file (e.g. "MS-DOS.xml.gz").
    pub metadata_file: &'static str,
    /// Bundled .torrent filename (e.g. "eXoDOS.torrent").
    pub torrent_file: &'static str,
    /// Optional bundled DOSBox/emulator configs ZIP.
    pub configs_zip: Option<&'static str>,
    /// The folder name the torrent creates inside the data dir (always "eXoDOS").
    /// All four collections (eXoDOS, GLP, PLP, SLP) share the same output folder via
    /// the overlay model - their torrents all have the internal name "eXoDOS" and write
    /// to <data_dir>/eXoDOS/ without any per-collection subdirectory.
    pub inner_folder: &'static str,
    /// Path from <inner_folder> to the individual game directories.
    /// e.g. "eXo/eXoDOS" → games are at <inner_folder>/eXo/eXoDOS/<shortcode>/
    pub game_prefix: &'static str,
    /// Segment in the LaunchBox application_path used to extract the shortcode.
    /// e.g. "!dos" for eXoDOS (path looks like "eXo\eXoDOS\!dos\<shortcode>\…")
    pub shortcode_segment: &'static str,
    /// Language subdirectory inside game_prefix for LP variant games.
    /// None for the base English collection.
    pub lang_dir: Option<&'static str>,
    /// LaunchBox platform name. Names the media subtree in the metadata pack
    /// (`Images/<platform>/`, `Manuals/<platform>/`) and matches the XML's
    /// `<Platform>` value.
    pub platform: &'static str,
    /// Games live under a 4-digit year subdirectory and are keyed by their
    /// title directory instead of an 8-char shortcode:
    /// `<game_prefix>/<year>/<Title (Year)>/` (eXoWin9x layout). All path
    /// derivation keys off this flag, never off the collection id.
    pub year_subdirs: bool,
}

/// Name of the ONE folder inside the data dir that holds every collection.
///
/// eXo ships each pack as its own torrent but expects them merged: their
/// Setup/eXoMerge bats copy `Content\` and `eXo\` of every pack into a single
/// folder, giving one `eXo/` tree with `eXoDOS/`, `eXoWin3x/`, `eXoWin9x/`
/// side by side. Exodium writes the same layout, so an installation made by
/// eXo's own setup can be imported as-is - and nothing is downloaded twice
/// because we looked in a folder eXo never creates.
///
/// Cached rather than read per call: every path derivation needs it, and the
/// three places that can change it (fresh setup, import, data-dir change) all
/// set it explicitly.
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

/// Point an install made before the single-root layout at the tree it already
/// has, instead of nesting a second one inside it.
///
/// Until the single root, the main eXoDOS torrent wrote STRAIGHT INTO the data
/// dir, so a legacy install looks like `<data>/eXo/…` + `<data>/Content/…`.
/// `game_root` is `<data>/<root_folder>`, which can never equal `<data>` - so
/// those users would get `<data>/eXoDOS/` as their root and re-download 282 GB
/// next to the games they already own. Seen in testing: 8.7 GB of a second
/// eXoDOS tree inside the first one.
///
/// The repair is a re-labelling, not a move: `root_folder` becomes the `.`
/// sentinel, so `game_root` resolves to the data dir itself - exactly where the
/// games already are. The data dir stays put deliberately; moving it up a level
/// would work for the games but strand everything else keyed to it (content
/// packs, the video and gallery caches) and drop Exodium's `content/` into the
/// user's parent folder.
///
/// Keyed on `root_folder` being unset - true only for installs predating the
/// change. Once written, the user's value is trusted.
fn repair_legacy_root(conn: &rusqlite::Connection) {
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

/// Load the root folder name from config into the cache.
///
/// Repairs the pre-single-root layout on the way: every caller reads `data_dir`
/// right after this, so it is the one place where the fix reliably lands
/// before a path is derived from either value.
pub fn load_root_folder(conn: &rusqlite::Connection) {
    repair_legacy_root(conn);
    let name = queries::get_config(conn, "root_folder")
        .ok()
        .flatten()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ROOT_FOLDER.to_string());
    set_root_folder(&name);
}

/// The single directory holding every collection's files.
///
/// Replaces the old per-collection roots (`<data>/eXoWin9x/…`), which were an
/// artefact of librqbit naming a torrent's output folder after the torrent -
/// not a layout eXo or anyone else expects.
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

/// Old per-torrent roots still sitting next to the real one.
///
/// Exodium used to give each pack the folder librqbit names after the torrent
/// (`<data>/eXoWin3x/`, `<data>/eXoWin9x/`), which is not a layout eXo
/// produces. Everything lives in one root now, so an install made before that
/// has to be merged - with the user's consent, since it moves their files.
#[derive(Debug, Clone, Serialize)]
pub struct LayoutMigration {
    /// Folder names still holding games, relative to the data dir.
    pub folders: Vec<String>,
    /// Rough size, so the dialog can say what is about to be moved.
    pub bytes: u64,
    /// Whether to raise the question on startup. False once declined - the
    /// merge itself stays reachable from Settings, which is why the folders
    /// are still reported rather than hidden.
    pub prompt: bool,
}

/// Folders that look like a torrent root of their own (they contain `eXo/`)
/// and are not the configured root.
fn stray_roots(data_dir: &str) -> Vec<PathBuf> {
    let root = game_root(data_dir);
    COLLECTION_MAP
        .iter()
        .map(|c| c.inner_folder)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        // Three places a per-torrent root can sit. Two are inside the data
        // dir: beside the game root, and inside it once the data dir IS the
        // root. The third is BESIDE the data dir - only reachable when the
        // data dir is the root, because that is the legacy shape whose folders
        // were created while the data dir was one level up. Scanning a normal
        // install's parent would mean sweeping the user's home directory.
        .flat_map(|name| {
            let mut candidates = vec![PathBuf::from(data_dir).join(name), root.join(name)];
            if root == Path::new(data_dir) {
                if let Some(parent) = Path::new(data_dir).parent() {
                    candidates.push(parent.join(name));
                }
            }
            candidates
        })
        // `eXo/` inside tells a pack root apart from the root's own
        // `eXo/eXoWin3x/`, which is where those files belong.
        .filter(|p| *p != root && p.join("eXo").is_dir())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Is there anything to merge into the single root?
#[tauri::command]
pub async fn pending_layout_migration(
    db_state: State<'_, DbState>,
) -> Result<Option<LayoutMigration>, String> {
    let (data_dir, asked) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        load_root_folder(&conn);
        (
            queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?,
            queries::get_config(&conn, "layout_migration").map_err(|e| e.to_string())?,
        )
    };
    let Some(data_dir) = data_dir else { return Ok(None) };
    let strays = stray_roots(&data_dir);
    if strays.is_empty() {
        return Ok(None);
    }
    if asked.as_deref() == Some("skip") {
        // Worth a log line every start: those folders are not read, so their
        // games look uninstalled and a re-download lands a second copy in the
        // real root while the first keeps occupying the disk.
        log::warn!(
            "Layout: {} folder(s) next to the game root are being ignored ({}). \
             Merge them from Settings to make their games visible again.",
            strays.len(),
            strays
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Apparent size only - walking 282 GB of files to add up bytes would
    // stall startup, and the dialog only needs an order of magnitude.
    let bytes = strays.iter().map(|p| dir_size_shallow(p)).sum();
    Ok(Some(LayoutMigration {
        folders: strays
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect(),
        bytes,
        prompt: asked.as_deref() != Some("skip"),
    }))
}

/// Sum of the game archives one level below `<root>/eXo/<pack>/`.
fn dir_size_shallow(root: &Path) -> u64 {
    fn walk(dir: &Path, depth: usize, acc: &mut u64) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.filter_map(|e| e.ok()) {
            match e.metadata() {
                Ok(m) if m.is_file() => *acc += m.len(),
                Ok(m) if m.is_dir() => walk(&e.path(), depth + 1, acc),
                _ => {}
            }
        }
    }
    let mut total = 0;
    walk(root, 0, &mut total);
    total
}

/// Remember that the user does not want the folders merged.
#[tauri::command]
pub async fn skip_layout_migration(db_state: State<'_, DbState>) -> Result<(), String> {
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
    queries::set_config(&conn, "layout_migration", "skip").map_err(|e| e.to_string())
}

/// Move every stray root's contents into the single root.
///
/// Renames only: the folders sit on one filesystem, so this is instant and
/// nothing is copied. Anything already present at the destination is left
/// alone rather than overwritten - a half-finished manual merge must not lose
/// data. Torrent state is re-derived afterwards by re-initialising the
/// managers; librqbit re-checks the files in place rather than downloading
/// them again.
#[tauri::command]
pub async fn migrate_layout(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<MergeTally, String> {
    // Stop the torrent engine before touching a single file. librqbit's
    // session opens (and therefore CREATES) every selected file of a torrent,
    // so a live session pointed at the old folder re-materializes the whole
    // tree the instant the merge empties it - measured: 673 files back one
    // second after the move. Dropping the managers releases the last Arc to
    // the session; the frontend calls init_download_manager afterwards, which
    // rebuilds it against the single root.
    torrent_state.0.write().await.clear();

    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        load_root_folder(&conn);
        queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("No data directory configured")?
    };
    let root = game_root(&data_dir);
    let mut tally = MergeTally::default();
    let strays = stray_roots(&data_dir);
    log::info!(
        "Layout migration: merging {:?} into {}",
        strays.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        root.display()
    );
    for stray in strays {
        tally.add(merge_tree(&stray, &root)?);
        // Everything is either moved or a deleted duplicate by now, so what
        // remains is empty directories - clear them out so the folder is gone
        // and the prompt has nothing left to find. A folder that still holds
        // a file (something we did not put there) is kept, deliberately.
        remove_empty_tree(&stray);
        if stray.exists() {
            log::warn!(
                "Layout migration: {} kept - it still holds files Exodium did not place there",
                stray.display()
            );
        }
    }
    // Every fastresume bitfield now describes a tree that no longer exists at
    // the path it was recorded for. Leaving them would have librqbit skip its
    // check and read the freshly moved-in files as missing pieces.
    if let Ok(config_dir) = app.path().app_data_dir() {
        let persistence = fastresume_dir(&config_dir);
        if let Ok(entries) = std::fs::read_dir(&persistence) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "bitv") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, "layout_migration", "done").map_err(|e| e.to_string())?;
    }
    log::info!(
        "Layout migration: {} moved, {} duplicates removed, {} left alone",
        tally.moved,
        tally.deduped,
        tally.skipped
    );
    Ok(tally)
}

/// Move `src`'s children into `dst`, descending where both sides have the
/// same directory so an existing `eXo/` or `Content/` is merged rather than
/// replaced.
///
/// When the same file exists on both sides, **the larger one wins** and the
/// other is deleted. That is not arbitrary: the loser is almost always a
/// zero-byte torrent placeholder (librqbit allocates one per file of the
/// collection) or a half-finished download, and the winner the real archive.
/// Leaving both behind was the first attempt - it meant the old folders never
/// disappeared, so the migration prompt came back at every start with nothing
/// left to do.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct MergeTally {
    /// Files that only existed on the old side and were renamed across.
    pub moved: usize,
    /// Duplicates resolved - the smaller copy (usually a zero-byte torrent
    /// placeholder) was deleted.
    pub deduped: usize,
    /// Entries neither side could resolve: a directory facing a file, or the
    /// reverse. Counted rather than ignored, because "nothing happened and
    /// nothing was reported" is exactly how the first version of this hid a
    /// folder that never emptied.
    pub skipped: usize,
}

impl MergeTally {
    fn add(&mut self, other: MergeTally) {
        self.moved += other.moved;
        self.deduped += other.deduped;
        self.skipped += other.skipped;
    }
}

fn merge_tree(src: &Path, dst: &Path) -> Result<MergeTally, String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    let mut tally = MergeTally::default();
    let entries = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for entry in entries.filter_map(|e| e.ok()) {
        let from = entry.path();
        // Not worth carrying across, and counting it would inflate the report
        // the user reads. remove_empty_tree deletes what is left.
        if is_os_metadata(&from) {
            continue;
        }
        let to = dst.join(entry.file_name());
        if !to.exists() {
            std::fs::rename(&from, &to)
                .map_err(|e| format!("moving {} to {}: {e}", from.display(), to.display()))?;
            tally.moved += 1;
            continue;
        }
        if from.is_dir() && to.is_dir() {
            tally.add(merge_tree(&from, &to)?);
            let _ = std::fs::remove_dir(&from);
            continue;
        }
        let src_len = from.metadata().map(|m| m.len()).unwrap_or(0);
        let dst_len = to.metadata().map(|m| m.len()).unwrap_or(0);
        if from.is_file() && to.is_file() && src_len > dst_len {
            std::fs::remove_file(&to).map_err(|e| e.to_string())?;
            std::fs::rename(&from, &to)
                .map_err(|e| format!("replacing {}: {e}", to.display()))?;
            tally.moved += 1;
        } else if from.is_file() {
            std::fs::remove_file(&from)
                .map_err(|e| format!("removing duplicate {}: {e}", from.display()))?;
            tally.deduped += 1;
        } else {
            log::warn!(
                "Layout migration: {} and {} are different kinds of entry - left alone",
                from.display(),
                to.display()
            );
            tally.skipped += 1;
        }
    }
    Ok(tally)
}

/// Delete a directory tree that contains only (empty) directories.
fn remove_empty_tree(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            remove_empty_tree(&p);
        } else if is_os_metadata(&p) {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Files the OS drops into a folder on its own. They are not user data, and
/// keeping them is not free: a merged folder that still holds a `.DS_Store` is
/// not empty, so it survives, `stray_roots` finds it again and the migration
/// prompt returns at every start with nothing left to move. Seen in testing
/// with exactly two `.DS_Store` files against 48 GB moved.
pub(crate) fn is_os_metadata(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // `._x` is the AppleDouble sidecar a Mac writes on non-native filesystems.
    matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini") || name.starts_with("._")
}

/// Look up a collection definition by ID.  Returns None for unknown IDs.
pub fn collection_def(id: &str) -> Option<&'static CollectionDef> {
    COLLECTION_MAP.iter().find(|c| c.id == id)
}

/// The collection to consult for assets `collection` has none of its own.
///
/// Language packs borrow their base collection's art and manuals - their
/// variants hash to the EN title's key. A collection with its own game tree
/// borrows nothing: its games are not in the other pack, so a same-title hit
/// would show a different game's cover. `None` means "no fallback".
pub fn asset_fallback(collection: &str) -> Option<&'static str> {
    let base = collection_base_id(collection);
    (base != collection).then_some(base)
}

/// The base (non-language-pack) collection a source belongs to.
///
/// Language packs share the base collection's game tree and its GameData
/// archives, so "eXoDOS_GLP" resolves to "eXoDOS". A collection with its own
/// game tree resolves to itself. Used wherever a lookup may only cross
/// collection boundaries WITHIN one pack family - shortcodes are unique per
/// family, not globally, so an unqualified match can hit a different game in
/// another pack that happens to share the code.
pub fn collection_base_id(source: &str) -> &'static str {
    let Some(def) = collection_def(source) else {
        return "eXoDOS";
    };
    if def.lang_dir.is_none() {
        return def.id;
    }
    COLLECTION_MAP
        .iter()
        .find(|c| c.lang_dir.is_none() && c.game_prefix == def.game_prefix)
        .map(|c| c.id)
        .unwrap_or("eXoDOS")
}

/// Config value of `network_mode` that keeps the torrent engine shut down.
pub(crate) const OFFLINE_MODE: &str = "offline";

/// True when the user picked "offline" during setup (or later in Settings):
/// no librqbit session is created, nothing is downloaded, nothing is shared.
/// A missing key means "live" so installs from before this setting keep
/// behaving as they did.
pub(crate) fn is_offline(db: &std::sync::Mutex<rusqlite::Connection>) -> bool {
    db.lock()
        .ok()
        .and_then(|conn| queries::get_config(&conn, "network_mode").ok().flatten())
        .as_deref()
        == Some(OFFLINE_MODE)
}

/// Sharing is opt-in: only an explicit "1" counts. Split out from
/// `apply_transfer_preferences` so the rule can be tested without a session.
pub(crate) fn seeding_enabled(db: &std::sync::Mutex<rusqlite::Connection>) -> bool {
    db.lock()
        .ok()
        .and_then(|conn| queries::get_config(&conn, "seeding_enabled").ok().flatten())
        .as_deref()
        == Some("1")
}

/// The user's transfer caps in KB/s as `(upload, download)`, `None` meaning
/// unlimited. Anything unparseable or zero reads as unlimited: a stored "0"
/// would otherwise mean "throttle to nothing", which is not a setting the UI
/// can offer back out again.
pub(crate) fn rate_limits(db: &std::sync::Mutex<rusqlite::Connection>) -> (Option<u32>, Option<u32>) {
    let read = |key: &str| -> Option<u32> {
        let conn = db.lock().ok()?;
        let raw = queries::get_config(&conn, key).ok().flatten()?;
        raw.parse::<u32>().ok().filter(|v| *v > 0)
    };
    (read("rate_limit_up_kbps"), read("rate_limit_down_kbps"))
}

/// Extract the bundled emulator configs for every enabled collection. This is
/// all `init_download_manager` does in offline mode, so it is the whole offline
/// path in one testable place.
fn extract_all_bundled_configs(
    collections: &[&str],
    metadata_dir: Option<&PathBuf>,
    data_path: &Path,
) {
    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        extract_bundled_configs(col, metadata_dir, &game_root(&data_path.to_string_lossy()));
    }
}

/// Serialisable summary returned by the `get_available_collections` command.
#[derive(Debug, Serialize)]
pub struct CollectionInfo {
    pub id: String,
    pub display_name: String,
    pub torrent_file: String,
    /// Catalogue rows in this collection (variant rows, not merged groups) -
    /// rendered on the collection shelf cards.
    pub game_count: i64,
}

/// Return the list of all known collections (for the frontend to render
/// collection pickers / labels without hardcoding IDs).
#[tauri::command]
pub async fn get_available_collections(
    db_state: State<'_, DbState>,
) -> Result<Vec<CollectionInfo>, String> {
    // NULL torrent_source rows (a handful of unmatched variants and the pack
    // sentinel) are deliberately NOT attributed to eXoDOS: the grid's
    // collection filter can't reach them, so counting them would make the
    // shelf number disagree with the grid it opens.
    let counts: std::collections::HashMap<String, i64> = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT torrent_source, COUNT(*) FROM games GROUP BY torrent_source")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok())
            .filter_map(|(k, v)| k.map(|k| (k, v)))
            .collect()
    };
    Ok(COLLECTION_MAP
        .iter()
        .map(|c| CollectionInfo {
            id: c.id.to_string(),
            display_name: c.display_name.to_string(),
            torrent_file: c.torrent_file.to_string(),
            game_count: counts.get(c.id).copied().unwrap_or(0),
        })
        .collect())
}

/// Return the directory where Exodium writes its log file. Served from the
/// `LOG_DIR` cache populated in `lib.rs::run` to avoid re-resolving via
/// `app.path().app_log_dir()` at command time - that round-trip was observed
/// failing silently on shipped Windows builds while the setup-time call
/// succeeded.
#[tauri::command]
pub async fn get_log_dir(app: AppHandle) -> Result<String, String> {
    if let Some(dir) = LOG_DIR.get() {
        return Ok(dir.to_string_lossy().into_owned());
    }
    // Fallback: try the live resolver. If the cache isn't populated (init
    // bug or test harness), this still works in dev mode.
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("app_log_dir failed: {e}"))?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Open the log folder in the user's file explorer. Bypasses the frontend's
/// `getLogDir() + openPath()` two-step which was observed leaving the UI
/// "Resolving…" forever in shipped Windows builds - by doing both lookup
/// and open server-side, the UI just calls one command and either succeeds
/// or sees the error.
#[tauri::command]
pub async fn open_log_folder(app: AppHandle) -> Result<(), String> {
    let dir = match LOG_DIR.get() {
        Some(d) => d.clone(),
        None => app
            .path()
            .app_log_dir()
            .map_err(|e| format!("app_log_dir failed: {e}"))?,
    };
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create log folder: {e}"))?;
    }
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(&dir).spawn();
    result
        .map(|_| ())
        .map_err(|e| format!("Failed to open log folder {}: {e}", dir.display()))
}

/// Managed state for the download system - supports multiple torrents.
pub struct TorrentState(pub RwLock<std::collections::HashMap<String, Arc<DownloadManager>>>);

#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub phase: String,
    pub metadata_progress: Option<DownloadProgress>,
    pub dosbox_metadata_progress: Option<DownloadProgress>,
    pub games_imported: usize,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentInfo {
    pub name: String,
    pub file_count: usize,
    pub total_size: u64,
    pub metadata_size: u64,
}

/// Decide if a torrent_root looks like a fresh install - i.e. either missing
/// or contains nothing that librqbit needs to validate against. We use this
/// to gate the empty-bitfield pre-seed: if the user pointed Exodium at a
/// directory full of existing game data, we must let librqbit do its real
/// validation pass rather than tell it "everything is missing" (which would
/// trigger a re-download of complete files).
///
/// Dotfiles (e.g. `.eXoDOS_configs_extracted` markers we drop into the
/// torrent_root after extracting bundled DOSBox configs) are NOT torrent
/// content and must be ignored - otherwise the second app launch would see
/// our own markers and refuse to seed, defeating the optimisation for users
/// who haven't actually downloaded any games yet.
fn torrent_root_looks_empty(torrent_root: &Path) -> bool {
    let iter = match std::fs::read_dir(torrent_root) {
        Err(_) => return true, // doesn't exist or unreadable → safe to seed
        Ok(it) => it,
    };
    for entry in iter.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if !s.starts_with('.') {
            return false;
        }
    }
    true
}

/// Pre-seed `<persistence_dir>/<info_hash>.bitv` files with `ceil(piece_count/8)`
/// zero bytes for each enabled collection where (a) the .bitv doesn't already
/// exist and (b) the data directory looks fresh.
///
/// librqbit's `JsonSessionPersistenceStore::BitVFactory::load(info_hash)` reads
/// this file directly by info_hash - it does not require an entry in
/// `session.json`. With a present, all-zero bitfield, `validate_fastresume`
/// accepts "I have 0 pieces" without iterating any pieces, and the slow
/// `initial_check` pass is skipped entirely. Net effect: first download on a
/// fresh install starts in seconds instead of 5–10 minutes on Windows.
fn seed_fastresume_bitvs(
    persistence_dir: &Path,
    collections: &[&str],
    data_path: &Path,
) {
    if let Err(e) = std::fs::create_dir_all(persistence_dir) {
        log::warn!("fastresume: could not create {}: {}", persistence_dir.display(), e);
        return;
    }
    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        let torrent_path = match bundled_torrent_path(col.torrent_file) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Parse torrent for info_hash + piece_count. We use lava_torrent
        // directly here rather than TorrentIndex because TorrentIndex doesn't
        // expose piece_count.
        let torrent = match lava_torrent::torrent::v1::Torrent::read_from_file(&torrent_path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("fastresume: failed to parse {}: {}", col.torrent_file, e);
                continue;
            }
        };
        let info_hash = torrent.info_hash();
        let bitv_path = persistence_dir.join(format!("{}.bitv", info_hash));

        if bitv_path.exists() {
            log::debug!("fastresume: existing bitv for {} ({})", col.id, info_hash);
            continue;
        }

        // One root for every collection. Only seed if it looks empty -
        // otherwise librqbit must verify what's actually on disk.
        let torrent_root = game_root(&data_path.to_string_lossy());
        if !torrent_root_looks_empty(&torrent_root) {
            log::info!(
                "fastresume: skipping seed for {} - torrent_root {} non-empty, real validation needed",
                col.id,
                torrent_root.display()
            );
            continue;
        }

        let piece_count = torrent.pieces.len();
        let byte_count = piece_count.div_ceil(8);
        match std::fs::write(&bitv_path, vec![0u8; byte_count]) {
            Ok(_) => log::info!(
                "fastresume: seeded empty bitv for {} ({} pieces, {} bytes)",
                col.id, piece_count, byte_count
            ),
            Err(e) => log::warn!(
                "fastresume: failed to write {}: {}",
                bitv_path.display(), e
            ),
        }
    }
}

/// Initialize download managers for all available torrents.
/// Returns true if initialized, false if no config found.
#[tauri::command]
pub async fn init_download_manager(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<bool, String> {
    // Clear existing managers
    torrent_state.0.write().await.clear();

    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        // Every path below (and in launch/uninstall/scan) is derived from the
        // root, so it has to be in the cache before anything asks.
        load_root_folder(&conn);
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    };

    let data_dir = match data_dir {
        Some(d) => d,
        None => return Ok(false),
    };

    let data_path = PathBuf::from(&data_dir);

    // Get selected collections from config
    let collections_str = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "collections")
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "eXoDOS".to_string())
    };
    let collections: Vec<&str> = collections_str.split(',').collect();

    let metadata_dir = bundled_metadata_dir().ok();

    // Startup is the natural place to bound the gallery cache: nothing else
    // deletes from it, and doing it here keeps it off the panel-open path.
    remove_legacy_cache_dirs(&data_dir);
    // Earlier versions wrote per-launch DOSBox fragments straight into the
    // user's game folder; they are regenerated on demand, so sweep them up.
    crate::commands::games::sweep_legacy_launch_confs(&data_dir);
    crate::commands::games::prune_launch_confs(&app);
    prune_gallery_cache(&gallery_cache_dir(&data_dir), GALLERY_CACHE_MAX_BYTES);
    crate::commands::media::prune_video_cache(&data_dir);

    // Offline mode: no session, no torrents, no swarm traffic - the app is a
    // launcher for whatever is already on disk. Bundled emulator configs are
    // still extracted; they are shipped with Exodium, not with the torrent.
    // Managers were cleared above, so a live -> offline switch at runtime
    // drops the last Arc to the session and shuts librqbit down.
    if is_offline(&db_state.0) {
        extract_all_bundled_configs(&collections, metadata_dir.as_ref(), &data_path);
        log::info!("Offline mode: torrent engine not started (data_dir: {})", data_dir);
        return Ok(false);
    }

    // All collections share one librqbit session and the same data directory.
    // Session state (.librqbit/) is stored in the app config dir, not the game data dir.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let persistence_dir = fastresume_dir(&config_dir);

    // Plant empty bitfields for fresh-install torrents BEFORE creating the
    // session so librqbit's `BitVFactory::load(info_hash)` finds them and
    // skips the initial_check pass. Existing data triggers real validation.
    seed_fastresume_bitvs(&persistence_dir, &collections, &data_path);

    let session = DownloadManager::create_session(&config_dir, &persistence_dir)
        .await
        .map_err(|e| e.to_string())?;
    evict_mismatched_session_torrents(&session, &persistence_dir, &data_path).await;
    apply_transfer_preferences(&session, &db_state);

    // Build all managers and do slow work (infohash, config extraction) WITHOUT holding
    // the torrent_state write lock - archive.extract() on 7 000+ files blocks for seconds.
    let mut new_managers: Vec<(String, Arc<DownloadManager>)> = Vec::new();

    for col in COLLECTION_MAP {
        if !collections.contains(&col.id) {
            continue;
        }
        if let Ok(torrent_path) = bundled_torrent_path(col.torrent_file) {
            match DownloadManager::new_with_session(Arc::clone(&session), &torrent_path, &data_path, &persistence_dir) {
                Ok(mgr) => {
                    // Record which torrent this install was built against.
                    // Nothing reads it today - the catalogue update check was
                    // removed until there is a migration that keeps a user's
                    // library (issue #18) - but it has to be captured NOW: once
                    // a newer bundled torrent ships, the old baseline is gone
                    // and the comparison can never be made retroactively.
                    // Write-if-absent for the same reason.
                    match TorrentIndex::infohash(&torrent_path) {
                        Ok(hash) => {
                            match db_state.0.lock() {
                                Ok(conn) => {
                                    let key = format!("{}_infohash", col.id);
                                    let existing = queries::get_config(&conn, &key).ok().flatten();
                                    if existing.is_none() {
                                        if let Err(e) = queries::set_config(&conn, &key, &hash) {
                                            log::warn!("Failed to save infohash for {}: {}", col.id, e);
                                        }
                                    }
                                }
                                Err(e) => log::warn!("Failed to lock DB for infohash write ({}): {}", col.id, e),
                            }
                        }
                        Err(e) => log::warn!("Failed to compute infohash for {}: {}", col.id, e),
                    }

                    extract_bundled_configs(col, metadata_dir.as_ref(), &mgr.torrent_root());
                    log::info!("Initialized download manager: {}", col.id);
                    new_managers.push((col.id.to_string(), Arc::new(mgr)));
                }
                Err(e) => {
                    log::warn!("Failed to init {} download manager: {}", col.id, e);
                }
            }
        }
    }

    // All enabled torrents overlay into the same root - placeholder cleanup
    // in any one manager must keep the union of every torrent's file list.
    set_union_cleanup_keep_paths(&new_managers);

    // Adopt torrents the session auto-resumed from persistence, so downloads
    // interrupted by an app restart report progress and finish extraction.
    for (id, mgr) in &new_managers {
        if mgr.hydrate_from_session().await {
            log::info!("{}: adopted persisted torrent from session", id);
        }
    }

    // Resume support-file extraction (MT-32 ROMs / ECE build) if util.zip
    // was in flight when the app last quit - its watcher died with the app.
    if let Some((_, mgr)) = new_managers.iter().find(|(id, _)| id == "eXoDOS") {
        crate::commands::games::rearm_support_extraction(mgr).await;
    }
    // Same for the Win9x support payload (OS parent VHDs + emulators).
    if let Some((_, mgr)) = new_managers.iter().find(|(id, _)| id == "eXoWin9x") {
        crate::commands::win9x::rearm_win9x_support(mgr).await;
    }

    // Acquire write lock only for the insert - no blocking work inside.
    let count = new_managers.len();
    {
        let mut managers = torrent_state.0.write().await;
        for (id, mgr) in new_managers {
            managers.insert(id, mgr);
        }
    }

    log::info!("Download managers initialized: {} (data_dir: {})", count, data_dir);
    Ok(count > 0)
}

/// Extract a collection's bundled emulator-config ZIP into the torrent root,
/// once (a lock file marks success). Called for every enabled collection in
/// BOTH network modes - an offline install still needs the DOSBox configs
/// that would otherwise arrive with the torrent.
fn extract_bundled_configs(col: &CollectionDef, metadata_dir: Option<&PathBuf>, torrent_root: &Path) {
    let (Some(cfg_zip), Some(md)) = (col.configs_zip, metadata_dir) else {
        return;
    };
    let cfg_path = md.join(cfg_zip);
    if !cfg_path.exists() {
        return;
    }
    let lock = torrent_root.join(format!(".{}_configs_extracted", col.id));
    if lock.exists() {
        return;
    }
    log::info!("Extracting {} configs to {}", col.id, torrent_root.display());
    // Write the lock ONLY on success - latching a failed extract (disk full,
    // permissions) left the configs permanently missing with no retry.
    let extracted = std::fs::File::open(&cfg_path)
        .map_err(|e| e.to_string())
        .and_then(|f| zip::ZipArchive::new(f).map_err(|e| e.to_string()))
        .and_then(|mut a| a.extract(torrent_root).map_err(|e| e.to_string()));
    match extracted {
        Ok(()) => {
            if let Err(e) = std::fs::write(&lock, "") {
                log::warn!("Could not write configs lock for {}: {}", col.id, e);
            }
        }
        Err(e) => log::error!(
            "Failed to extract {} configs (will retry next startup): {}",
            col.id, e
        ),
    }
}

/// Apply the persisted seeding preference to a freshly created session.
/// Push the stored transfer preferences into a freshly created session.
///
/// Sharing is OPT-IN: only an explicit "1" lifts the upload cap, anything else
/// (including an unset key) caps upload at 1 KB/s. Uploading copyrighted
/// material is a legal risk in some jurisdictions, so it must never start
/// without consent. The user's own caps ride along, because a new session
/// starts unlimited in both directions and would otherwise ignore them until
/// the next time Settings was touched.
fn apply_transfer_preferences(session: &Arc<librqbit::Session>, db_state: &State<'_, DbState>) {
    let seeding = seeding_enabled(&db_state.0);
    let (up_kbps, down_kbps) = rate_limits(&db_state.0);
    crate::torrent::manager::apply_session_limits(session, seeding, up_kbps, down_kbps);
}

/// Drop session torrents whose persisted output folder is not the current game
/// root. Adopting them via hydrate_from_session would report progress against
/// files librqbit writes to the OLD location - extraction probes the new one
/// and loops on "100% but ZIP missing" forever.
///
/// The test is EXACT, not "somewhere under the data dir": every torrent is
/// added with `output_folder = game_root`, so any other value is stale by
/// definition. The looser check let the pre-single-root layout survive - a
/// torrent persisted with `<data>/eXoWin9x` sat inside the data dir, passed,
/// and librqbit re-created its whole file tree there seconds after the layout
/// migration had merged it away.
async fn evict_mismatched_session_torrents(
    session: &Arc<librqbit::Session>,
    persistence_dir: &Path,
    data_path: &Path,
) {
    let session_json = persistence_dir.join("session.json");
    let content = match std::fs::read_to_string(&session_json) {
        Ok(c) => c,
        Err(_) => return, // no persisted session yet
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Could not parse {}: {}", session_json.display(), e);
            return;
        }
    };
    let Some(torrents) = parsed.get("torrents").and_then(|t| t.as_object()) else {
        return;
    };

    // Normalize for comparison: strip Windows \\?\ long-path prefix, unify
    // slashes (output folders are written with to_long_path on Windows),
    // trim trailing separators, and case-fold on case-insensitive platforms
    // (C:\Games vs c:\games must not read as a mismatch).
    let normalize = |p: &str| {
        let s = p
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        if cfg!(any(windows, target_os = "macos")) {
            s.to_ascii_lowercase()
        } else {
            s
        }
    };
    let root_norm = normalize(&game_root(&data_path.to_string_lossy()).to_string_lossy());

    for entry in torrents.values() {
        let (Some(hash), Some(folder)) = (
            entry.get("info_hash").and_then(|h| h.as_str()),
            entry.get("output_folder").and_then(|f| f.as_str()),
        ) else {
            continue;
        };
        if normalize(folder) == root_norm {
            continue;
        }
        let stale_id = session.with_torrents(|iter| {
            for (tid, t) in iter {
                if t.info_hash().as_string().eq_ignore_ascii_case(hash) {
                    return Some(tid);
                }
            }
            None
        });
        if let Some(tid) = stale_id {
            log::warn!(
                "Evicting session torrent {} - persisted output folder {} is not the game root {}",
                hash, folder, root_norm
            );
            if let Err(e) = session
                .delete(librqbit::api::TorrentIdOrHash::Id(tid), false)
                .await
            {
                log::warn!("Failed to evict stale session torrent {}: {}", hash, e);
            }
        }
    }
}

/// Give every manager the union of all managers' torrent file lists as its
/// placeholder-cleanup keep-list. See DownloadManager::cleanup_keep_paths.
fn set_union_cleanup_keep_paths(managers: &[(String, Arc<DownloadManager>)]) {
    let union: Arc<Vec<String>> = Arc::new(
        managers
            .iter()
            .flat_map(|(_, m)| m.index().files.iter().map(|f| f.path.clone()))
            .collect(),
    );
    for (_, mgr) in managers {
        mgr.set_cleanup_keep_paths(Arc::clone(&union));
    }
}

/// Reset all data: clear DB, remove config. Returns to setup state.
#[tauri::command]
pub async fn factory_reset(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    delete_game_data: bool,
) -> Result<(), String> {
    log::info!("factory_reset called (delete_game_data={})", delete_game_data);
    // Read data_dir before clearing config (Mutex must not be held across await)
    let data_dir = if delete_game_data {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    } else {
        None
    };

    // Drop all download managers. Use a timeout so a stuck reader doesn't hang forever.
    let session_mgr = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        torrent_state.0.write(),
    ).await {
        Ok(mut managers) => {
            // Keep one Arc to reach the shared session after clearing the map.
            let any = managers.values().next().cloned();
            managers.clear();
            any
        }
        Err(_) => {
            log::error!("factory_reset: timed out waiting for torrent write lock");
            return Err("Could not stop downloads in time. Cancel any active downloads and try again.".to_string());
        }
    };

    // Quiesce librqbit BEFORE deleting files: spawned tasks hold Arcs that
    // keep the session's writers alive past the map clear, and a live writer
    // can re-create files after the wipe - leaving a piece ledger that
    // claims data which no longer exists ("100% but ZIP missing" on the
    // next setup).
    if let Some(mgr) = session_mgr {
        mgr.shutdown_session().await;
    }

    // Reset user state without touching the game catalog.
    // Games are catalog data (from the bundled DB) - clearing them would leave
    // the library empty until next restart. Only reset per-user flags and config.
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "UPDATE games SET in_library = 0, installed = 0, favorited = 0, last_played = NULL;
             DELETE FROM game_config;
             DELETE FROM downloads;
             DELETE FROM images;
             -- Curated playlists are catalog data like the games rows above:
             -- deleting them here would leave the Playlists dropdown empty
             -- until the next launch re-runs the catalog refresh. Only user
             -- playlists are user state (their memberships cascade).
             DELETE FROM playlists WHERE kind = 'user';
             DELETE FROM config;",
        )
        .map_err(|e| e.to_string())?;
    }

    // Optionally delete the game folder + content packs + stale downloads.
    if let Some(dir) = data_dir {
        if !dir.is_empty() {
            let base = std::path::Path::new(&dir);
            let root = game_root(&dir);
            // Normally the root is a folder INSIDE the data dir, so it can go
            // whole. On a legacy install the two are the same directory - the
            // one the user picked - and removing it would take the folder
            // itself with it. Delete the two trees eXo puts there instead.
            let targets: Vec<PathBuf> = if root == base {
                vec![base.join("eXo"), base.join("Content")]
            } else {
                vec![root]
            };
            for target in targets {
                if !target.exists() {
                    continue;
                }
                log::info!("Deleting game data: {}", target.display());
                if let Err(e) = std::fs::remove_dir_all(&target) {
                    log::error!("Failed to delete game data folder: {}", e);
                    return Err(format!("Failed to delete game data: {}", e));
                }
            }
            // Also remove downloaded content packs and staging artifacts.
            let content_path = base.join("content");
            if content_path.exists() {
                log::info!("Deleting content packs: {}", content_path.display());
                let _ = std::fs::remove_dir_all(&content_path);
            }
            let downloads_path = base.join(".content-downloads");
            if downloads_path.exists() {
                let _ = std::fs::remove_dir_all(&downloads_path);
            }
            // Legacy residue: pre-0.8.4 setup sessions persisted fastresume
            // in the DATA dir (session split-brain bug). Clean it up here so
            // a later setup against the same dir can't load a stale ledger.
            let legacy_fastresume = base.join("librqbit-fastresume");
            if legacy_fastresume.exists() {
                let _ = std::fs::remove_dir_all(&legacy_fastresume);
            }
        }
    }

    // Fastresume cache: clear it ONLY when the data was deleted too. The
    // ledger describes the torrent bytes on disk - after a delete it's stale
    // and would mislead librqbit, but in keep-data mode it's still accurate,
    // and clearing it forced the next download into a full multi-minute
    // revalidation of ~14k files (the exact hang the seeding avoids).
    if delete_game_data {
        match app.path().app_data_dir() {
            Ok(config_dir) => {
                let persistence = fastresume_dir(&config_dir);
                if persistence.exists() {
                    log::info!("Clearing fastresume cache: {}", persistence.display());
                    if let Err(e) = std::fs::remove_dir_all(&persistence) {
                        log::warn!("Failed to clear fastresume cache: {}", e);
                    }
                }
            }
            Err(e) => log::warn!(
                "factory_reset: could not resolve app_data_dir to clear fastresume cache: {}", e
            ),
        }
    }

    log::info!("Factory reset completed (delete_game_data={})", delete_game_data);
    Ok(())
}

/// Convert a PathBuf to a forward-slash string. Tauri's convertFileSrc on
/// the frontend expects consistent separators when we later join `${dir}/${file}`;
/// mixed Windows backslash + frontend forward slash produces broken asset URLs.
pub(crate) fn path_to_fwd_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Get the thumbnail directory path.
/// Checks: dev project dir → data_dir/thumbnails → exe dir/thumbnails
#[tauri::command]
pub async fn get_thumbnail_dir(
    db_state: State<'_, DbState>,
    collection: String,
) -> Result<String, String> {
    // Dev: project directory
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("thumbnails").join(&collection))
        .unwrap_or_default();
    if dev_path.exists() {
        return Ok(path_to_fwd_slash(&dev_path));
    }

    // Production: data_dir/thumbnails/<collection>
    if let Ok(conn) = db_state.0.lock() {
        if let Ok(Some(data_dir)) = queries::get_config(&conn, "data_dir") {
            let prod_path = PathBuf::from(&data_dir).join("thumbnails").join(&collection);
            if prod_path.exists() {
                return Ok(path_to_fwd_slash(&prod_path));
            }
        }
    }

    Err("Thumbnail directory not found".to_string())
}

/// Get the Tier 0 preview directory for a collection.
/// Checks multiple platform-specific layouts because Tauri's bundle.resources
/// placement varies: macOS uses Contents/Resources/, Linux deb uses
/// /usr/lib/<pkg>/, AppImage uses <mount>/usr/lib/<pkg>/, Windows flat-installs
/// into the install directory.
#[tauri::command]
pub async fn get_preview_dir(collection: String) -> Result<String, String> {
    // Dev mode: direct from repo tree.
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("previews")
        .join(&collection);
    if dev_path.exists() {
        return Ok(path_to_fwd_slash(&dev_path));
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. Tauri's reported resource_dir (canonical)
    if let Some(res_dir) = RESOURCE_DIR.get() {
        candidates.push(res_dir.join("previews").join(&collection));
    }

    // 2. Next to the executable (Windows flat install, macOS Contents/MacOS)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("previews").join(&collection));
            candidates.push(exe_dir.join("resources").join("previews").join(&collection));
            // Linux /usr layout: /usr/bin/exodium → /usr/lib/exodium/previews/
            if let Some(usr_dir) = exe_dir.parent() {
                candidates.push(
                    usr_dir
                        .join("lib")
                        .join("exodium")
                        .join("previews")
                        .join(&collection),
                );
                candidates.push(
                    usr_dir
                        .join("share")
                        .join("exodium")
                        .join("previews")
                        .join(&collection),
                );
            }
        }
    }

    for candidate in &candidates {
        if candidate.exists() {
            log::info!("get_preview_dir: found at {}", candidate.display());
            return Ok(path_to_fwd_slash(candidate));
        }
    }

    // Only eXoDOS ships a preview pack; the language packs share it. That is
    // by design, not a missing file, so it must not be a warning - three of
    // them fired at every startup and buried the real ones.
    if let Some(base) = asset_fallback(&collection) {
        log::debug!("get_preview_dir({}): no dedicated pack, falling back to {}", collection, base);
        return Box::pin(get_preview_dir(base.to_string())).await;
    }

    let checked: Vec<String> = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    log::warn!(
        "get_preview_dir({}): not found. Checked: {}",
        collection,
        checked.join(", ")
    );
    Err(format!(
        "Preview directory not found. Checked: {}",
        checked.join(", ")
    ))
}

/// Get the Tier 1 poster content-pack directory for a collection.
/// Returns <data_dir>/content/posters/<collection> if it exists.
#[tauri::command]
pub async fn get_poster_dir(
    db_state: State<'_, DbState>,
    collection: String,
) -> Result<String, String> {
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;
    let data_dir = queries::get_config(&conn, "data_dir")
        .map_err(|e| e.to_string())?
        .ok_or("Data directory not configured")?;
    let base = PathBuf::from(&data_dir).join("content").join("posters");
    // Check collection-specific dir first, fall back to eXoDOS.
    // All poster thumbnails live in the eXoDOS pack; LP collections share them.
    let poster_path = base.join(&collection);
    if poster_path.exists() {
        return Ok(path_to_fwd_slash(&poster_path));
    }
    if let Some(fallback) = asset_fallback(&collection).map(|c| base.join(c)) {
        if fallback.exists() {
            return Ok(path_to_fwd_slash(&fallback));
        }
    }
    Err("Poster directory not found".to_string())
}

/// Metadata payload for a single game.
///
/// Manuals are deferred to v2 (they live inside individual game ZIPs, not
/// XODOSMetadata.zip) so `manual_path` / `manual_kind` are always None today -
/// kept in the struct to avoid a future breaking frontend change.
#[derive(Debug, Serialize)]
pub struct GameMetadata {
    pub manual_path: Option<String>,
    pub manual_kind: Option<String>,
    pub images: Vec<String>,
    /// Small cached copies of `images`, same order and length. The gallery
    /// strip renders these; the lightbox keeps using `images`. Entries fall
    /// back to the full-size path when a thumbnail can't be produced.
    pub thumbnails: Vec<String>,
}

/// Long edge of a cached gallery thumbnail. The strip draws them at 64x48 CSS
/// px, so 160 covers 2x displays with room to spare.
const THUMB_MAX_EDGE: u32 = 160;

/// Where cached gallery thumbnails live. Inside the content dir so a factory
/// reset that clears content also clears the cache.
///
/// NOT a dotted directory: Tauri's asset-protocol scope glob does not match
/// hidden path components, so `.thumbcache` was silently denied for every
/// image (202 denials in one session) while `content/posters` worked.
fn gallery_cache_dir(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("content").join("thumbcache")
}

/// Remove caches from before the rename - they can never be served.
fn remove_legacy_cache_dirs(data_dir: &str) {
    for legacy in [".thumbcache", ".videocache"] {
        let dir = PathBuf::from(data_dir).join("content").join(legacy);
        if dir.is_dir() {
            log::info!("Removing unusable legacy cache dir {}", dir.display());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Cache filename for a source image: content-addressed by path + size +
/// mtime, so a re-downloaded or updated metadata pack misses the cache
/// instead of serving a stale thumbnail.
fn gallery_cache_name(source: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let meta = std::fs::metadata(source).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(source.to_string_lossy().as_bytes());
    hasher.update(meta.len().to_le_bytes());
    hasher.update(mtime.to_le_bytes());
    let hex = format!("{:x}", hasher.finalize());
    Some(format!("{}.jpg", &hex[..24]))
}

/// Upper bound for the gallery cache. At ~5 KB per thumbnail this is far more
/// than any real browsing session produces (opening all 7,600 games would
/// reach ~180 MB), so pruning is a backstop, not a routine event.
const GALLERY_CACHE_MAX_BYTES: u64 = 250 * 1024 * 1024;

/// Counter for temp filenames. Two threads (or two overlapping panel opens)
/// thumbnailing the same source would otherwise write the same `.part` file.
static GALLERY_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Return a small cached JPEG for `source`, generating it on first use.
/// Returns None (caller falls back to the original) on any failure - a broken
/// or exotic image must not cost the user their gallery.
fn gallery_thumbnail(source: &Path, cache_dir: &Path) -> Option<PathBuf> {
    let name = gallery_cache_name(source)?;
    let cached = cache_dir.join(name);
    if cached.is_file() {
        return Some(cached);
    }
    // Decoding is why this exists: some of these are 18 MB PNGs. Doing it here
    // (inside the command's spawn_blocking) costs the first open of a game and
    // saves every later one - and every one on any other machine start.
    let img = image::open(source).ok()?;
    let thumb = img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);
    std::fs::create_dir_all(cache_dir).ok()?;
    // Write to a unique temp name first: a half-written JPEG left by a crash
    // or a full disk would otherwise be cached forever as a valid-looking
    // file, and a shared temp name would let two writers clobber each other.
    let seq = GALLERY_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = cache_dir.join(format!("{}.{}.{}.part", cached.file_stem()?.to_string_lossy(), std::process::id(), seq));
    thumb.to_rgb8().save_with_format(&tmp, image::ImageFormat::Jpeg).ok()?;
    if std::fs::rename(&tmp, &cached).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(cached)
}

/// Thumbnail a game's gallery, falling back to the full-size path per image.
/// Spread over a few threads: the first open of a game can mean decoding a
/// dozen multi-megabyte PNGs, and that is the one time the user waits.
fn generate_gallery_thumbnails(images: &[String], cache_dir: &Path) -> Vec<String> {
    const MAX_THREADS: usize = 4;
    let mut out: Vec<String> = images.to_vec();
    if images.len() < 2 {
        for (full, slot) in images.iter().zip(out.iter_mut()) {
            if let Some(p) = gallery_thumbnail(Path::new(full), cache_dir) {
                *slot = path_to_fwd_slash(&p);
            }
        }
        return out;
    }
    let chunk = images.len().div_ceil(MAX_THREADS).max(1);
    std::thread::scope(|scope| {
        for (sources, slots) in images.chunks(chunk).zip(out.chunks_mut(chunk)) {
            scope.spawn(move || {
                for (full, slot) in sources.iter().zip(slots.iter_mut()) {
                    if let Some(p) = gallery_thumbnail(Path::new(full), cache_dir) {
                        *slot = path_to_fwd_slash(&p);
                    }
                }
            });
        }
    });
    out
}

/// Drop the oldest thumbnails when the cache grows past its cap. Nothing else
/// ever deletes from it, so without this it only grows - and it also sweeps
/// up `.part` files orphaned by a crash mid-write.
fn prune_gallery_cache(cache_dir: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else { return };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        total += meta.len();
        files.push((entry.path(), meta.len(), modified));
    }
    if total <= max_bytes {
        return;
    }
    // Oldest first, and delete down to 80% so pruning isn't triggered again
    // by the very next thumbnail.
    files.sort_by_key(|(_, _, modified)| *modified);
    let target = max_bytes / 5 * 4;
    let mut removed = 0u64;
    for (path, len, _) in files {
        if total - removed <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += len;
        }
    }
    log::info!(
        "Gallery cache pruned: {:.1} MB freed (was {:.1} MB)",
        removed as f64 / 1_048_576.0,
        total as f64 / 1_048_576.0
    );
}

/// Category-priority for the gallery strip. Ordered by typical visual impact
/// and resolution quality in LaunchBox's database: 3D box renders lead (1-2 MB
/// PNGs at 1500-2000 px), then the authentic 2D box art (smaller JPGs), then
/// back/spine/disc packaging, screenshots, fanart, advertisements, posters,
/// and the long tail. Folder names match XODOSMetadata.zip's `Images/MS-DOS/`
/// tree exactly.
const IMAGE_CATEGORY_ORDER: &[&str] = &[
    "Box - 3D",
    "Box - Front",
    "Box - Back",
    "Box - Spine",
    "Cart - Front",
    "Cart - Back",
    "Disc",
    "Clear Logo",
    "Screenshot - Gameplay",
    "Screenshot - Title",
    "Screenshot - Game Title",
    "Screenshot - Game Select",
    "Screenshot - Game Over",
    "Screenshot - High Scores",
    "Fanart - Box - Front",
    "Fanart - Background",
    "Fanart - Disc",
    "Advertisement Flyer - Front",
    "Advertisement Flyer - Back",
    "Poster",
    "Banner",
    "Arcade - Marquee",
];

/// Strip a trailing `-NN` (hyphen + digits) from a filename stem so
/// "Capitalism-03" matches title "Capitalism" exactly rather than collision-
/// matching into "Capitalism Plus". Returns the stripped stem or the original.
fn strip_trailing_suffix_num(stem: &str) -> &str {
    if let Some(idx) = stem.rfind('-') {
        let tail = &stem[idx + 1..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return &stem[..idx];
        }
    }
    stem
}

/// Recursively walk a category directory, collecting image files whose
/// (suffix-stripped, normalized) stem equals `target_norm`. Depth-limited to
/// 3 levels to handle `<category>/<region>/file` nesting without ever going
/// off into pathological trees.
fn collect_matches_recursive(
    dir: &Path,
    depth: usize,
    target_norm: &str,
    priority: usize,
    out: &mut std::collections::BTreeMap<usize, Vec<String>>,
) {
    if depth > 3 {
        return;
    }
    let Ok(iter) = std::fs::read_dir(dir) else { return };
    for entry in iter.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_matches_recursive(&path, depth + 1, target_norm, priority, out);
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let base_stem = strip_trailing_suffix_num(stem);
        if normalize_alnum(base_stem) == target_norm {
            // Tauri's asset protocol can't serve files whose names start with
            // '.' (dots are treated as path-traversal components in the URL).
            // Rename them on first encounter - only affects 2 games in the
            // entire eXoDOS collection ("...A Personal Nightmare", ".386 Spys").
            let serve_path = if path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                let clean_name = path.file_name().unwrap().to_string_lossy()
                    .trim_start_matches('.').to_string();
                let clean = path.with_file_name(&clean_name);
                if !clean.exists() {
                    let _ = std::fs::rename(&path, &clean);
                }
                clean
            } else {
                path
            };
            if serve_path.exists() {
                out.entry(priority).or_default().push(path_to_fwd_slash(&serve_path));
            }
        }
    }
}

/// Scan the installed metadata content pack for a game's image assets.
/// Walks `<install_dir>/Images/MS-DOS/<category>/` folders and collects every
/// file whose (suffix-stripped, normalized) stem equals the normalized game
/// title. Returns empty images (not an error) when the pack isn't installed -
/// the frontend renders a Media section only if something is present.
#[tauri::command]
pub async fn get_game_metadata(
    db_state: State<'_, DbState>,
    collection: String,
    title: String,
    #[allow(unused_variables)] shortcode: Option<String>,
    manual_path: Option<String>,
) -> Result<GameMetadata, String> {
    let data_dir = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir")
            .map_err(|e| e.to_string())?
            .ok_or("Data directory not configured")?
    };

    tauri::async_runtime::spawn_blocking(move || {
        scan_game_metadata(&data_dir, &collection, &title, manual_path.as_deref())
    })
    .await
    .map_err(|e| format!("metadata scan panicked: {}", e))?
}

fn scan_game_metadata(
    data_dir: &str,
    collection: &str,
    title: &str,
    manual_path: Option<&str>,
) -> Result<GameMetadata, String> {
    let base = PathBuf::from(data_dir).join("content").join("metadata");

    // LP collections fall back to their base collection's metadata if their own
    // pack isn't installed (mirrors get_poster_dir). Both are checked so an
    // LP-installed user still picks up eXoDOS box art when no LP pack exists.
    // A collection with its own game tree gets no fallback - its games are not
    // in the other pack, and a same-title hit would show the wrong art.
    let mut roots: Vec<PathBuf> = vec![base.join(collection)];
    roots.extend(asset_fallback(collection).map(|c| base.join(c)));

    // Media subtrees are per LaunchBox platform, not per collection.
    let platform = collection_def(collection).map(|c| c.platform).unwrap_or("MS-DOS");

    let target_norm = normalize_alnum(title);
    let mut by_category: std::collections::BTreeMap<usize, Vec<String>> =
        std::collections::BTreeMap::new();

    for root in &roots {
        let images_root = root.join("Images").join(platform);
        if !images_root.is_dir() {
            continue;
        }
        let Ok(dir_iter) = std::fs::read_dir(&images_root) else { continue };
        for entry in dir_iter.flatten() {
            let category_path = entry.path();
            if !category_path.is_dir() {
                continue;
            }
            let category_name = category_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let priority = IMAGE_CATEGORY_ORDER
                .iter()
                .position(|n| *n == category_name)
                .unwrap_or(usize::MAX);

            collect_matches_recursive(
                &category_path,
                0,
                &target_norm,
                priority,
                &mut by_category,
            );
        }
        if !by_category.is_empty() {
            break;
        }
    }

    let mut images: Vec<String> = Vec::new();
    for (_, mut paths) in by_category {
        paths.sort();
        images.extend(paths);
    }

    // Resolve manual. Lookup order:
    //   1. Torrent root (downloaded game extracted manual)
    //   2. Content metadata pack for this collection (ships manuals without download)
    //   3. eXoDOS metadata pack as LP fallback (LP packs share EN manuals)
    //   4. Lazy-extract from GameData ZIP (legacy path before metadata packs)
    let torrent_root = game_root(data_dir);
    let (resolved_manual, manual_kind) = if let Some(mp) = manual_path {
        let normalized = mp.replace('\\', "/");
        let pack_base = PathBuf::from(data_dir).join("content").join("metadata");
        let mut candidates: Vec<PathBuf> = vec![
            torrent_root.join(&normalized),
            pack_base.join(collection).join(&normalized),
        ];
        candidates.extend(
            asset_fallback(collection).map(|c| pack_base.join(c).join(&normalized)),
        );

        let found = candidates.into_iter().find(|p| p.is_file());
        if let Some(path) = found {
            let kind = manual_kind_from_path(&path);
            (Some(path_to_fwd_slash(&path)), Some(kind.to_string()))
        } else {
            match extract_manual_from_gamedata(&torrent_root, collection, &normalized) {
                Ok(Some(extracted)) => {
                    let kind = manual_kind_from_path(&extracted);
                    (Some(path_to_fwd_slash(&extracted)), Some(kind.to_string()))
                }
                _ => (None, None),
            }
        }
    } else {
        (None, None)
    };

    // Gallery thumbnails: cheap after the first open of a game, and the strip
    // then loads ~5 KB per image instead of up to 18 MB.
    let cache_dir = gallery_cache_dir(data_dir);
    let thumbnails = generate_gallery_thumbnails(&images, &cache_dir);

    Ok(GameMetadata {
        manual_path: resolved_manual,
        manual_kind,
        images,
        thumbnails,
    })
}

/// Map a manual file extension to the kind tag the frontend renders against.
fn manual_kind_from_path(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str())
        .unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "text" => "txt",
        "html" | "htm" => "html",
        _ => "pdf",
    }
}

/// Extract a single manual file from a GameData ZIP on first access.
/// GameData ZIPs live at `<torrent_root>/Content/GameData/<collection>/<Title (Year)>.zip`
/// and contain `Manuals/MS-DOS/<Title (Year)>.{pdf,txt,doc}`.
///
/// `manual_rel` is the forward-slash-normalized ManualPath from the XML
/// (e.g. "Manuals/MS-DOS/Capitalism (1995).pdf").
fn extract_manual_from_gamedata(
    torrent_root: &Path,
    collection: &str,
    manual_rel: &str,
) -> Result<Option<PathBuf>, String> {
    // Derive the GameData ZIP name from the manual filename.
    // ManualPath: "Manuals/MS-DOS/Capitalism (1995).pdf"
    // GameData ZIP: "Content/GameData/eXoDOS/Capitalism (1995).zip"
    let manual_filename = Path::new(manual_rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Cannot parse manual filename")?;

    // Determine the GameData subdirectory. For eXoDOS it's "eXoDOS", for LP
    // collections they also use "eXoDOS" as the GameData folder (shared).
    let gd_collection = if collection.starts_with("eXoDOS") {
        "eXoDOS"
    } else {
        collection
    };
    let gd_dir = torrent_root.join("Content").join("GameData").join(gd_collection);

    // Try exact filename match first, then scan for case-insensitive match.
    let zip_name = format!("{}.zip", manual_filename);
    let gd_zip = gd_dir.join(&zip_name);
    let zip_path = if gd_zip.is_file() {
        gd_zip
    } else {
        // Scan directory for a case-insensitive match
        let Ok(entries) = std::fs::read_dir(&gd_dir) else {
            return Ok(None);
        };
        let lower = zip_name.to_lowercase();
        match entries.flatten().find(|e| {
            e.file_name().to_string_lossy().to_lowercase() == lower
        }) {
            Some(e) => e.path(),
            None => return Ok(None),
        }
    };

    // Skip placeholder / missing files (game not downloaded yet).
    if !zip_path.is_file() || zip_path.metadata().map(|m| m.len()).unwrap_or(0) == 0 {
        return Ok(None);
    }

    let file = std::fs::File::open(&zip_path)
        .map_err(|e| format!("Cannot open GameData zip: {}", e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Invalid GameData zip: {}", e))?;

    // Find the Manuals/ entry (case-insensitive prefix).
    let manual_entry_name = (0..archive.len())
        .find_map(|i| {
            let entry = archive.by_index(i).ok()?;
            let name = entry.name().to_string();
            if name.to_lowercase().starts_with("manuals/") && !name.ends_with('/') {
                Some(name)
            } else {
                None
            }
        });

    let Some(entry_name) = manual_entry_name else {
        return Ok(None);
    };

    let dest = torrent_root.join(&entry_name);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create manual dir: {}", e))?;
    }

    let mut entry = archive.by_name(&entry_name)
        .map_err(|e| format!("Cannot read manual from zip: {}", e))?;
    let mut out = std::fs::File::create(&dest)
        .map_err(|e| format!("Cannot write manual: {}", e))?;
    std::io::copy(&mut entry, &mut out)
        .map_err(|e| format!("Cannot extract manual: {}", e))?;

    log::info!("Lazy-extracted manual: {}", dest.display());
    Ok(Some(dest))
}

/// Return the default parent directory for game storage ($HOME).
/// The eXoDOS folder will be created inside this directory by the torrent engine.
#[tauri::command]
pub async fn get_default_data_dir() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| "Cannot determine home directory".to_string())?;
    Ok(home)
}

/// Whether a directory holds nothing Exodium would recognise as game data.
///
/// "Change game folder" POINTS Exodium at a folder, it does not move anything
/// into one - so an empty target is the signature of the misunderstanding: the
/// user meant to relocate their library and is about to end up with an empty
/// view and their games still on the old disk. OS metadata does not count as
/// content; a Finder visit is not an install.
#[tauri::command]
pub async fn data_dir_is_empty(path: String) -> Result<bool, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Ok(true);
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
    Ok(!entries
        .flatten()
        .any(|entry| !is_os_metadata(&entry.path())))
}

/// All known eXo collections.
/// Language packs are listed BEFORE eXoDOS so their games are matched to the
/// correct torrent before eXoDOS can claim same-title translations.
/// To add a new collection, append a CollectionDef entry here - no other
/// Rust file needs to be changed for path/emulator dispatch.
pub const COLLECTION_MAP: &[CollectionDef] = &[
    CollectionDef {
        id: "eXoDOS_GLP",
        display_name: "German Language Pack",
        metadata_file: "GLP.xml.gz",
        torrent_file: "eXoDOS_GLP.torrent",
        configs_zip: Some("GLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!german"),
        platform: "MS-DOS",
        year_subdirs: false,
    },
    CollectionDef {
        id: "eXoDOS_PLP",
        display_name: "Polish Language Pack",
        metadata_file: "PLP.xml.gz",
        torrent_file: "eXoDOS_PLP.torrent",
        configs_zip: Some("PLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!polish"),
        platform: "MS-DOS",
        year_subdirs: false,
    },
    CollectionDef {
        id: "eXoDOS_SLP",
        display_name: "Spanish Language Pack",
        metadata_file: "SLP.xml.gz",
        torrent_file: "eXoDOS_SLP.torrent",
        configs_zip: Some("SLP_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: Some("!spanish"),
        platform: "MS-DOS",
        year_subdirs: false,
    },
    CollectionDef {
        id: "eXoDOS",
        display_name: "eXoDOS",
        metadata_file: "MS-DOS.xml.gz",
        torrent_file: "eXoDOS.torrent",
        configs_zip: Some("eXoDOS_configs.zip"),
        inner_folder: "eXoDOS",
        game_prefix: "eXo/eXoDOS",
        shortcode_segment: "!dos",
        lang_dir: None,
        platform: "MS-DOS",
        year_subdirs: false,
    },
    // First collection with an inner_folder of its own: the eXoWin3x torrent
    // carries the internal name "eXoWin3x", so it cannot collide with the four
    // eXoDOS torrents and writes to <data_dir>/eXoWin3x/ instead.
    CollectionDef {
        id: "eXoWin3x",
        display_name: "eXoWin3x",
        metadata_file: "Win3x.xml.gz",
        torrent_file: "eXoWin3x.torrent",
        configs_zip: Some("Win3x_configs.zip"),
        inner_folder: "eXoWin3x",
        game_prefix: "eXo/eXoWin3x",
        shortcode_segment: "!win3x",
        lang_dir: None,
        platform: "Windows 3x",
        year_subdirs: false,
    },
    // eXoWin9x nests its games one level deeper than every other pack
    // (`eXo/eXoWin9x/<year>/<Title (Year)>.zip`) and has no 8-char shortcodes:
    // the title directory doubles as the shortcode. Games boot Windows 95/98
    // inside DOSBox-X (or 86Box) from VHD images - Staging cannot run them.
    CollectionDef {
        id: "eXoWin9x",
        display_name: "eXoWin9x",
        metadata_file: "Win9x.xml.gz",
        torrent_file: "eXoWin9x.torrent",
        configs_zip: Some("Win9x_configs.zip"),
        inner_folder: "eXoWin9x",
        game_prefix: "eXo/eXoWin9x",
        shortcode_segment: "!win9x",
        lang_dir: None,
        platform: "Windows 9x",
        year_subdirs: true,
    },
];

/// Resolve bundled metadata directory.
///
/// Dev mode reads straight from the repo tree via CARGO_MANIFEST_DIR. Prod
/// mode looks inside the Tauri resource_dir cached by `init_resource_dir`
/// at app startup. current_exe().parent() is NOT used because on macOS
/// that's Contents/MacOS/ while bundled resources live in Contents/Resources/.
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

/// Get info about the bundled torrent without starting anything.
#[tauri::command]
pub async fn get_torrent_info() -> Result<TorrentInfo, String> {
    let torrent_path = bundled_torrent_path("eXoDOS.torrent")?;
    let index =
        TorrentIndex::from_file(&torrent_path).map_err(|e| format!("Failed to parse torrent: {}", e))?;

    let metadata_size = index
        .find_metadata_zip()
        .map(|f| f.size)
        .unwrap_or(0);

    Ok(TorrentInfo {
        name: index.name.clone(),
        file_count: index.files.len(),
        total_size: index.total_size,
        metadata_size,
    })
}

/// Initialize the download system and start downloading metadata.
#[tauri::command]
pub async fn setup_start(
    app: tauri::AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    data_dir: String,
) -> Result<String, String> {
    use tauri::Manager;

    // Save data_dir to config. A fresh install keeps the historical root
    // name, so nothing existing has to move.
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
        queries::set_config(&conn, "root_folder", DEFAULT_ROOT_FOLDER).map_err(|e| e.to_string())?;
    }
    set_root_folder(DEFAULT_ROOT_FOLDER);

    let torrent_path = bundled_torrent_path("eXoDOS.torrent")?;
    let data_path = PathBuf::from(&data_dir);

    // Same session root as init_download_manager (app config dir, NOT the
    // data dir). The old data-dir session split the piece ledger from the
    // main flow: fastresume earned during setup was invisible afterwards
    // (forcing a full ~14k-file revalidation on the first game download) and
    // its data-dir persistence was cleaned by neither factory_reset nor
    // eviction, leaving stale ledgers behind.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let persistence_dir = fastresume_dir(&config_dir);
    seed_fastresume_bitvs(&persistence_dir, &["eXoDOS"], &data_path);
    let session = DownloadManager::create_session(&config_dir, &persistence_dir)
        .await
        .map_err(|e| e.to_string())?;
    evict_mismatched_session_torrents(&session, &persistence_dir, &data_path).await;
    apply_transfer_preferences(&session, &db_state);
    let manager =
        DownloadManager::new_with_session(session, &torrent_path, &data_path, &persistence_dir)
            .map_err(|e| format!("Failed to init download manager: {}", e))?;

    // Find metadata files in the torrent
    let metadata_idx = manager
        .index()
        .find_metadata_zip()
        .map(|f| f.index)
        .ok_or("XODOSMetadata.zip not found in torrent")?;

    let dosbox_idx = manager
        .index()
        .find_dosbox_metadata_zip()
        .map(|f| f.index);

    // Queue metadata files for download
    let mut files_to_download = vec![metadata_idx];
    if let Some(idx) = dosbox_idx {
        files_to_download.push(idx);
    }

    manager
        .download_files(files_to_download)
        .await
        .map_err(|e| format!("Failed to start metadata download: {}", e))?;

    let manager = Arc::new(manager);
    torrent_state.0.write().await.insert("eXoDOS".to_string(), manager);

    Ok("Metadata download started".to_string())
}

/// Poll setup progress (metadata download + import status).
#[tauri::command]
pub async fn get_setup_status(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<SetupStatus, String> {
    // Clone the Arc so we can drop the read guard before any .await points.
    // Holding the guard across awaits blocks factory_reset's write lock indefinitely.
    let manager_arc = {
        let guard = torrent_state.0.read().await;
        guard.get("eXoDOS").cloned()
    };

    let manager = match manager_arc {
        Some(ref m) => m,
        None => {
            // Ready if data_dir is configured AND the game DB has content
            let (has_data_dir, count) = {
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                load_root_folder(&conn);
                let dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
                let count = queries::count_games(&conn, "").map_err(|e| e.to_string())?;
                (dir.is_some(), count)
            };
            let ready = has_data_dir && count > 0;
            return Ok(SetupStatus {
                phase: if ready {
                    "ready".to_string()
                } else {
                    "not_started".to_string()
                },
                metadata_progress: None,
                dosbox_metadata_progress: None,
                games_imported: count,
                ready,
            });
        }
    };

    let metadata_idx = manager.index().find_metadata_zip().map(|f| f.index);
    let dosbox_idx = manager.index().find_dosbox_metadata_zip().map(|f| f.index);

    let metadata_progress = if let Some(idx) = metadata_idx {
        manager.file_progress(idx).await
    } else {
        None
    };

    let dosbox_progress = if let Some(idx) = dosbox_idx {
        manager.file_progress(idx).await
    } else {
        None
    };

    let metadata_done = metadata_progress
        .as_ref()
        .map(|p| p.finished)
        .unwrap_or(false);

    // Check if games are already imported
    let games_imported = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::count_games(&conn, "").map_err(|e| e.to_string())?
    };

    let phase = if games_imported > 0 {
        "ready"
    } else if metadata_done {
        "metadata_ready"
    } else if metadata_progress.is_some() {
        "downloading_metadata"
    } else {
        "starting"
    };

    Ok(SetupStatus {
        phase: phase.to_string(),
        metadata_progress,
        dosbox_metadata_progress: dosbox_progress,
        games_imported,
        ready: games_imported > 0,
    })
}

/// After metadata ZIP is downloaded, extract and import games.
#[tauri::command]
pub async fn setup_import(
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
) -> Result<usize, String> {
    // Clone the Arc and drop the read guard immediately (same pattern as
    // get_setup_status) - holding it across the multi-second import blocks
    // factory_reset's write lock into its 5s timeout.
    let manager = {
        let guard = torrent_state.0.read().await;
        guard
            .get("eXoDOS")
            .cloned()
            .ok_or("Download manager not initialized")?
    };

    // Find the downloaded metadata ZIP path
    let metadata_idx = manager
        .index()
        .find_metadata_zip()
        .map(|f| f.index)
        .ok_or("Metadata ZIP not found in torrent")?;

    if !manager.is_file_complete(metadata_idx).await {
        return Err("Metadata ZIP is still downloading".to_string());
    }

    let zip_path = manager
        .file_output_path(metadata_idx)
        .ok_or("Cannot determine metadata ZIP path")?;

    if !zip_path.exists() {
        return Err(format!("Metadata ZIP not found at: {}", zip_path.display()));
    }

    // Get DB path for a separate connection
    let db_path = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.path()
            .map(PathBuf::from)
            .ok_or_else(|| "Cannot determine database path".to_string())?
    };

    // Clone what we need for the blocking task
    let torrent_index = manager.index().clone();
    let zip = zip_path.clone();

    // Also extract !DOSmetadata.zip (DOSBox configs) if downloaded
    let dosbox_zip_path = manager
        .index()
        .find_dosbox_metadata_zip()
        .and_then(|f| manager.file_output_path(f.index))
        .filter(|p| p.exists());

    let torrent_root = manager.torrent_root();

    let count = tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open(&db_path).map_err(|e| e.to_string())?;
        db::init(&conn).map_err(|e| e.to_string())?;

        // Clear existing games to prevent duplicates on re-import
        queries::clear_games(&conn).map_err(|e| e.to_string())?;

        let count =
            import::import_from_zip(&zip, &conn, "!dos").map_err(|e| e.to_string())?;

        // Extract !DOSmetadata.zip to torrent root so eXo/eXoDOS/!dos/ is available
        if let Some(dosbox_zip) = dosbox_zip_path {
            log::info!("Extracting DOSBox configs to {}", torrent_root.display());
            let file = std::fs::File::open(&dosbox_zip).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
            archive.extract(&torrent_root).map_err(|e| e.to_string())?;
        }

        match_torrent_indices(&conn, &torrent_index, "eXoDOS").map_err(|e| e.to_string())?;

        Ok::<usize, String>(count)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(count)
}

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
fn match_torrent_indices(
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

/// Import from an existing eXoDOS directory on disk (skips metadata download).
/// The user selects the eXoDOS folder itself; the parent is stored as data_dir
/// so that new downloads land correctly inside the existing eXoDOS tree.
#[tauri::command]
pub async fn setup_from_local(
    app: AppHandle,
    db_state: State<'_, DbState>,
    torrent_state: State<'_, TorrentState>,
    exodos_path: String,
) -> Result<usize, String> {
    let root = PathBuf::from(&exodos_path);

    // The data_dir is the parent of the selected eXoDOS folder.
    // librqbit will write new downloads to <data_dir>/eXoDOS/ which equals the selected path.
    let data_dir = root
        .parent()
        .ok_or("Selected path has no parent directory")?
        .to_string_lossy()
        .to_string();

    // The imported folder IS the root - whatever the user called it. eXo's
    // setup does not dictate a name, and assuming "eXoDOS" made every path
    // miss for anyone who had named it differently or merged their packs.
    let root_folder = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| DEFAULT_ROOT_FOLDER.to_string());

    // Save data_dir, the root folder and all collections to config
    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::set_config(&conn, "data_dir", &data_dir).map_err(|e| e.to_string())?;
        queries::set_config(&conn, "root_folder", &root_folder).map_err(|e| e.to_string())?;
        // Remembered, not just passed to this one scan: every later startup
        // scan has to know that on THIS install the archives on disk are the
        // library, or it treats them as somebody else's download debris.
        queries::set_config(&conn, "library_from_disk", "1").map_err(|e| e.to_string())?;
        let all_collections = COLLECTION_MAP.iter().map(|c| c.id).collect::<Vec<_>>().join(",");
        queries::set_config(&conn, "collections", &all_collections).map_err(|e| e.to_string())?;
    }
    set_root_folder(&root_folder);
    crate::allow_asset_dir(&app, &PathBuf::from(&data_dir));

    // The bundled DB already has the full game catalog - no need to re-parse the
    // eXoDOS XML (XODOSMetadata.zip is 5 GB and would block for minutes).
    // Just report how many games are in the current DB.
    let count = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT COUNT(*) FROM games", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    };

    // Init download managers for all collections (for future game downloads).
    // Session state goes in the app config dir, game files in the existing eXoDOS tree.
    // Build managers WITHOUT holding the write lock - create_session is async and can be slow.
    let config_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let data_path = PathBuf::from(&data_dir);
    let persistence_dir = fastresume_dir(&config_dir);

    // Same fastresume seed as init_download_manager - see notes there. This
    // path is hit by `setup_from_local`, where the data_dir often already
    // contains pre-extracted games, so the seed is a no-op except for
    // truly empty cases. That's the correct behavior.
    let collection_ids: Vec<&str> = COLLECTION_MAP.iter().map(|c| c.id).collect();
    seed_fastresume_bitvs(&persistence_dir, &collection_ids, &data_path);

    // The torrent file lists are needed in BOTH network modes: the LP backfill
    // below wires game_torrent_index from them, and an offline user who later
    // switches to live must find those indices already populated.
    let mut torrent_indices: Vec<(String, TorrentIndex)> = Vec::new();
    for col in COLLECTION_MAP {
        if let Ok(col_torrent_path) = bundled_torrent_path(col.torrent_file) {
            match TorrentIndex::from_file(&col_torrent_path) {
                Ok(idx) => torrent_indices.push((col.id.to_string(), idx)),
                Err(e) => log::warn!("Failed to parse {} torrent: {}", col.id, e),
            }
        }
    }

    let offline = is_offline(&db_state.0);
    let mut new_managers = Vec::new();
    if offline {
        log::info!("Offline mode: importing local collection without starting the torrent engine");
    } else {
        let session = DownloadManager::create_session(&config_dir, &persistence_dir)
            .await
            .map_err(|e| format!("Failed to init session: {}", e))?;
        evict_mismatched_session_torrents(&session, &persistence_dir, &data_path).await;
        apply_transfer_preferences(&session, &db_state);

        for col in COLLECTION_MAP {
            if let Ok(col_torrent_path) = bundled_torrent_path(col.torrent_file) {
                match DownloadManager::new_with_session(Arc::clone(&session), &col_torrent_path, &data_path, &persistence_dir) {
                    Ok(mgr) => new_managers.push((col.id.to_string(), Arc::new(mgr))),
                    Err(e) => log::warn!("Failed to init {} download manager: {}", col.id, e),
                }
            }
        }
        set_union_cleanup_keep_paths(&new_managers);
        for (id, mgr) in &new_managers {
            if mgr.hydrate_from_session().await {
                log::info!("{}: adopted persisted torrent from session", id);
            }
        }
    }

    // Backfill any LP collections that are absent from the DB.
    // This happens when the DB was originally built from XODOSMetadata.zip (EN only) by the
    // old setup path.  The bundled .xml.gz files always include all LP catalogs, so we import
    // whichever collections are missing and then run match_torrent_indices to wire up
    // torrent_source / game_torrent_index.
    if let Ok(metadata_dir) = bundled_metadata_dir() {
        for (col_id, torrent_index) in &torrent_indices {
            let col = match collection_def(col_id) {
                Some(c) => c,
                None => continue,
            };
            if col.lang_dir.is_none() {
                continue; // base eXoDOS - skip
            }

            let already_in_db: i64 = {
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                conn.query_row(
                    "SELECT COUNT(*) FROM games WHERE torrent_source = ?1",
                    rusqlite::params![col_id],
                    |r| r.get(0),
                )
                .unwrap_or(0)
            };
            if already_in_db > 0 {
                continue;
            }

            let xml_gz = metadata_dir.join(col.metadata_file);
            if !xml_gz.exists() {
                log::warn!("Bundled metadata not found for {}: {}", col_id, xml_gz.display());
                continue;
            }

            let imported = {
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                import::import_from_gz(&xml_gz, &conn, col.shortcode_segment)
                    .unwrap_or_else(|e| {
                        log::warn!("Failed to import {} XML: {}", col_id, e);
                        0
                    })
            };
            log::info!("Backfilled {} {} games from bundled XML", imported, col_id);

            if imported > 0 {
                // Wire up game_torrent_index and torrent_source for the newly imported rows.
                let col_id_owned = col_id.clone();
                let conn = db_state.0.lock().map_err(|e| e.to_string())?;
                if let Err(e) = match_torrent_indices(&conn, torrent_index, &col_id_owned) {
                    log::warn!("match_torrent_indices failed for {}: {}", col_id_owned, e);
                }
            }
        }

        // Populate thumbnail_key for every game whose row got its hash wiped
        // by `import_bundled_metadata`'s clear_games() + XML re-import above.
        // Without this the library would show no covers after first-run setup
        // even though the bundled preview pack has everything. Shared helper
        // in db::populate_thumbnail_keys uses the same hash function as
        // gen_thumbnails.py and generate_db.rs.
        {
            let conn = db_state.0.lock().map_err(|e| e.to_string())?;
            db::populate_thumbnail_keys(&conn).map_err(|e| e.to_string())?;
        }

        // Backfill shortcodes, dosbox_conf and has_thumbnail for LP games that lack them.
        // PLP and SLP XMLs use a path format without the "!dos/<shortcode>" segment, so their
        // shortcodes (and derived dosbox_conf) come out as NULL after import.  Mirror the same
        // two-step approach as generate_db.rs: exact EN title match first.
        // This is idempotent - rows already having values are unaffected.
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        // Every LP↔EN match is family-scoped (CLAUDE.md §1): titles and
        // shortcodes repeat across pack families, and an unscoped match hands
        // an LP row another family's shortcode - which orphans it from its
        // variant group. There is deliberately NO thumbnail_key copy here:
        // db::propagate_lp_thumbnail_keys below owns that rule, and a second
        // copy of it drifted from the first once already.
        let same = db::queries::same_group("en", "games");
        let fam_en = db::queries::family_expr("en");
        let fam_g = db::queries::family_expr("games");
        let _ = conn.execute_batch(&format!(
            "-- Inherit shortcode from the matching EN game by exact title
             UPDATE games
             SET shortcode = (
                 SELECT en.shortcode FROM games en
                 WHERE en.language = 'EN'
                   AND en.shortcode IS NOT NULL
                   AND en.title = games.title
                   AND {fam_en} = {fam_g}
                 LIMIT 1
             )
             WHERE shortcode IS NULL;

             -- Second pass: normalized title match (handles punctuation differences like
             -- 'Foo - Bar' vs 'Foo: Bar').  Only touches LP rows still without a shortcode.
             UPDATE games
             SET shortcode = (
                 SELECT en.shortcode FROM games en
                 WHERE en.language = 'EN'
                   AND en.shortcode IS NOT NULL
                   AND {fam_en} = {fam_g}
                   AND LOWER(TRIM(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(
                         en.title, ':', ' '), '-', ' '), ',', ''), '.', ''), '  ', ' ')))
                     = LOWER(TRIM(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(
                         games.title, ':', ' '), '-', ' '), ',', ''), '.', ''), '  ', ' ')))
                 LIMIT 1
             )
             WHERE shortcode IS NULL AND language != 'EN';

             -- LP games use the EN DOSBox config; backfill dosbox_conf from the EN variant
             UPDATE games
             SET dosbox_conf = (
                 SELECT en.dosbox_conf FROM games en
                 WHERE {same}
                   AND en.language = 'EN'
                   AND en.dosbox_conf IS NOT NULL
                 LIMIT 1
             )
             WHERE dosbox_conf IS NULL AND shortcode IS NOT NULL;

             -- Propagate has_thumbnail flag from EN variant (same group = same cover art)
             UPDATE games
             SET has_thumbnail = 1
             WHERE has_thumbnail = 0
               AND EXISTS (
                   SELECT 1 FROM games en
                   WHERE en.language = 'EN' AND en.has_thumbnail = 1 AND {same}
               );",
        ));
        log::info!("LP shortcode/dosbox_conf/has_thumbnail/thumbnail_key backfill complete");

        // Pass 3: pull shortcodes for LP-exclusive games from the bundled static DB.
        // metadata/exodium.db (built by generate_db.rs) contains 100% shortcode coverage
        // including LP-exclusive games with no EN equivalent (via generate_shortcode()).
        // Passes 1 & 2 only matched titles present in the EN catalog; this covers the rest.
        //
        // ATTACH and DETACH are issued as separate calls so DETACH always runs even when an
        // UPDATE fails - execute_batch stops at the first error, which would leave lp_static
        // attached for the lifetime of the connection if DETACH were part of the same batch.
        let static_db = metadata_dir.join("exodium.db");
        if static_db.exists() {
            let path_esc = static_db.to_string_lossy().replace('\'', "''");
            let attach_ok = conn
                .execute_batch(&format!("ATTACH DATABASE '{path}' AS lp_static;", path = path_esc))
                .is_ok();
            if attach_ok {
                // Title matches against the static DB are family-scoped too -
                // it catalogues every pack, and eXoWin9x shares titles with
                // the LP rows this pass is meant to serve.
                let fam_s = db::queries::family_expr("s");
                let result = conn.execute_batch(&format!(
                    "UPDATE games
                     SET shortcode = (
                         SELECT s.shortcode FROM lp_static.games s
                         WHERE s.title = games.title AND s.shortcode IS NOT NULL
                           AND {fam_s} = {fam_g}
                         LIMIT 1
                     )
                     WHERE shortcode IS NULL AND language != 'EN';
                     UPDATE games
                     SET has_thumbnail = COALESCE((
                         SELECT s.has_thumbnail FROM lp_static.games s
                         WHERE s.title = games.title AND {fam_s} = {fam_g}
                         LIMIT 1
                     ), has_thumbnail)
                     WHERE language != 'EN' AND shortcode IS NOT NULL;
                     UPDATE games
                     SET thumbnail_key = COALESCE((
                         SELECT s.thumbnail_key FROM lp_static.games s
                         WHERE s.title = games.title AND s.thumbnail_key IS NOT NULL
                           AND {fam_s} = {fam_g}
                         LIMIT 1
                     ), thumbnail_key)
                     WHERE thumbnail_key IS NULL AND language != 'EN';
                     UPDATE games
                     SET dosbox_conf = (
                         SELECT en.dosbox_conf FROM games en
                         WHERE {same}
                           AND en.language = 'EN'
                           AND en.dosbox_conf IS NOT NULL
                         LIMIT 1
                     )
                     WHERE dosbox_conf IS NULL AND shortcode IS NOT NULL;",
                ));
                let _ = conn.execute_batch("DETACH DATABASE lp_static;");
                match result {
                    Ok(_) => log::info!("Pass 3: LP-exclusive shortcode backfill from static DB complete"),
                    Err(e) => log::warn!("Pass 3: LP-exclusive shortcode backfill from static DB failed: {}", e),
                }
            } else {
                log::warn!("Pass 3: failed to attach {:?}, skipping LP backfill", static_db);
            }
        } else {
            log::debug!("Static exodium.db not found at {:?}, skipping Pass 3 LP backfill", static_db);
        }

        // Any rows still with NULL thumbnail_key (Pass 3 static-DB backfill
        // might have added new rows without keys) get their own-title hash.
        db::populate_thumbnail_keys(&conn).map_err(|e| e.to_string())?;

        // Final pass: match LP titles to EN via canonical form (article-
        // stripped, word-numbers-as-digits, etc.) and overwrite LP's
        // thumbnail_key with EN's. Catches the ~575 LP games whose auto-
        // generated shortcode diverged from EN but whose titles are clearly
        // the same game (e.g. PL "Legend of Kyrandia Book 2" ↔ EN
        // "The Legend of Kyrandia: Book Two").
        db::propagate_lp_thumbnail_keys(&conn).map_err(|e| e.to_string())?;
    }

    // Briefly acquire write lock just to insert - no awaits inside this block.
    {
        let mut managers = torrent_state.0.write().await;
        for (id, mgr) in new_managers {
            managers.insert(id, mgr);
        }
    }

    // The user's existing eXoDOS tree already has all DOSBox configs in place.
    // No need to extract !DOSmetadata.zip - that's only required when downloading from scratch.
    // (init_download_manager handles the bundled configs zip for fresh installs.)

    // Scan the existing eXoDOS tree to mark games that are already on disk as installed.
    let installed_count = scan_installed_games_with_db(&db_state.0, &data_dir, true)
        .unwrap_or_else(|e| { log::warn!("scan_installed_games failed: {}", e); 0 });
    log::info!("Import from local complete: {} games, {} installed, data_dir={}", count, installed_count, data_dir);

    Ok(count)
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
fn scan_installed_games_with_db(
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
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?
    };
    let data_dir = data_dir.ok_or("data_dir not configured")?;
    scan_installed_games_with_db(&db_state.0, &data_dir, adopt.unwrap_or(false))
}

/// Result of validating a candidate eXoDOS installation directory.
#[derive(Debug, Serialize)]
pub struct ExodosValidation {
    pub valid: bool,
    pub hint: String,
}

/// Check whether a directory looks like a valid eXoDOS installation.
/// Expects the folder the user selected to BE the eXoDOS folder (e.g. ~/eXoDOS),
/// which should contain eXo/eXoDOS/ with at least one game language subdirectory.
#[tauri::command]
pub async fn validate_exodos_dir(path: String) -> Result<ExodosValidation, String> {
    let root = Path::new(&path);
    let game_root = root.join("eXo/eXoDOS");

    if !game_root.is_dir() {
        return Ok(ExodosValidation {
            valid: false,
            hint: "Not a valid eXoDOS folder (eXo/eXoDOS/ not found)".to_string(),
        });
    }

    let has_games = ["!dos", "!german", "!polish", "!spanish"]
        .iter()
        .any(|sub| game_root.join(sub).is_dir());

    if !has_games {
        return Ok(ExodosValidation {
            valid: false,
            hint: "No game directories found inside eXo/eXoDOS/".to_string(),
        });
    }

    // Count top-level game directories as a rough hint
    let count: usize = std::fs::read_dir(&game_root)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0);

    Ok(ExodosValidation {
        valid: true,
        hint: format!("Valid eXoDOS installation (~{} directories)", count),
    })
}

/// Resolve bundled torrent file path.
fn bundled_torrent_path(filename: &str) -> Result<PathBuf, String> {
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
mod real_pack_tests {
    use super::*;

    /// Opt-in check against a REAL installed metadata pack, because synthetic
    /// fixtures cannot model what actually costs time here: eXo's box art is
    /// photographic PNGs up to 18 MB, which compress and decode nothing like a
    /// generated test image.
    ///
    ///   EXODIUM_REAL_DATA_DIR=/path/to/data cargo test real_pack -- --ignored --nocapture
    #[test]
    #[ignore]
    fn scan_against_a_real_metadata_pack() {
        let Ok(data_dir) = std::env::var("EXODIUM_REAL_DATA_DIR") else {
            eprintln!("set EXODIUM_REAL_DATA_DIR to run this");
            return;
        };
        let title = std::env::var("EXODIUM_REAL_TITLE")
            .unwrap_or_else(|_| "Magic Carpet Plus".to_string());

        // Cold: whatever this game costs on its first open.
        let cold_start = std::time::Instant::now();
        let meta = scan_game_metadata(&data_dir, "eXoDOS", &title, None).expect("scan");
        let cold = cold_start.elapsed();

        assert!(!meta.images.is_empty(), "no art found for {} - wrong title?", title);
        assert_eq!(meta.images.len(), meta.thumbnails.len());

        let full: u64 = meta.images.iter()
            .map(|p| std::fs::metadata(Path::new(p)).map(|m| m.len()).unwrap_or(0)).sum();
        let thumbs: u64 = meta.thumbnails.iter()
            .map(|p| std::fs::metadata(Path::new(p)).map(|m| m.len()).unwrap_or(0)).sum();

        // Warm: every later open, and every open after a restart.
        let warm_start = std::time::Instant::now();
        let again = scan_game_metadata(&data_dir, "eXoDOS", &title, None).expect("rescan");
        let warm = warm_start.elapsed();
        assert_eq!(again.thumbnails, meta.thumbnails);

        eprintln!(
            "\n{}: {} images\n  payload  {:.0} KB -> {:.0} KB  ({:.0}x smaller)\n  cold scan {:?}\n  warm scan {:?}\n",
            title, meta.images.len(),
            full as f64 / 1024.0, thumbs as f64 / 1024.0,
            full as f64 / thumbs.max(1) as f64, cold, warm,
        );

        assert!(thumbs < full / 5, "expected a big reduction on real art: {} vs {}", thumbs, full);
        assert!(warm < cold, "a cached scan must be faster than a decoding one");
    }
}

#[cfg(test)]
mod metadata_scan_tests {
    use super::*;

    /// Build a data dir shaped like a real install with the metadata pack
    /// extracted: <data>/content/metadata/<collection>/Images/MS-DOS/<cat>/…
    fn make_pack(data: &Path, collection: &str, title: &str, categories: &[&str]) {
        for (i, cat) in categories.iter().enumerate() {
            let dir = data.join("content").join("metadata").join(collection)
                .join("Images").join("MS-DOS").join(cat);
            std::fs::create_dir_all(&dir).unwrap();
            // Big enough that a 160px thumbnail is unambiguously smaller.
            let img = image::RgbImage::from_fn(1200, 900, |x, y| {
                image::Rgb([(x % 251) as u8, (y % 253) as u8, (i * 40) as u8])
            });
            img.save(dir.join(format!("{}-01.png", title))).unwrap();
        }
    }

    fn temp_data(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("exodium_scan_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The whole point of the cache: the gallery strip must be served small
    /// copies while the lightbox keeps the originals.
    #[test]
    fn scan_returns_cached_thumbnails_alongside_full_images() {
        let data = temp_data("thumbs");
        make_pack(&data, "eXoDOS", "Magic Carpet Plus",
                  &["Box - 3D", "Screenshot - Gameplay", "Clear Logo"]);

        let meta = scan_game_metadata(data.to_str().unwrap(), "eXoDOS", "Magic Carpet Plus", None)
            .expect("scan");

        assert_eq!(meta.images.len(), 3, "one image per category");
        assert_eq!(meta.thumbnails.len(), meta.images.len(), "arrays must stay aligned");

        let mut full_total = 0u64;
        let mut thumb_total = 0u64;
        for (full, thumb) in meta.images.iter().zip(meta.thumbnails.iter()) {
            assert_ne!(full, thumb, "strip must not be handed the full-size file");
            assert!(thumb.contains("/thumbcache/"), "thumbnail should live in the cache dir");
            // The bug this encodes: Tauri's asset-protocol scope glob does not
            // match hidden path components, so a dotted cache dir is served to
            // nobody - 202 denials in one session before this was found.
            assert!(
                !thumb.split('/').any(|part| part.starts_with('.')),
                "cache path must have no hidden component: {}", thumb
            );
            let (w, h) = image::image_dimensions(Path::new(thumb)).unwrap();
            assert!(w <= THUMB_MAX_EDGE && h <= THUMB_MAX_EDGE, "{}x{}", w, h);
            full_total += std::fs::metadata(Path::new(full)).unwrap().len();
            thumb_total += std::fs::metadata(Path::new(thumb)).unwrap().len();
        }
        // Absolute, not a ratio: a synthetic fixture's PNG/JPEG behaviour says
        // nothing about real box art (measured there: a 2.7 MB gallery -> 39 KB).
        // What the cache guarantees is a small bounded payload per image.
        assert!(thumb_total < 30_000, "gallery payload too large: {} bytes", thumb_total);
        assert!(full_total > thumb_total, "{} vs {}", full_total, thumb_total);

        // Second open must reuse the cache rather than decode again.
        let before: Vec<_> = meta.thumbnails.iter()
            .map(|t| std::fs::metadata(Path::new(t)).unwrap().modified().unwrap())
            .collect();
        let again = scan_game_metadata(data.to_str().unwrap(), "eXoDOS", "Magic Carpet Plus", None)
            .expect("second scan");
        assert_eq!(again.thumbnails, meta.thumbnails);
        for (t, was) in again.thumbnails.iter().zip(before) {
            assert_eq!(std::fs::metadata(Path::new(t)).unwrap().modified().unwrap(), was,
                       "cached file must not be rewritten");
        }

        let _ = std::fs::remove_dir_all(&data);
    }

    /// LP collections fall back to the eXoDOS pack, and that fallback has to
    /// keep working now that thumbnails sit in between.
    #[test]
    fn lp_collection_falls_back_to_the_english_pack() {
        let data = temp_data("lpfallback");
        make_pack(&data, "eXoDOS", "Das Amt", &["Box - Front"]);

        let meta = scan_game_metadata(data.to_str().unwrap(), "eXoDOS_GLP", "Das Amt", None)
            .expect("scan");

        assert_eq!(meta.images.len(), 1);
        assert!(meta.images[0].contains("/eXoDOS/"), "resolved from the EN pack");
        assert!(meta.thumbnails[0].contains("/thumbcache/"));
        assert!(!meta.thumbnails[0].split('/').any(|p| p.starts_with('.')));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_game_without_art_returns_empty_arrays() {
        let data = temp_data("noart");
        make_pack(&data, "eXoDOS", "Some Other Game", &["Box - 3D"]);

        let meta = scan_game_metadata(data.to_str().unwrap(), "eXoDOS", "Nothing Here", None)
            .expect("scan");

        assert!(meta.images.is_empty());
        assert!(meta.thumbnails.is_empty());
        let _ = std::fs::remove_dir_all(&data);
    }
}

#[cfg(test)]
mod scan_tests {
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

    /// The merge has to END with the old folder gone, and it must resolve
    /// duplicates in favour of the real data: the loser is a zero-byte
    /// torrent placeholder or a half-finished download, never the archive.
    #[test]
    fn merging_keeps_the_larger_copy_and_clears_the_old_folder() {
        let dir = std::env::temp_dir().join(format!("exodium_merge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = dir.join("eXoWin9x/eXo/eXoWin9x/1995");
        let new_root = dir.join("eXoDOS/eXo/eXoWin9x/1995");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new_root).unwrap();
        std::fs::write(old.join("Moved.zip"), b"only in the old tree").unwrap();
        // The old tree holds the real archive, the new one a placeholder.
        std::fs::write(old.join("Real.zip"), b"the actual game archive").unwrap();
        std::fs::write(new_root.join("Real.zip"), b"").unwrap();
        // And the other way round.
        std::fs::write(old.join("Placeholder.zip"), b"").unwrap();
        std::fs::write(new_root.join("Placeholder.zip"), b"already downloaded").unwrap();

        // Finder leaves one of these in every folder it has been shown.
        std::fs::write(dir.join("eXoWin9x/.DS_Store"), b"finder junk").unwrap();

        let stray = dir.join("eXoWin9x");
        let tally = merge_tree(&stray, &dir.join("eXoDOS")).unwrap();
        remove_empty_tree(&stray);

        assert!(new_root.join("Moved.zip").is_file(), "new files move across");
        assert_eq!(
            std::fs::read_to_string(new_root.join("Real.zip")).unwrap(),
            "the actual game archive",
            "a placeholder must not win over real data"
        );
        assert_eq!(
            std::fs::read_to_string(new_root.join("Placeholder.zip")).unwrap(),
            "already downloaded",
            "and the existing archive is kept when the old side is empty"
        );
        assert_eq!(tally.moved, 2, "Moved.zip and the larger Real.zip");
        assert_eq!(tally.deduped, 1, "the zero-byte Placeholder.zip is dropped");
        assert_eq!(tally.skipped, 0);
        assert!(!stray.exists(), "the old folder must be gone, or we ask again forever");
        let _ = std::fs::remove_dir_all(&dir);
    }

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

#[cfg(test)]
mod network_mode_tests {
    use super::*;
    use std::sync::Mutex;

    fn db_with(pairs: &[(&str, &str)]) -> Mutex<rusqlite::Connection> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        for (k, v) in pairs {
            queries::set_config(&conn, k, v).unwrap();
        }
        Mutex::new(conn)
    }

    #[test]
    fn missing_network_mode_means_live() {
        // Installs from before the setting exists must keep downloading.
        assert!(!is_offline(&db_with(&[])));
    }

    #[test]
    fn only_the_exact_offline_value_disables_the_engine() {
        assert!(is_offline(&db_with(&[("network_mode", "offline")])));
        assert!(!is_offline(&db_with(&[("network_mode", "live")])));
        assert!(!is_offline(&db_with(&[("network_mode", "")])));
        assert!(!is_offline(&db_with(&[("network_mode", "Offline")])));
    }

    /// Sharing uploads copyrighted data, so anything short of an explicit yes
    /// has to read as no - including the unset key of an older install.
    #[test]
    fn seeding_requires_an_explicit_yes() {
        assert!(seeding_enabled(&db_with(&[("seeding_enabled", "1")])));
        assert!(!seeding_enabled(&db_with(&[])));
        assert!(!seeding_enabled(&db_with(&[("seeding_enabled", "0")])));
        assert!(!seeding_enabled(&db_with(&[("seeding_enabled", "true")])));
    }

    /// A cap of 0 would throttle to nothing and the UI has no way to express
    /// it, so it has to read as "unlimited" like an absent or broken value.
    #[test]
    fn rate_limits_treat_zero_and_junk_as_unlimited() {
        assert_eq!(
            rate_limits(&db_with(&[("rate_limit_up_kbps", "500"), ("rate_limit_down_kbps", "2000")])),
            (Some(500), Some(2000))
        );
        assert_eq!(rate_limits(&db_with(&[])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "")])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "0")])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "-5")])), (None, None));
        assert_eq!(rate_limits(&db_with(&[("rate_limit_up_kbps", "abc")])), (None, None));
    }

    /// The offline branch of init_download_manager is exactly this call, and
    /// the import flow depends on it: an offline install still needs eXo's
    /// DOSBox configs, which normally arrive with the torrent.
    #[test]
    fn offline_still_extracts_bundled_configs() {
        let dir = std::env::temp_dir().join(format!("exodium_offline_cfg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = dir.join("metadata");
        std::fs::create_dir_all(&meta).unwrap();

        // A stand-in for eXoDOS_configs.zip carrying one config file.
        let zip_path = meta.join("eXoDOS_configs.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file::<_, ()>("eXo/eXoDOS/!dos/TEST/dosbox.conf", Default::default()).unwrap();
            use std::io::Write;
            zip.write_all(b"[sdl]\nfullscreen=false\n").unwrap();
            zip.finish().unwrap();
        }

        let data = dir.join("data");
        extract_all_bundled_configs(&["eXoDOS"], Some(&meta), &data);

        let extracted = data.join("eXoDOS").join("eXo/eXoDOS/!dos/TEST/dosbox.conf");
        assert!(extracted.is_file(), "configs must land even with no torrent session");
        assert!(data.join("eXoDOS").join(".eXoDOS_configs_extracted").is_file(),
                "lock file marks the extraction as done");

        // Second call is a no-op thanks to the lock: delete the payload and
        // confirm nothing restores it.
        std::fs::remove_file(&extracted).unwrap();
        extract_all_bundled_configs(&["eXoDOS"], Some(&meta), &data);
        assert!(!extracted.exists(), "lock file must prevent a re-extract");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Collections the user did not enable must be skipped entirely.
    #[test]
    fn disabled_collections_are_not_extracted() {
        let dir = std::env::temp_dir().join(format!("exodium_offline_skip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = dir.join("metadata");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(meta.join("eXoDOS_configs.zip"), b"not a zip").unwrap();

        let data = dir.join("data");
        extract_all_bundled_configs(&["eXoDOS_GLP"], Some(&meta), &data);

        assert!(!data.join("eXoDOS").join(".eXoDOS_configs_extracted").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod gallery_cache_tests {
    use super::*;

    /// Write a deliberately oversized PNG, the shape of the real problem: the
    /// metadata pack's gallery images run to 18 MB while the strip draws them
    /// at 64x48.
    fn write_source(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let path = dir.join(name);
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        img.save(&path).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("exodium_thumbtest_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generates_a_much_smaller_thumbnail() {
        let dir = temp_dir("small");
        let cache = dir.join("cache");
        let src = write_source(&dir, "big.png", 1600, 1200);

        let thumb = gallery_thumbnail(&src, &cache).expect("thumbnail");
        assert!(thumb.is_file());

        let (sw, sh) = image::image_dimensions(&src).unwrap();
        let (tw, th) = image::image_dimensions(&thumb).unwrap();
        assert!(tw <= THUMB_MAX_EDGE && th <= THUMB_MAX_EDGE, "got {}x{}", tw, th);
        // Aspect ratio preserved (thumbnail() fits inside the box).
        assert_eq!(sw / sh, tw / th);
        // Absolute, not a ratio against the source: a synthetic gradient PNG
        // compresses far better than the pack's real box art, so a ratio test
        // would measure the fixture instead of the thumbnail. On real assets
        // this comes out around 5 KB (measured: a 2.7 MB gallery -> 39 KB).
        let thumb_bytes = std::fs::metadata(&thumb).unwrap().len();
        assert!(thumb_bytes < 30_000, "thumbnail too large: {} bytes", thumb_bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reuses_the_cached_file_on_the_second_call() {
        let dir = temp_dir("reuse");
        let cache = dir.join("cache");
        let src = write_source(&dir, "img.png", 800, 600);

        let first = gallery_thumbnail(&src, &cache).unwrap();
        let stamp = std::fs::metadata(&first).unwrap().modified().unwrap();
        let second = gallery_thumbnail(&src, &cache).unwrap();

        assert_eq!(first, second);
        // Same file, untouched - a regenerated one would carry a newer mtime.
        assert_eq!(stamp, std::fs::metadata(&second).unwrap().modified().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A replaced metadata pack must not keep serving the old thumbnail: the
    /// cache key includes the source's size and mtime, not just its path.
    #[test]
    fn a_changed_source_misses_the_cache() {
        let dir = temp_dir("changed");
        let cache = dir.join("cache");
        let src = write_source(&dir, "img.png", 800, 600);
        let first = gallery_thumbnail(&src, &cache).unwrap();

        std::fs::remove_file(&src).unwrap();
        write_source(&dir, "img.png", 400, 400);
        let second = gallery_thumbnail(&src, &cache).unwrap();

        assert_ne!(first, second, "different source content must map to a different cache entry");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_drops_the_oldest_entries_when_over_the_cap() {
        let dir = temp_dir("prune");
        std::fs::create_dir_all(&dir).unwrap();
        // Five 10 KB files written oldest-first. A few ms apart so the mtime
        // ordering is unambiguous on filesystems with coarse timestamps.
        let mut paths = Vec::new();
        for i in 0..5 {
            let p = dir.join(format!("thumb{}.jpg", i));
            std::fs::write(&p, vec![0u8; 10 * 1024]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
            paths.push(p);
        }

        // Cap at 30 KB: pruning targets 80% of that, so it must free at least
        // 26 KB - the three oldest files.
        prune_gallery_cache(&dir, 30 * 1024);

        assert!(!paths[0].exists(), "oldest should be gone");
        assert!(!paths[1].exists());
        assert!(paths[4].exists(), "newest must survive");
        let left: u64 = std::fs::read_dir(&dir).unwrap()
            .flatten()
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        assert!(left <= 30 * 1024 / 5 * 4, "still over target: {} bytes", left);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_leaves_a_cache_under_the_cap_alone() {
        let dir = temp_dir("prune_noop");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("thumb.jpg");
        std::fs::write(&p, vec![0u8; 1024]).unwrap();

        prune_gallery_cache(&dir, 1024 * 1024);

        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole gallery is thumbnailed in parallel; every slot must line up
    /// with its source, and an undecodable file keeps its full-size path.
    #[test]
    fn parallel_generation_preserves_order_and_falls_back() {
        let dir = temp_dir("parallel");
        let cache = dir.join("cache");
        let mut sources = Vec::new();
        for i in 0..6 {
            sources.push(path_to_fwd_slash(&write_source(&dir, &format!("img{}.png", i), 200 + i * 10, 150)));
        }
        let broken = dir.join("broken.png");
        std::fs::write(&broken, b"nope").unwrap();
        sources.push(path_to_fwd_slash(&broken));

        let out = generate_gallery_thumbnails(&sources, &cache);

        assert_eq!(out.len(), sources.len());
        for (i, thumb) in out.iter().take(6).enumerate() {
            assert_ne!(thumb, &sources[i], "image {} should have been thumbnailed", i);
            let (w, h) = image::image_dimensions(Path::new(thumb)).unwrap();
            assert!(w <= THUMB_MAX_EDGE && h <= THUMB_MAX_EDGE);
        }
        assert_eq!(out[6], sources[6], "undecodable source keeps its own path");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_image_falls_back_instead_of_failing() {
        let dir = temp_dir("broken");
        let cache = dir.join("cache");
        let src = dir.join("not-an-image.png");
        std::fs::write(&src, b"this is not a PNG").unwrap();

        assert!(gallery_thumbnail(&src, &cache).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod data_dir_tests {
    use super::*;

    #[test]
    fn a_folder_with_only_os_metadata_counts_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".DS_Store"), b"x").unwrap();
        std::fs::write(tmp.path().join("._something"), b"x").unwrap();
        // A Finder visit is not an install - the warning has to fire here, or
        // it never fires for the macOS users who most need it.
        let empty = tauri::async_runtime::block_on(data_dir_is_empty(
            tmp.path().to_string_lossy().to_string(),
        ))
        .unwrap();
        assert!(empty);
    }

    #[test]
    fn a_folder_holding_game_data_is_not_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("eXo/eXoDOS")).unwrap();
        let empty = tauri::async_runtime::block_on(data_dir_is_empty(
            tmp.path().to_string_lossy().to_string(),
        ))
        .unwrap();
        assert!(!empty);
    }

    #[test]
    fn a_folder_that_does_not_exist_yet_counts_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let empty = tauri::async_runtime::block_on(data_dir_is_empty(
            missing.to_string_lossy().to_string(),
        ))
        .unwrap();
        assert!(empty);
    }
}
