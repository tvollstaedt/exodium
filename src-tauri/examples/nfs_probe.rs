//! Times the startup and torrent-add filesystem passes against a data dir on
//! a slow filesystem (an NFS mount in the lab). Each pass is a real call
//! into the app's own code, so a fix is measurable with the same numbers.
//!
//! Run from src-tauri/:  cargo run --example nfs_probe -- <data dir> [--root eXoDOS] [--stat-samples 2000]
//!
//! Output, one line per pass: `PROBE <name> <seconds> <entries>`. Writes a
//! throwaway database in the temp dir, never the app's; the tree itself IS
//! modified (configs overwritten, placeholders removed) - point it at a
//! generated tree, not a library.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use exodium_lib::db::queries::set_config;
use exodium_lib::torrent::manager::cleanup_placeholder_files;
use exodium_lib::torrent::TorrentIndex;
use exodium_lib::{
    bundled_metadata_dir, extract_bundled_configs, game_root, install_bundled_db,
    scan_installed_games_with_db, set_root_folder, COLLECTION_MAP,
};

struct Args {
    data_dir: PathBuf,
    root: String,
    stat_samples: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut data_dir = None;
    let mut root = "eXoDOS".to_string();
    let mut stat_samples = 2000usize;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = args.next().ok_or("--root needs a value")?,
            "--stat-samples" => {
                stat_samples = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--stat-samples needs a number")?
            }
            _ if data_dir.is_none() => data_dir = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    Ok(Args {
        data_dir: data_dir.ok_or("usage: nfs_probe <data dir> [--root NAME] [--stat-samples N]")?,
        root,
        stat_samples,
    })
}

fn probe<T>(name: &str, f: impl FnOnce() -> (T, usize)) -> T {
    let start = Instant::now();
    let (out, entries) = f();
    println!("PROBE {name} {:.3} {entries}", start.elapsed().as_secs_f64());
    out
}

fn main() -> Result<(), String> {
    let args = parse_args()?;
    let data_dir = args
        .data_dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", args.data_dir.display()))?;
    let data_dir_str = data_dir.to_string_lossy().into_owned();

    let db_path = std::env::temp_dir().join(format!("exodium-nfs-probe-{}.db", std::process::id()));
    install_bundled_db(&db_path)?;
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    let collections = COLLECTION_MAP.iter().map(|c| c.id).collect::<Vec<_>>().join(",");
    for (key, value) in [
        ("data_dir", data_dir_str.as_str()),
        ("root_folder", args.root.as_str()),
        ("library_from_disk", "1"),
        ("collections", collections.as_str()),
    ] {
        set_config(&conn, key, value).map_err(|e| e.to_string())?;
    }
    set_root_folder(&args.root);
    let db = Mutex::new(conn);

    let root = game_root(&data_dir_str);
    if !root.join("eXo").is_dir() {
        return Err(format!("no eXo/ under {}", root.display()));
    }
    println!("root: {}", root.display());

    // Per-syscall latency: read_dir already knows the names, so each
    // metadata() is one round trip on NFS.
    let stat_dir = root.join("eXo/eXoDOS");
    probe("stat", || {
        let mut n = 0usize;
        if let Ok(entries) = std::fs::read_dir(&stat_dir) {
            for e in entries.flatten().take(args.stat_samples) {
                let _ = e.path().metadata();
                n += 1;
            }
        }
        ((), n)
    });

    // Full-tree walk with one stat per entry: the floor for any pass that
    // has to look at every file.
    probe("walk", || {
        let n = walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .count();
        ((), n)
    });

    probe("scan_startup", || {
        let n = scan_installed_games_with_db(&db, &data_dir_str, false, &Default::default()).unwrap_or(0);
        ((), n)
    });
    probe("scan_adopt", || {
        let n = scan_installed_games_with_db(&db, &data_dir_str, true, &Default::default()).unwrap_or(0);
        ((), n)
    });

    // The one-time extraction an import pays on its first start: the lock
    // file is removed so the pass really runs.
    let metadata_dir = bundled_metadata_dir()?;
    let exodos = COLLECTION_MAP
        .iter()
        .find(|c| c.id == "eXoDOS")
        .ok_or("eXoDOS missing from COLLECTION_MAP")?;
    let lock = root.join(".eXoDOS_configs_extracted");
    let _ = std::fs::remove_file(&lock);
    let cfg_entries = exodos
        .configs_zip
        .and_then(|z| std::fs::File::open(metadata_dir.join(z)).ok())
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .map(|a| a.len())
        .unwrap_or(0);
    probe("extract_configs", || {
        extract_bundled_configs(exodos, Some(&metadata_dir), &root);
        ((), cfg_entries)
    });

    // Placeholder cleanup with the union keep-list, as after a torrent add.
    let torrents_dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("torrents");
    let keep: Vec<String> = COLLECTION_MAP
        .iter()
        .filter_map(|c| TorrentIndex::from_file(&torrents_dir.join(c.torrent_file)).ok())
        .flat_map(|idx| idx.files.into_iter().map(|f| f.path))
        .collect();
    probe("cleanup_placeholders", || {
        let _ = cleanup_placeholder_files(&root, &keep);
        ((), keep.len())
    });

    let _ = std::fs::remove_file(&db_path);
    Ok(())
}
