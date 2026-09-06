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
        for tr in tracks.iter().take(30) {
            println!("   {} ({:.1} MB, method {})", tr.name, tr.uncompressed_size as f64 / 1e6, tr.method);
        }
        if let Some(first) = tracks.iter().find(|e| e.uncompressed_size > 0 && !e.name.ends_with('/')) {
            let t = Instant::now();
            let s0 = mgr.session_transfer();
            let bytes = zip_range::read_entry_with(&mut inner, first, |_, _| true).await?;
            let target = out.with_extension("track");
            std::fs::write(&target, &bytes)?;
            println!("track {} ({} bytes) in {:.1}s, session peers={} -> {}", first.name, bytes.len(), t.elapsed().as_secs_f64(), s0.peers, target.display());
        }
        stream = inner.into_inner();
    }
    drop(stream);
    mgr.shutdown_session().await;
    Ok(())
}
