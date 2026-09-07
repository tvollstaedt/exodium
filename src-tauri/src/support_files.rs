//! One-time support payloads inside a torrent's `eXo/util/*.zip` (one inner
//! zip, subtrees land under `eXo/`): queued with the first game that needs
//! them, extracted by a watcher, re-armed at startup. Readiness is directory
//! existence, so extraction stages and renames atomically.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::torrent::manager::DownloadManager;

pub(crate) struct SupportPack {
    /// Collection whose torrent carries the util zip.
    pub collection: &'static str,
    /// Suffix of the util zip's path inside the torrent.
    pub util_suffix: &'static str,
    /// Name of the nested zip holding the payload.
    pub inner_zip: &'static str,
    /// Subtrees to extract, lowercase with forward slashes and trailing `/`.
    /// Each becomes one rename unit under `eXo/`, real case preserved.
    pub prefixes: &'static [&'static str],
    /// What the log calls the payload.
    pub label: &'static str,
    /// Everything this pack provides is on disk.
    pub ready: fn(&Path) -> bool,
    /// Runs on the `eXo` dir after the renames.
    pub post: Option<fn(&Path)>,
    running: AtomicBool,
    /// Latched when the watcher gives up (3 failures, e.g. disk full), so a
    /// status query can say "failed" instead of "setting up" forever.
    failed: AtomicBool,
}

impl SupportPack {
    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

#[cfg(windows)]
const DOS_PREFIXES: &[&str] = &["mt32/", "emulators/dosbox/ece4230/", "emulators/dosbox/ece4460/"];
#[cfg(not(windows))]
const DOS_PREFIXES: &[&str] = &["mt32/"];

fn dos_ready(root: &Path) -> bool {
    root.join("eXo/mt32").exists()
        && (!cfg!(windows) || root.join("eXo/emulators/dosbox/ece4230").exists())
}

fn win9x_ready(root: &Path) -> bool {
    use crate::commands::win9x::win9x_support_ready;
    win9x_support_ready(root, None) && win9x_support_ready(root, Some("86box"))
}

/// MT-32/CM32L ROMs and soundfonts (~54 MB); on Windows also eXo's DOSBox
/// ECE builds. The rest of EXTDOS.zip (467 MB) is never extracted.
pub(crate) static DOS_SUPPORT: SupportPack = SupportPack {
    collection: "eXoDOS",
    util_suffix: "util/util.zip",
    inner_zip: "EXTDOS.zip",
    prefixes: DOS_PREFIXES,
    label: "MT-32 ROMs + SoundCanvas soundfont for MIDI music",
    ready: dos_ready,
    post: None,
    running: AtomicBool::new(false),
    failed: AtomicBool::new(false),
};

/// Windows 9x parent VHDs plus the emulators that boot them (x98 DOSBox-X,
/// 86Box). Needed on every platform - the VHDs are data. `emulators/PCBox/`
/// and `emulators/audio/` stay in the zip.
pub(crate) static WIN9X_SUPPORT: SupportPack = SupportPack {
    collection: "eXoWin9x",
    util_suffix: "util/utilWin9x.zip",
    inner_zip: "EXTWin9x.zip",
    // Not `emulators/dosbox/`: that directory also holds eXoDOS's ece builds.
    prefixes: &[
        "emulators/dosbox/x98/",
        "emulators/dosbox/config9x.bat",
        "emulators/dosbox/options9x.conf",
        "emulators/86box98/",
    ],
    label: "Windows 9x OS images + emulators",
    ready: win9x_ready,
    post: Some(crate::commands::win9x::add_parent_case_aliases),
    running: AtomicBool::new(false),
    failed: AtomicBool::new(false),
};

#[cfg(windows)]
const SCUMMVM_PREFIXES: &[&str] = &["mt32/", "emulators/scmvm/"];
#[cfg(not(windows))]
const SCUMMVM_PREFIXES: &[&str] = &["mt32/"];

pub(crate) fn scummvm_ready(root: &Path) -> bool {
    root.join("eXo/mt32").exists()
        && (!cfg!(windows) || root.join("eXo/emulators/scmvm").exists())
}

/// MT-32 ROMs and the SoundCanvas soundfont for ScummVM's sound menu; on
/// Windows also eXo's seven pinned ScummVM builds (§18).
pub(crate) static SCUMMVM_SUPPORT: SupportPack = SupportPack {
    collection: "eXoScummVM",
    util_suffix: "util/utilSVM.zip",
    inner_zip: "EXTsvm.zip",
    prefixes: SCUMMVM_PREFIXES,
    label: "MT-32 ROMs + SoundCanvas soundfont for ScummVM",
    ready: scummvm_ready,
    post: None,
    running: AtomicBool::new(false),
    failed: AtomicBool::new(false),
};

pub(crate) static PACKS: [&SupportPack; 3] = [&DOS_SUPPORT, &WIN9X_SUPPORT, &SCUMMVM_SUPPORT];

/// Where a pack's payload stands, for the detail panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SupportStatus {
    /// "ready" | "downloading" | "missing" | "failed"
    pub phase: String,
    /// Download progress 0..1 while phase == "downloading".
    pub progress: f32,
    /// Size of the util zip, so the panel can say what the one-time download
    /// costs. 0 when the torrent index is unavailable.
    pub total_bytes: u64,
}

impl SupportStatus {
    pub(crate) fn new(phase: &str, progress: f32, total_bytes: u64) -> Self {
        Self { phase: phase.into(), progress, total_bytes }
    }
}

/// The pack's status for the panel; "missing" when its collection has no
/// manager (offline, or the collection is not enabled).
pub(crate) async fn status_for(
    torrent_state: &crate::commands::TorrentState,
    pack: &SupportPack,
    ready: impl FnOnce(&Path) -> bool,
) -> SupportStatus {
    let mgr = torrent_state.0.read().await.get(pack.collection).cloned();
    let Some(mgr) = mgr else {
        return SupportStatus::new("missing", 0.0, 0);
    };
    let ready = ready(&mgr.torrent_root());
    status(pack, &mgr, ready).await
}

/// `ready` is the caller's readiness verdict (Win9x scopes it per variant).
pub(crate) async fn status(pack: &SupportPack, mgr: &DownloadManager, ready: bool) -> SupportStatus {
    if ready {
        return SupportStatus::new("ready", 1.0, 0);
    }
    let Some(util) = mgr.index().find_by_suffix(pack.util_suffix) else {
        return SupportStatus::new("missing", 0.0, 0);
    };
    if pack.failed() {
        return SupportStatus::new("failed", 1.0, util.size);
    }
    if mgr.is_file_selected(util.index).await {
        // Verified bytes from the session, never the on-disk length: the
        // sparse file reaches full size long before the pieces are in.
        let progress = match mgr.file_progress(util.index).await {
            Some(p) if p.finished => 1.0,
            Some(p) => (p.progress as f32).min(0.99),
            None => 0.0,
        };
        return SupportStatus::new("downloading", progress, util.size);
    }
    SupportStatus::new("missing", 0.0, util.size)
}

/// Extract the pack's subtrees from the util zip (blocking). Serialised per
/// pack: the startup rearm and a download-click watcher can both find the
/// zip complete at the same moment.
pub(crate) fn extract(pack: &SupportPack, util_zip: &Path, torrent_root: &Path) -> Result<usize, String> {
    if pack.running.swap(true, Ordering::SeqCst) {
        return Err("extraction already running".to_string());
    }
    let result = do_extract(pack, util_zip, torrent_root);
    pack.running.store(false, Ordering::SeqCst);
    result
}

fn do_extract(pack: &SupportPack, util_zip: &Path, torrent_root: &Path) -> Result<usize, String> {
    // Unique temp names so a leftover from a killed run can't collide.
    let pid = std::process::id();
    let tmp_path = util_zip.with_extension(format!("support_tmp_{pid}"));
    let staging_root = torrent_root.join("eXo").join(format!(".support_staging_{pid}"));

    let result = (|| {
        let file = std::fs::File::open(util_zip).map_err(|e| e.to_string())?;
        let mut outer = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        {
            let mut inner_entry = outer.by_name(pack.inner_zip).map_err(|e| {
                format!("{} not found inside {}: {}", pack.inner_zip, util_zip.display(), e)
            })?;
            let mut tmp = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut inner_entry, &mut tmp).map_err(|e| e.to_string())?;
        }

        let tmp = std::fs::File::open(&tmp_path).map_err(|e| e.to_string())?;
        let mut inner = zip::ZipArchive::new(tmp).map_err(|e| e.to_string())?;
        let mut extracted = 0usize;
        for i in 0..inner.len() {
            let mut entry = inner.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().replace('\\', "/");
            let lower = name.to_ascii_lowercase();
            if !pack.prefixes.iter().any(|p| lower.starts_with(p))
                || name.contains("..")
                || entry.is_dir()
            {
                continue;
            }
            let out_path = staging_root.join(&name);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            extracted += 1;
        }
        if extracted == 0 {
            return Err(format!("no matching entries in {}", pack.inner_zip));
        }

        // Each prefix is replaced whole - stale files must not survive a
        // new build - so a prefix must name what the pack OWNS: `emulators/
        // dosbox/` holds eXoDOS's ece builds and eXoWin9x's x98 side by side,
        // and either pack naming the parent would delete the other.
        let dest_root = torrent_root.join("eXo");
        for prefix in pack.prefixes {
            let Some(rel) = staged_dir(&staging_root, prefix) else { continue };
            let dst = dest_root.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if dst.is_dir() {
                std::fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
            } else if dst.exists() {
                std::fs::remove_file(&dst).map_err(|e| e.to_string())?;
            }
            std::fs::rename(staging_root.join(&rel), &dst)
                .map_err(|e| format!("moving {} into place: {}", rel.display(), e))?;
        }
        if let Some(post) = pack.post {
            post(&dest_root);
        }
        Ok(extracted)
    })();

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_dir_all(&staging_root);
    result
}

/// The staged directory a lowercase prefix names, in its real case.
fn staged_dir(staging_root: &Path, prefix: &str) -> Option<PathBuf> {
    let mut rel = PathBuf::new();
    for want in prefix.trim_end_matches('/').split('/') {
        let found = std::fs::read_dir(staging_root.join(&rel))
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .find(|n| n.to_string_lossy().eq_ignore_ascii_case(want))?;
        rel.push(found);
    }
    Some(rel)
}

/// Queue the util zip if it is not selected yet and arm the watcher. Called
/// from `download_game` when a game needs the pack and it is not on disk.
pub(crate) async fn ensure_queued(pack: &'static SupportPack, mgr: &Arc<DownloadManager>) {
    let Some(util) = mgr.index().find_by_suffix(pack.util_suffix) else {
        log::warn!("{} not found in the {} torrent index", pack.util_suffix, pack.collection);
        return;
    };
    if !mgr.is_file_selected(util.index).await {
        let _ = mgr.download_files(vec![util.index]).await;
        log::info!(
            "Also downloading {} ({}, one-time: {})",
            pack.util_suffix,
            crate::commands::content_packs::format_bytes(util.size),
            pack.label
        );
    }
    // Always re-arm: the zip may have finished in an earlier run with nobody
    // left to extract it.
    spawn_watcher(pack, Arc::clone(mgr), util.index);
}

/// Re-arm the watcher after a restart: one armed by a download click dies
/// with the app, and a zip finishing later would never extract.
/// Remove staging dirs and inner-zip copies a dead process left behind
/// (`.support_staging_<pid>`, `<util>.support_tmp_<pid>`): a hard stop
/// mid-extraction leaves gigabytes nobody would ever delete.
pub(crate) fn sweep_leftovers(torrent_root: &Path) {
    let own = format!("_{}", std::process::id());
    let stale = |name: &str, marker: &str| name.contains(marker) && !name.ends_with(&own);
    let exo = torrent_root.join("eXo");
    for (dir, marker) in [(exo.clone(), ".support_staging_"), (exo.join("util"), ".support_tmp_")] {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !stale(&name, marker) {
                continue;
            }
            let path = e.path();
            let res = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            match res {
                Ok(()) => log::info!("Removed stale support leftover {}", path.display()),
                Err(err) => log::warn!("Could not remove {}: {}", path.display(), err),
            }
        }
    }
}

pub(crate) async fn rearm(pack: &'static SupportPack, mgr: &Arc<DownloadManager>) {
    sweep_leftovers(&mgr.torrent_root());
    if (pack.ready)(&mgr.torrent_root()) {
        return;
    }
    let Some(util) = mgr.index().find_by_suffix(pack.util_suffix) else {
        return;
    };
    let selected = mgr.is_file_selected(util.index).await;
    let on_disk = mgr
        .file_output_path(util.index)
        .and_then(|p| std::fs::metadata(p).ok())
        .is_some_and(|m| m.len() > 0);
    if !selected && !on_disk {
        return; // never requested - nothing to resume
    }
    log::info!(
        "Re-arming {} extraction watcher ({})",
        pack.util_suffix,
        if selected { "still selected" } else { "present on disk" }
    );
    spawn_watcher(pack, Arc::clone(mgr), util.index);
}

/// Watch the util zip until it is complete, then extract. Its own task
/// because the frontend only polls while a GAME download is active, and the
/// util zip routinely finishes long after the game that triggered it.
fn spawn_watcher(pack: &'static SupportPack, mgr: Arc<DownloadManager>, util_index: usize) {
    tauri::async_runtime::spawn(async move {
        pack.failed.store(false, Ordering::SeqCst);
        let torrent_root = mgr.torrent_root();
        let expected_size = mgr.index().files.get(util_index).map(|f| f.size).unwrap_or(0);
        let mut failures = 0u32;
        // 6 h at 10 s per check, for slow swarms.
        for _ in 0..2160 {
            if (pack.ready)(&torrent_root) {
                return; // someone else finished the job
            }
            let Some(zip_path) = mgr.file_output_path(util_index) else {
                return;
            };
            // The session's verdict while it has a handle; the on-disk size
            // only when it has none (a restart without session state). With
            // a handle, full length means nothing - the file is sparse.
            let stats = mgr.file_progress(util_index).await;
            let stats_complete = stats.as_ref().is_some_and(|p| p.finished);
            let disk_complete = stats.is_none()
                && expected_size > 0
                && std::fs::metadata(&zip_path).is_ok_and(|m| m.len() >= expected_size);
            if stats_complete || disk_complete {
                let root = torrent_root.clone();
                let outcome =
                    tauri::async_runtime::spawn_blocking(move || extract(pack, &zip_path, &root)).await;
                match outcome {
                    Ok(Ok(n)) => {
                        log::info!("Extracted {} files from {}", n, pack.util_suffix);
                        return;
                    }
                    Ok(Err(e)) if e == "extraction already running" => return,
                    Ok(Err(e)) if !stats_complete => {
                        // The size fallback fired on a sparse file the session
                        // is still filling: not a failure, just not yet.
                        log::info!("{} not extractable yet ({}); waiting", pack.util_suffix, e);
                    }
                    Ok(Err(e)) => {
                        failures += 1;
                        log::error!(
                            "Failed to extract {} (attempt {}): {}",
                            pack.util_suffix, failures, e
                        );
                        if failures >= 3 {
                            pack.failed.store(true, Ordering::SeqCst);
                            return;
                        }
                    }
                    Err(e) => {
                        log::error!("{} extraction task panicked: {}", pack.util_suffix, e);
                        pack.failed.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
        log::warn!("{} extraction watcher timed out", pack.util_suffix);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn never_ready(_: &Path) -> bool {
        false
    }

    static TEST_PACK: SupportPack = SupportPack {
        collection: "test",
        util_suffix: "util/util.zip",
        inner_zip: "INNER.zip",
        prefixes: &["mt32/", "emulators/foo98/"],
        label: "test payload",
        ready: never_ready,
        post: None,
        running: AtomicBool::new(false),
        failed: AtomicBool::new(false),
    };

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for (name, data) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extracts_prefixes_case_preserved_and_replaces_old_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let inner = zip_bytes(&[
            ("mt32/a.rom", b"rom"),
            ("emulators/Foo98/x.bin", b"bin"),
            ("emulators/audio/skip.dll", b"no"),
            ("readme.txt", b"no"),
        ]);
        let util = root.join("util.zip");
        std::fs::write(&util, zip_bytes(&[("INNER.zip", &inner)])).unwrap();
        let stale = root.join("eXo/mt32/old.rom");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, b"stale").unwrap();

        let n = extract(&TEST_PACK, &util, root).unwrap();

        assert_eq!(n, 2);
        assert_eq!(std::fs::read(root.join("eXo/mt32/a.rom")).unwrap(), b"rom");
        assert_eq!(std::fs::read(root.join("eXo/emulators/Foo98/x.bin")).unwrap(), b"bin");
        assert!(!stale.exists(), "old subtree is replaced, not merged");
        assert!(!root.join("eXo/emulators/audio").exists());
        assert!(!root.join("eXo/readme.txt").exists());
        let leftovers: Vec<_> = std::fs::read_dir(root.join("eXo"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "staging dir removed");
        assert!(!util.with_extension(format!("support_tmp_{}", std::process::id())).exists());
    }

    static SIBLING_PACK: SupportPack = SupportPack {
        collection: "test",
        util_suffix: "util/util.zip",
        inner_zip: "INNER.zip",
        prefixes: &["emulators/dosbox/x98/", "emulators/dosbox/options9x.conf"],
        label: "test payload",
        ready: never_ready,
        post: None,
        running: AtomicBool::new(false),
        failed: AtomicBool::new(false),
    };

    /// eXoDOS's ece builds and eXoWin9x's x98 share `emulators/dosbox/`;
    /// a pack replaces what it names, and a file prefix works like a dir.
    #[test]
    fn a_pack_leaves_its_siblings_under_a_shared_parent_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let inner = zip_bytes(&[
            ("emulators/DOSBox/x98/dosbox-x.exe", b"x98"),
            ("emulators/DOSBox/options9x.conf", b"conf"),
        ]);
        let util = root.join("util.zip");
        std::fs::write(&util, zip_bytes(&[("INNER.zip", &inner)])).unwrap();
        let ece = root.join("eXo/emulators/dosbox/ece4230/DOSBox.exe");
        std::fs::create_dir_all(ece.parent().unwrap()).unwrap();
        std::fs::write(&ece, b"ece").unwrap();
        std::fs::write(root.join("eXo/emulators/dosbox/options9x.conf"), b"old").unwrap();

        extract(&SIBLING_PACK, &util, root).unwrap();

        assert_eq!(std::fs::read(&ece).unwrap(), b"ece", "the other pack's build survives");
        assert_eq!(std::fs::read(root.join("eXo/emulators/dosbox/x98/dosbox-x.exe")).unwrap(), b"x98");
        assert_eq!(std::fs::read(root.join("eXo/emulators/dosbox/options9x.conf")).unwrap(), b"conf");
    }

    #[test]
    fn missing_inner_zip_is_an_error_and_cleans_up() {
        // Own pack: `running` is per pack, and the tests run in parallel.
        static PACK: SupportPack = SupportPack {
            collection: "test",
            util_suffix: "util/util.zip",
            inner_zip: "INNER.zip",
            prefixes: &["mt32/"],
            label: "test payload",
            ready: never_ready,
            post: None,
            running: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        };
        let tmp = tempfile::tempdir().unwrap();
        let util = tmp.path().join("util.zip");
        std::fs::write(&util, zip_bytes(&[("OTHER.zip", b"x")])).unwrap();
        let err = extract(&PACK, &util, tmp.path()).unwrap_err();
        assert!(err.contains("INNER.zip not found"));
        assert!(!PACK.running.load(Ordering::SeqCst));
    }
}
