//! What the data folder holds, by what it is for - the Storage tab.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::db::queries;

use super::archives::{real_archive_bytes, zip_path_of};
use super::collections::{collection_rel_game_dir, COLLECTION_MAP};
use super::games::configured_data_dir;
use super::lp_overlay::dir_size;
use super::paths::game_root;
use super::user_data::remove_tree;
use super::DbState;

/// Every byte in the data folder lands in exactly one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Games,
    Archives,
    Extras,
    Saves,
    Support,
    Packs,
    PackArchives,
    Reading,
    Caches,
    Configs,
    Other,
}

const ORDER: [Category; 11] = [
    Category::Games, Category::Archives, Category::Extras, Category::Saves, Category::Support,
    Category::Packs, Category::PackArchives, Category::Reading, Category::Caches, Category::Configs,
    Category::Other,
];

#[derive(Debug, Serialize, Clone)]
pub struct CategoryUsage {
    pub id: Category,
    pub bytes: u64,
    /// Games, archives, backups: what the user would count. Zero where a
    /// count would mean nothing (support files, caches).
    pub items: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct StorageOverview {
    pub folder: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    /// The rest of the volume: total minus free (root reserve included,
    /// `f_bfree` not `f_bavail`) minus what Exodium holds.
    pub other_bytes: u64,
    pub categories: Vec<CategoryUsage>,
}

const CACHE_DIRS: [&str; 3] = ["videocache", "musiccache", "thumbcache"];
const PACK_DIRS: [&str; 4] = ["posters", "metadata", "emulators", "scummvm"];

fn is_year(s: &str) -> bool {
    s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_zip(name: &str) -> bool {
    name.len() > 4 && name.to_ascii_lowercase().ends_with(".zip")
}

/// Exodium's `content/` and eXo's `Content/` are one directory on a
/// case-insensitive filesystem, so both names dispatch on the same table.
fn classify_content(rest: &[&str], size: u64) -> (Category, Option<String>) {
    let Some(first) = rest.first() else { return (Category::Other, None) };
    let key = |depth: usize| rest.get(depth).map(|s| s.to_string());
    if rest.len() == 1 {
        return if is_zip(first) && size > 0 { (Category::PackArchives, key(0)) } else { (Category::Other, None) };
    }
    match *first {
        "GameData" => (Category::Extras, if rest.len() == 3 { key(2) } else { None }),
        "magazinecache" => (Category::Reading, key(1)),
        d if CACHE_DIRS.contains(&d) => (Category::Caches, None),
        d if PACK_DIRS.contains(&d) => (Category::Packs, None),
        "pristine" => (Category::Configs, None),
        _ => (Category::Other, None),
    }
}

/// `rest` starts below `eXo/<collection dir>/`. A year directory is only
/// stripped where the collection has them; eXoDOS's shortcodes include
/// `1939` and `1993` (§16).
fn classify_game_tree(rest: &[&str], size: u64, year_subdirs: bool) -> (Category, Option<String>) {
    let mut rest = rest;
    if let Some(first) = rest.first() {
        let lang = COLLECTION_MAP.iter().any(|c| c.lang_dir == Some(*first));
        if lang || (year_subdirs && is_year(first)) {
            rest = &rest[1..];
        }
    }
    let Some(first) = rest.first() else { return (Category::Other, None) };
    if *first == "!save" {
        return (Category::Saves, rest.get(1).map(|s| s.to_string()));
    }
    if first.starts_with('!') || first.starts_with('.') {
        return (Category::Configs, None);
    }
    if rest.len() == 1 {
        return if is_zip(first) && size > 0 { (Category::Archives, Some(first.to_string())) } else { (Category::Other, None) };
    }
    (Category::Games, Some(first.to_string()))
}

/// Where a file under the data folder belongs. `rel` is relative to the
/// game root when the file sits under it, else to the data folder.
fn classify(rel: &[&str], in_root: bool, size: u64) -> (Category, Option<String>) {
    let Some(first) = rel.first() else { return (Category::Other, None) };
    if first.eq_ignore_ascii_case("content") {
        return classify_content(&rel[1..], size);
    }
    if !in_root || *first != "eXo" {
        return (Category::Other, None);
    }
    match rel.get(1).copied() {
        Some("emulators") | Some("util") => (Category::Support, None),
        Some("Magazines") => (Category::Reading, rel.get(2).map(|s| s.to_string())),
        None => (Category::Other, None),
        Some(dir) => match COLLECTION_MAP.iter().find(|c| c.game_prefix == format!("eXo/{dir}")) {
            Some(col) => classify_game_tree(&rel[2..], size, col.year_subdirs),
            None => (Category::Other, None),
        },
    }
}

#[cfg(unix)]
fn allocated(meta: &std::fs::Metadata, _path: &Path, _maybe_sparse: bool) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks() * 512
}

/// Windows has no allocated size without a handle, and every open goes
/// through Defender - so only torrent payloads, the files librqbit marks
/// sparse, pay for it. Pack images and caches are never sparse.
#[cfg(not(unix))]
fn allocated(meta: &std::fs::Metadata, path: &Path, maybe_sparse: bool) -> u64 {
    use fs4::FileExt;
    if !maybe_sparse || meta.len() == 0 {
        return meta.len();
    }
    std::fs::File::open(path)
        .and_then(|f| f.allocated_size())
        .unwrap_or(meta.len())
}

type Tally = (
    std::collections::HashMap<Category, u64>,
    std::collections::HashMap<Category, HashSet<String>>,
);

fn account(data_dir: &Path, root: &Path, entry: &walkdir::DirEntry, tally: &mut Tally) {
    if !entry.file_type().is_file() {
        return;
    }
    let Ok(meta) = entry.metadata() else { return };
    let path = entry.path();
    let root_is_data = root == data_dir;
    let (base, in_root) = match path.strip_prefix(root) {
        Ok(_) if !root_is_data => (root, true),
        _ => (data_dir, root_is_data),
    };
    let Ok(rel) = path.strip_prefix(base) else { return };
    let comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let refs: Vec<&str> = comps.iter().map(String::as_str).collect();
    let (mut cat, mut key) = classify(&refs, in_root, meta.len());
    // A piece-sized fragment left by a neighbour's download (§16) is a zip
    // by name only.
    if cat == Category::Archives && real_archive_bytes(path).is_none() {
        cat = Category::Other;
        key = None;
    }
    let maybe_sparse = matches!(cat, Category::Archives | Category::Extras | Category::PackArchives | Category::Other | Category::Support);
    *tally.0.entry(cat).or_default() += allocated(&meta, path, maybe_sparse);
    if let Some(k) = key {
        tally.1.entry(cat).or_default().insert(k);
    }
}

/// One stat per file is the cost, and a large library is 100k+ files - so
/// the depth-2 subtrees are walked on all cores.
fn walk(data_dir: &Path, root: &Path) -> Vec<CategoryUsage> {
    // Jobs: the depth-2 directories, except the collection trees under
    // `eXo/`, which hold most files - those split into their children, with
    // their own loose files (the archives) as one shallow job.
    let mut jobs: Vec<(PathBuf, Option<usize>)> = Vec::new();
    let mut shallow = Tally::default();
    let root_depth = if root == data_dir { 0 } else { 1 };
    for entry in walkdir::WalkDir::new(data_dir).max_depth(2 + root_depth).into_iter().filter_map(Result::ok) {
        let depth = entry.depth();
        let is_dir = entry.file_type().is_dir();
        let under_exo = entry
            .path()
            .strip_prefix(root)
            .ok()
            .and_then(|r| r.components().next())
            .is_some_and(|c| c.as_os_str() == "eXo");
        if is_dir && depth == 2 + root_depth && under_exo {
            jobs.push((entry.path().to_path_buf(), Some(1)));
            if let Ok(children) = std::fs::read_dir(entry.path()) {
                jobs.extend(children.flatten().filter(|c| c.path().is_dir()).map(|c| (c.path(), None)));
            }
        } else if is_dir && depth == 2 && !under_exo {
            jobs.push((entry.path().to_path_buf(), None));
        } else if (depth <= 2 || (under_exo && depth == 2 + root_depth)) && !(is_dir && depth == 2 + root_depth) {
            account(data_dir, root, &entry, &mut shallow);
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(jobs.len().max(1));
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut tally = Tally::default();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((job, max_depth)) = jobs.get(i) else { break };
                        let mut walker = walkdir::WalkDir::new(job).min_depth(1);
                        if let Some(d) = max_depth {
                            walker = walker.max_depth(*d);
                        }
                        for entry in walker.into_iter().filter_map(Result::ok) {
                            account(data_dir, root, &entry, &mut tally);
                        }
                    }
                    tally
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().expect("storage walk worker panicked")).collect()
    });
    let mut bytes = shallow.0;
    let mut items = shallow.1;
    for (b, i) in tallies {
        for (c, n) in b {
            *bytes.entry(c).or_default() += n;
        }
        for (c, set) in i {
            items.entry(c).or_default().extend(set);
        }
    }
    ORDER
        .iter()
        .map(|c| CategoryUsage {
            id: *c,
            bytes: bytes.get(c).copied().unwrap_or(0),
            items: items.get(c).map(|s| s.len() as u64).unwrap_or(0),
        })
        .collect()
}

#[tauri::command]
pub async fn storage_overview(db_state: State<'_, DbState>) -> Result<StorageOverview, String> {
    let data_dir = {
        let conn = db_state.lock()?;
        configured_data_dir(&conn)?
    };
    tauri::async_runtime::spawn_blocking(move || {
        let root = game_root(&data_dir);
        let data = Path::new(&data_dir);
        let started = std::time::Instant::now();
        let categories = walk(data, &root);
        log::info!("storage_overview: measured {} in {:?}", data.display(), started.elapsed());
        let used_bytes: u64 = categories.iter().map(|c| c.bytes).sum();
        let total_bytes = fs4::total_space(data).unwrap_or(0);
        let free_all = fs4::free_space(data).unwrap_or(0);
        Ok(StorageOverview {
            folder: data_dir.clone(),
            free_bytes: fs4::available_space(data).unwrap_or(0),
            total_bytes,
            used_bytes,
            other_bytes: total_bytes.saturating_sub(free_all).saturating_sub(used_bytes),
            categories,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize, Clone)]
pub struct GameStorage {
    pub id: i64,
    pub title: String,
    pub collection: String,
    pub language: String,
    pub game_bytes: u64,
    pub archive_bytes: u64,
    pub save_bytes: u64,
    pub last_played: Option<String>,
}

/// Every installed game with what it occupies, for the list under the bar.
#[tauri::command]
pub async fn installed_games_storage(db_state: State<'_, DbState>) -> Result<Vec<GameStorage>, String> {
    let (games, root) = {
        let conn = db_state.lock()?;
        let data_dir = configured_data_dir(&conn)?;
        (queries::fetch_installed_rows(&conn).map_err(|e| e.to_string())?, game_root(&data_dir))
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::with_capacity(games.len());
        for game in games {
            let (Some(id), Some(shortcode)) = (game.id, game.shortcode.as_deref()) else { continue };
            let source = game.torrent_source.clone().unwrap_or_else(|| "eXoDOS".into());
            let rel = collection_rel_game_dir(&source, shortcode, game.application_path.as_deref());
            let game_dir = root.join(&rel);
            let save_dir = match rel.rsplit_once('/') {
                Some((parent, leaf)) => root.join(parent).join("!save").join(leaf),
                None => root.join("!save").join(&rel),
            };
            let measure = |d: &Path| if d.is_dir() { dir_size(d) } else { 0 };
            out.push(GameStorage {
                id,
                title: game.title.clone(),
                collection: source,
                language: game.language.clone(),
                game_bytes: measure(&game_dir),
                archive_bytes: real_archive_bytes(&zip_path_of(&root, &game)).unwrap_or(0),
                save_bytes: measure(&save_dir),
                last_played: game.last_played.clone(),
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize, Clone, Copy, Default)]
pub struct Freed {
    pub items: u64,
    pub bytes: u64,
}

fn remove_children(dir: &Path, keep: impl Fn(&Path) -> bool, freed: &mut Freed) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.flatten() {
        let path = entry.path();
        if keep(&path) {
            continue;
        }
        let bytes = if path.is_dir() { dir_size(&path) } else { entry.metadata().map(|m| m.len()).unwrap_or(0) };
        let result = if path.is_dir() { remove_tree(&path) } else { std::fs::remove_file(&path) };
        result.map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        freed.items += 1;
        freed.bytes += bytes;
    }
    Ok(())
}

/// Preview videos, theme tracks and gallery thumbnails: all re-fetched on
/// demand, so nothing is lost. The `.novideo`/`.nomusic` markers stay - each
/// one cost a torrent piece to learn (§14).
#[tauri::command]
pub async fn clear_media_caches(db_state: State<'_, DbState>) -> Result<Freed, String> {
    let data_dir = {
        let conn = db_state.lock()?;
        configured_data_dir(&conn)?
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut freed = Freed::default();
        let is_marker = |p: &Path| {
            matches!(p.extension().and_then(|e| e.to_str()), Some("novideo") | Some("nomusic"))
        };
        for name in CACHE_DIRS {
            remove_children(&PathBuf::from(&data_dir).join("content").join(name), is_marker, &mut freed)?;
        }
        Ok(freed)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The `!save` backups of games nobody came back for: not a game that is in
/// the library (its download is what restores the backup), and not a backup
/// that holds a whole game folder (marked by `whole_dir_marker`).
#[tauri::command]
pub async fn delete_save_backups(db_state: State<'_, DbState>) -> Result<Freed, String> {
    let (root, keep_rel) = {
        let conn = db_state.lock()?;
        let root = game_root(&configured_data_dir(&conn)?);
        let mut keep = HashSet::new();
        let mut stmt = conn
            .prepare("SELECT shortcode, torrent_source, application_path FROM games WHERE in_library = 1 OR installed = 1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))
            .map_err(|e| e.to_string())?;
        for (shortcode, source, app_path) in rows.flatten() {
            let Some(sc) = shortcode else { continue };
            let rel = collection_rel_game_dir(source.as_deref().unwrap_or("eXoDOS"), &sc, app_path.as_deref());
            if let Some((parent, leaf)) = rel.rsplit_once('/') {
                keep.insert(root.join(parent).join("!save").join(leaf));
            }
        }
        (root, keep)
    };
    tauri::async_runtime::spawn_blocking(move || {
        let keep = |p: &Path| {
            keep_rel.contains(p)
                || super::user_data::whole_dir_marker(p).is_file()
                || p.extension().and_then(|e| e.to_str()) == Some("whole")
        };
        let mut freed = Freed::default();
        let mut seen = HashSet::new();
        for col in COLLECTION_MAP {
            let base = root.join(col.game_prefix);
            let mut parents = vec![base.clone()];
            if let Some(lang) = col.lang_dir {
                parents.push(base.join(lang));
            }
            if col.year_subdirs {
                if let Ok(entries) = std::fs::read_dir(&base) {
                    parents.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                        p.is_dir() && p.file_name().map(|n| is_year(&n.to_string_lossy())).unwrap_or(false)
                    }));
                }
            }
            for parent in parents {
                let save = parent.join("!save");
                if save.is_dir() && seen.insert(save.clone()) {
                    remove_children(&save, keep, &mut freed)?;
                }
            }
        }
        Ok(freed)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(rel: &str, in_root: bool, size: u64) -> Category {
        let parts: Vec<&str> = rel.split('/').collect();
        classify(&parts, in_root, size).0
    }

    #[test]
    fn game_tree_paths_land_in_their_category() {
        assert_eq!(cat("eXo/eXoDOS/SQ5/SQ5.EXE", true, 1), Category::Games);
        assert_eq!(cat("eXo/eXoDOS/!german/AlienOdy/run.bat", true, 1), Category::Games);
        assert_eq!(cat("eXo/eXoWin9x/1995/Connect4 (1995)/game.vhd", true, 1), Category::Games);
        assert_eq!(cat("eXo/eXoDOS/1939/1939.EXE", true, 1), Category::Games, "a four-digit eXoDOS shortcode is a game");
        assert_eq!(cat("eXo/eXoDOS/Pokémon.txt", true, 1), Category::Other, "non-ASCII names must not panic");
        assert_eq!(cat("eXo/eXoDOS/Space Quest V (1993).zip", true, 500), Category::Archives);
        assert_eq!(cat("eXo/eXoDOS/Space Quest V (1993).zip", true, 0), Category::Other);
        assert_eq!(cat("eXo/eXoDOS/!save/SQ5/SQ5SG.001", true, 1), Category::Saves);
        assert_eq!(cat("eXo/eXoDOS/!german/!save/AlienOdy/x", true, 1), Category::Saves);
        assert_eq!(cat("eXo/eXoWin9x/1995/!save/Connect4 (1995)/x", true, 1), Category::Saves);
        assert_eq!(cat("eXo/eXoDOS/!dos/SQ5/dosbox.conf", true, 1), Category::Configs);
        assert_eq!(cat("eXo/emulators/dosbox/ece4230/DOSBox.exe", true, 1), Category::Support);
        assert_eq!(cat("eXo/util/utilWin9x.zip", true, 1), Category::Support);
        assert_eq!(cat("eXo/Magazines/BBD/004/run.bat", true, 1), Category::Reading);
        assert_eq!(cat("Content/GameData/eXoDOS/Space Quest V (1993).zip", true, 1), Category::Extras);
        assert_eq!(cat("Content/XODOSMetadata.zip", true, 9), Category::PackArchives);
        assert_eq!(cat("Content/XODOSMetadata.zip", true, 0), Category::Other);
    }

    /// The same names answer for `content/` at the data-dir level, where a
    /// case-insensitive filesystem folds it into eXo's `Content/`.
    #[test]
    fn exodium_content_dirs_are_split_by_name() {
        assert_eq!(cat("content/posters/eXoDOS/ab.jpg", false, 1), Category::Packs);
        assert_eq!(cat("content/emulators/dosbox-x/x", false, 1), Category::Packs);
        assert_eq!(cat("content/videocache/eXoDOS_1.mp4", false, 1), Category::Caches);
        assert_eq!(cat("content/magazinecache/abc.pdf", false, 1), Category::Reading);
        assert_eq!(cat("content/GameData/eXoDOS/x.zip", true, 1), Category::Extras);
        assert_eq!(cat("content/.content-downloads/x", false, 1), Category::Other);
        assert_eq!(cat("librqbit-fastresume/x", false, 1), Category::Other);
    }

    #[test]
    fn walk_counts_games_and_archives_once() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let root = data.join("eXoDOS");
        std::fs::create_dir_all(root.join("eXo/eXoDOS/SQ5/DATA")).unwrap();
        std::fs::write(root.join("eXo/eXoDOS/SQ5/SQ5.EXE"), vec![1u8; 4096]).unwrap();
        std::fs::write(root.join("eXo/eXoDOS/SQ5/DATA/RES.000"), vec![1u8; 4096]).unwrap();
        {
            let file = std::fs::File::create(root.join("eXo/eXoDOS/Space Quest V (1993).zip")).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            w.start_file("SQ5/SQ5.EXE", opts).unwrap();
            std::io::Write::write_all(&mut w, &[1u8; 4096]).unwrap();
            w.finish().unwrap();
        }
        std::fs::write(root.join("eXo/eXoDOS/Placeholder.zip"), b"").unwrap();
        std::fs::write(root.join("eXo/eXoDOS/Fragment.zip"), vec![0u8; 8192]).unwrap();
        std::fs::create_dir_all(data.join("content/videocache")).unwrap();
        std::fs::write(data.join("content/videocache/a.mp4"), vec![1u8; 4096]).unwrap();
        let usage = walk(data, &root);
        let get = |c: Category| usage.iter().find(|u| u.id == c).unwrap();
        assert_eq!(get(Category::Games).items, 1);
        assert!(get(Category::Games).bytes >= 8192);
        assert_eq!(get(Category::Archives).items, 1, "the fragment is not an archive");
        assert!(get(Category::Other).bytes >= 8192, "the fragment counts as leftovers");
        assert!(get(Category::Caches).bytes >= 4096);
        assert_eq!(get(Category::Other).items, 0);
    }
}
