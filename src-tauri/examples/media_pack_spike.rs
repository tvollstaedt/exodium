//! Read the central directory of one file inside a torrent without downloading
//! the file (#27). Adds the torrent with an empty selection and streams the
//! archive's tail through `zip_range`.
//!
//!   cargo run --example media_pack_spike -- <file.torrent> <data_dir> <path suffix> <out.jsonl> [timeout secs]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use exodium_lib::torrent::manager::DownloadManager;
use exodium_lib::torrent::zip_range;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: media_pack_spike <file.torrent> <data_dir> <path suffix> <out.jsonl> [timeout secs]");
        std::process::exit(2);
    }
    let torrent = PathBuf::from(&args[1]);
    let data_dir = PathBuf::from(&args[2]);
    let suffix = args[3].clone();
    let out = PathBuf::from(&args[4]);
    let timeout = Duration::from_secs(args.get(5).and_then(|s| s.parse().ok()).unwrap_or(600));

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(&torrent, &data_dir, &suffix, &out, timeout))
}

async fn run(torrent: &Path, data_dir: &Path, suffix: &str, out: &Path, timeout: Duration) -> anyhow::Result<()> {
    let session_dir = data_dir.join("session");
    let persistence = session_dir.join("fastresume");
    std::fs::create_dir_all(&persistence)?;
    // Empty bitfield, like the app's fastresume seed: without it librqbit
    // hash-checks 220 GB of placeholders before the torrent goes live.
    let parsed = lava_torrent::torrent::v1::Torrent::read_from_file(torrent)?;
    let bitv = persistence.join(format!("{}.bitv", parsed.info_hash()));
    if !bitv.exists() {
        std::fs::write(&bitv, vec![0u8; parsed.pieces.len().div_ceil(8)])?;
    }
    let session = DownloadManager::create_session(&session_dir, &persistence).await?;
    let mgr = Arc::new(DownloadManager::new_with_session(session, torrent, data_dir, &persistence)?);
    let entry = mgr
        .index()
        .find_by_suffix(suffix)
        .ok_or_else(|| anyhow::anyhow!("no file ending in {suffix} in the torrent"))?
        .clone();
    println!("file #{}: {} ({:.1} GB), piece {} MiB", entry.index, entry.path, entry.size as f64 / 1e9, mgr.index().piece_length / (1024 * 1024));

    let started = Instant::now();
    let mut stream = loop {
        match mgr.stream_file(entry.index).await {
            Ok(s) => break s,
            Err(e) if e.to_string().contains("initializing") && started.elapsed() < timeout => {
                println!("  torrent still initializing ({:.0}s)", started.elapsed().as_secs_f64());
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(e) => return Err(e),
        }
    };
    println!("stream open after {:.1}s", started.elapsed().as_secs_f64());

    let stats = {
        let mgr = Arc::clone(&mgr);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let s = mgr.session_transfer();
                println!("  peers={} down={} KB/s", s.peers, s.download_bps / 1024);
            }
        })
    };
    let entries = tokio::time::timeout(timeout, zip_range::read_central_directory(&mut stream, entry.size))
        .await
        .map_err(|_| anyhow::anyhow!("directory read timed out after {:?}", timeout))??;
    stats.abort();
    println!("central directory: {} entries after {:.1}s", entries.len(), started.elapsed().as_secs_f64());

    let mut f = std::fs::File::create(out)?;
    use std::io::Write;
    for e in &entries {
        writeln!(
            f,
            "{{\"name\":{},\"compressed\":{},\"uncompressed\":{},\"method\":{},\"offset\":{}}}",
            serde_json::to_string(&e.name)?, e.compressed_size, e.uncompressed_size, e.method, e.local_header_offset
        )?;
    }
    println!("wrote {}", out.display());
    // EXTRACT=<entry name>: also pull one entry out (a small XML, one piece).
    if let Ok(name) = std::env::var("EXTRACT") {
        let e = entries.iter().find(|e| e.name == name).ok_or_else(|| anyhow::anyhow!("no entry {name}"))?;
        let t = Instant::now();
        let bytes = zip_range::read_entry_with(&mut stream, e, |_, _| true).await?;
        let target = out.with_extension("extract");
        std::fs::write(&target, &bytes)?;
        println!("extracted {} ({} bytes) in {:.1}s -> {}", name, bytes.len(), t.elapsed().as_secs_f64(), target.display());
    }
    // EXTRACT_PREFIX=<prefix> EXTRACT_OUT=<dir>: pull every entry under a
    // prefix in ONE forward pass (entries of a subtree are contiguous, so
    // offset order turns thousands of seeks into a sequential read). Dev tool:
    // this is how the bundled Lesesaal covers are generated. NOT `OUT_DIR` -
    // cargo sets that one itself and wins.
    if let (Ok(prefix), Ok(out_dir)) = (std::env::var("EXTRACT_PREFIX"), std::env::var("EXTRACT_OUT")) {
        let dir = PathBuf::from(&out_dir);
        std::fs::create_dir_all(&dir)?;
        let mut wanted: Vec<&zip_range::ZipEntry> = entries
            .iter()
            .filter(|e| e.name.starts_with(&prefix) && e.compressed_size > 0 && !e.name.ends_with('/'))
            .collect();
        wanted.sort_by_key(|e| e.local_header_offset);
        let total: u64 = wanted.iter().map(|e| e.compressed_size).sum();
        let count = wanted.len();
        println!("extracting {} entries ({:.0} MB) under {}", count, total as f64 / 1e6, prefix);
        let started = Instant::now();
        let (mut done, mut bytes) = (0usize, 0u64);
        for entry in wanted {
            let name = entry.name.rsplit('/').next().unwrap_or(&entry.name);
            let target = dir.join(name);
            if target.exists() {
                done += 1;
                continue;
            }
            match zip_range::read_entry_with(&mut stream, entry, |_, _| true).await {
                Ok(data) => {
                    std::fs::write(&target, &data)?;
                    bytes += data.len() as u64;
                    done += 1;
                }
                Err(e) => println!("  skip {}: {}", entry.name, e),
            }
            if done % 25 == 0 {
                println!(
                    "  {}/{} files, {:.0} MB in {:.0}s ({:.0} KB/s)",
                    done, count, bytes as f64 / 1e6, started.elapsed().as_secs_f64(),
                    bytes as f64 / 1024.0 / started.elapsed().as_secs_f64().max(1.0)
                );
            }
        }
        println!("extracted {} files in {:.0}s", done, started.elapsed().as_secs_f64());
    }
    // NESTED=<album zip entry>: read the album's own directory through a
    // window onto the stored entry and pull its first track.
    if let Ok(name) = std::env::var("NESTED") {
        let album = entries.iter().find(|e| e.name == name).ok_or_else(|| anyhow::anyhow!("no entry {name}"))?;
        anyhow::ensure!(album.method == 0, "album is not stored (method {})", album.method);
        let t = Instant::now();
        let base = zip_range::entry_data_offset(&mut stream, album).await?;
        let mut inner = zip_range::OffsetReader::new(stream, base, album.uncompressed_size);
        let tracks = zip_range::read_central_directory(&mut inner, album.uncompressed_size).await?;
        println!("album {}: {} entries after {:.1}s", name, tracks.len(), t.elapsed().as_secs_f64());
        // The inner directory is the interesting listing for a wrapper zip
        // (the GLP magazine add-on is one STORED archive inside an installer).
        let nested_out = out.with_extension("nested.jsonl");
        let mut nf = std::fs::File::create(&nested_out)?;
        for e in &tracks {
            writeln!(
                nf,
                "{{\"name\":{},\"compressed\":{},\"uncompressed\":{},\"method\":{},\"offset\":{}}}",
                serde_json::to_string(&e.name)?, e.compressed_size, e.uncompressed_size, e.method, e.local_header_offset
            )?;
        }
        println!("wrote {}", nested_out.display());
        for tr in tracks.iter().take(30) {
            println!("   {} ({:.1} MB, method {})", tr.name, tr.uncompressed_size as f64 / 1e6, tr.method);
        }
        // NESTED_EXTRACT=<inner entry>: pull one named entry out of the inner
        // archive instead of its first file.
        let wanted = std::env::var("NESTED_EXTRACT").ok();
        if let Some(first) = tracks.iter().find(|e| {
            e.uncompressed_size > 0 && !e.name.ends_with('/') && wanted.as_deref().is_none_or(|w| e.name == w)
        }) {
            let t = Instant::now();
            let s0 = mgr.session_transfer();
            let bytes = zip_range::read_entry_with(&mut inner, first, |_, _| true).await?;
            let target = out.with_extension("track");
            std::fs::write(&target, &bytes)?;
            println!("track {} ({} bytes) in {:.1}s, session peers={} -> {}", first.name, bytes.len(), t.elapsed().as_secs_f64(), s0.peers, target.display());
        }
        // NESTED_EXTRACT_PREFIX=<p1,p2,..> NESTED_OUT=<dir>: every inner entry
        // under the prefixes in one forward pass, full paths under the dir.
        if let (Ok(prefixes), Ok(dest)) = (std::env::var("NESTED_EXTRACT_PREFIX"), std::env::var("NESTED_OUT")) {
            let prefixes: Vec<&str> = prefixes.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
            let dest = PathBuf::from(dest);
            let t = Instant::now();
            let written = zip_range::read_subtree_to_dir(&mut inner, &tracks, &prefixes, &[], &dest, |_, _| true).await?;
            println!("nested subtree: {:.1} MB in {:.0}s -> {}", written as f64 / 1e6, t.elapsed().as_secs_f64(), dest.display());
        }
        stream = inner.into_inner();
    }
    drop(stream);
    mgr.shutdown_session().await;
    Ok(())
}
