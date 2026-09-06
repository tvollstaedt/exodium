//! Art and documents from the bundled previews and the content packs: cover
//! tiers for the grid (§13), the detail-panel gallery with its thumbnail
//! cache, and manuals.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::db::normalize_alnum;

use super::collections::{asset_fallback, collection_def};
use super::paths::{game_root, path_to_fwd_slash, RESOURCE_DIR};
use super::DbState;

/// The Tier 0 preview dir for a collection; probes every layout Tauri's
/// bundle.resources produces per platform.
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
    let conn = db_state.lock()?;
    let data_dir = crate::commands::games::configured_data_dir(&conn)?;
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

/// A game's gallery images (originals plus 160 px thumbnails) and manual.
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

/// The gallery thumbnail cache, under `content/` so a factory reset clears
/// it. Not a dotted name: the asset-scope glob skips hidden components.
fn gallery_cache_dir(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("content").join("thumbcache")
}

/// Remove caches from before the rename - they can never be served.
/// Startup housekeeping: nothing else deletes from the gallery cache.
pub(crate) fn prune_gallery_cache_at_startup(data_dir: &str) {
    remove_legacy_cache_dirs(data_dir);
    prune_gallery_cache(&gallery_cache_dir(data_dir), GALLERY_CACHE_MAX_BYTES);
}

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

/// Gallery order, best art first. Names match the pack's `Images/<platform>/`.
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

/// Images under a category dir whose normalized stem is `target_norm`, up
/// to 3 levels deep (`<category>/<region>/file`).
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
            // The asset protocol cannot serve dot-prefixed names; rename on
            // first encounter (two games).
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

/// A game's gallery from the installed metadata pack. Empty, not an error,
/// without the pack.
#[tauri::command]
pub async fn get_game_metadata(
    db_state: State<'_, DbState>,
    collection: String,
    title: String,
    #[allow(unused_variables)] shortcode: Option<String>,
    manual_path: Option<String>,
) -> Result<GameMetadata, String> {
    let data_dir = {
        let conn = db_state.lock()?;
        crate::commands::games::configured_data_dir(&conn)?
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

    // Own pack first, then the base collection's (`asset_fallback`).
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

    // Manual: extracted in the game root, the metadata pack (own, then
    // fallback collection), else pulled out of the GameData zip.
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

/// Extract one manual (`manual_rel`, e.g. `Manuals/MS-DOS/<Title>.pdf`) from
/// the game's GameData zip on first access.
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
        // Absolute bound: a ratio would measure the synthetic fixture.
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
mod real_pack_tests {
    use super::*;

    /// Against a REAL metadata pack (18 MB photographic PNGs decode nothing
    /// like a fixture):
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
