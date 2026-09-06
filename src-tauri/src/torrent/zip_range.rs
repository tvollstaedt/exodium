//! Read a single entry out of a ZIP without reading the ZIP.
//!
//! eXoDOS keeps each game's extras in one `GameData/<Title>.zip` - manual,
//! video, music, artwork - and those run from 2 MB to 1.1 GB. Pulling a 2.5 MB
//! preview video by downloading the whole archive is what this avoids.
//!
//! ZIP is random-access by design: the central directory sits at the END of the
//! file and every entry is independently decodable. So the read order is
//! tail-first (directory), then one seek to the entry's local header, then only
//! that entry's bytes. Over a torrent `FileStream` those seeks translate into
//! piece requests, so the transfer is bounded by the entry size (rounded to
//! piece boundaries) instead of the archive size.

use anyhow::{bail, Context};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};

/// How much of the tail to pull when looking for the end-of-central-directory
/// record. The EOCD is 22 bytes plus an optional comment of up to 64 KB.
const TAIL_SCAN_BYTES: u64 = 66 * 1024;

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const CENTRAL_FILE_SIGNATURE: u32 = 0x0201_4b50;
const LOCAL_FILE_SIGNATURE: u32 = 0x0403_4b50;
const ZIP64_EOCD_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4b50;
const ZIP64_EXTRA_ID: u16 = 0x0001;
/// A corrupt directory size must not turn into a multi-GB allocation.
const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;

const METHOD_STORE: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub method: u16,
    pub local_header_offset: u64,
}

fn u16_at(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn u64_at(buf: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(b)
}

/// The directory's (entry count, size, offset) from the zip64 EOCD record.
/// The locator sits right before the EOCD and names the record's offset;
/// the record itself can be anywhere, so it is one more seek.
async fn zip64_directory_location<R>(
    reader: &mut R,
    tail: &[u8],
    eocd: usize,
) -> anyhow::Result<(u64, u64, u64)>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    let locator = eocd
        .checked_sub(20)
        .filter(|&i| u32_at(tail, i) == ZIP64_EOCD_LOCATOR_SIGNATURE)
        .context("zip64 sizes in the EOCD but no zip64 locator before it")?;
    let record_offset = u64_at(tail, locator + 8);
    reader.seek(std::io::SeekFrom::Start(record_offset)).await?;
    let mut record = [0u8; 56];
    reader.read_exact(&mut record).await.context("reading zip64 EOCD record")?;
    if u32_at(&record, 0) != ZIP64_EOCD_SIGNATURE {
        bail!("no zip64 EOCD record at offset {}", record_offset);
    }
    Ok((u64_at(&record, 32), u64_at(&record, 40), u64_at(&record, 48)))
}

/// Replace the 0xFFFFFFFF fields of a central entry with the values from its
/// zip64 extra field (0x0001), which carries only the fields that overflowed,
/// in spec order: uncompressed size, compressed size, local header offset.
fn apply_zip64_extra(extra: &[u8], sizes: (u64, u64, u64)) -> (u64, u64, u64) {
    let (mut uncompressed, mut compressed, mut offset) = sizes;
    let mut pos = 0usize;
    while pos + 4 <= extra.len() {
        let id = u16_at(extra, pos);
        let end = pos + 4 + u16_at(extra, pos + 2) as usize;
        if end > extra.len() {
            break;
        }
        if id == ZIP64_EXTRA_ID {
            let mut p = pos + 4;
            for field in [&mut uncompressed, &mut compressed, &mut offset] {
                if *field == 0xFFFF_FFFF && p + 8 <= end {
                    *field = u64_at(extra, p);
                    p += 8;
                }
            }
            break;
        }
        pos = end;
    }
    (uncompressed, compressed, offset)
}

/// Parse the central directory. `file_len` is the archive's total size, which
/// the caller knows from the torrent metadata without touching the file.
pub async fn read_central_directory<R>(reader: &mut R, file_len: u64) -> anyhow::Result<Vec<ZipEntry>>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    // Partially downloaded archives are normal here, so every length is
    // suspect: an EOCD needs 22 bytes and the scan reads 4 at a time.
    if file_len < 22 {
        bail!("archive too small to be a zip ({} bytes)", file_len);
    }
    let tail_len = TAIL_SCAN_BYTES.min(file_len);
    let tail_start = file_len - tail_len;
    reader.seek(std::io::SeekFrom::Start(tail_start)).await?;
    let mut tail = vec![0u8; tail_len as usize];
    reader.read_exact(&mut tail).await.context("reading zip tail")?;

    // Scan backwards - the EOCD is the last such signature in the file.
    let eocd = (0..=tail.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(&tail, i) == EOCD_SIGNATURE)
        .context("no end-of-central-directory record - not a zip?")?;

    let mut entry_count = u16_at(&tail, eocd + 10) as u64;
    let mut cd_size = u32_at(&tail, eocd + 12) as u64;
    let mut cd_offset = u32_at(&tail, eocd + 16) as u64;
    // zip64 (the 36 GB media-pack archives): the real values sit in a
    // separate record.
    if cd_offset == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF || entry_count == 0xFFFF {
        (entry_count, cd_size, cd_offset) = zip64_directory_location(reader, &tail, eocd).await?;
    }
    if cd_size > MAX_DIRECTORY_BYTES {
        bail!("refusing to read a {} byte central directory", cd_size);
    }

    reader.seek(std::io::SeekFrom::Start(cd_offset)).await?;
    let mut cd = vec![0u8; cd_size as usize];
    reader.read_exact(&mut cd).await.context("reading central directory")?;

    let mut entries = Vec::with_capacity(entry_count as usize);
    let mut pos = 0usize;
    while pos + 46 <= cd.len() {
        if u32_at(&cd, pos) != CENTRAL_FILE_SIGNATURE {
            break;
        }
        let method = u16_at(&cd, pos + 10);
        let mut compressed_size = u32_at(&cd, pos + 20) as u64;
        let mut uncompressed_size = u32_at(&cd, pos + 24) as u64;
        let name_len = u16_at(&cd, pos + 28) as usize;
        let extra_len = u16_at(&cd, pos + 30) as usize;
        let comment_len = u16_at(&cd, pos + 32) as usize;
        let mut local_header_offset = u32_at(&cd, pos + 42) as u64;
        let name_start = pos + 46;
        let name_end = name_start + name_len;
        if name_end > cd.len() {
            // Truncated directory - keep what parsed rather than indexing past
            // the buffer, which would panic inside a command.
            break;
        }
        let name = String::from_utf8_lossy(&cd[name_start..name_end]).into_owned();
        if [compressed_size, uncompressed_size, local_header_offset].contains(&0xFFFF_FFFF) {
            let extra_end = (name_end + extra_len).min(cd.len());
            (uncompressed_size, compressed_size, local_header_offset) = apply_zip64_extra(
                &cd[name_end..extra_end],
                (uncompressed_size, compressed_size, local_header_offset),
            );
        }
        entries.push(ZipEntry {
            name,
            compressed_size,
            uncompressed_size,
            method,
            local_header_offset,
        });
        pos = name_start + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// Read and decompress one entry, reporting progress and honouring a stop
/// signal between chunks.
///
/// `on_progress(read, total)` returns false to abort - over a torrent stream a
/// read can block for minutes waiting for pieces, so the caller needs a way out
/// when the user moves on.
pub async fn read_entry_with<R, F>(
    reader: &mut R,
    entry: &ZipEntry,
    mut on_progress: F,
) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + AsyncSeek + Unpin,
    F: FnMut(u64, u64) -> bool,
{
    // 1 MB keeps progress smooth without adding meaningful syscall overhead;
    // the torrent piece size (8 MB) dominates latency anyway.
    const CHUNK: usize = 1024 * 1024;
    // A corrupt (or half-downloaded) directory can carry an absurd size. Check
    // before any I/O so a bad entry costs nothing at all.
    const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
    if entry.compressed_size > MAX_ENTRY_BYTES {
        bail!("refusing to read a {} byte entry", entry.compressed_size);
    }

    let data_offset = entry_data_offset(reader, entry).await?;
    reader.seek(std::io::SeekFrom::Start(data_offset)).await?;

    let total = entry.compressed_size;
    let mut raw = Vec::with_capacity(total as usize);
    let mut buf = vec![0u8; CHUNK];
    while (raw.len() as u64) < total {
        if !on_progress(raw.len() as u64, total) {
            bail!("cancelled");
        }
        let want = CHUNK.min((total - raw.len() as u64) as usize);
        reader
            .read_exact(&mut buf[..want])
            .await
            .context("reading entry data")?;
        raw.extend_from_slice(&buf[..want]);
    }
    on_progress(total, total);

    decompress(entry, raw)
}

/// A window onto `inner` starting at `base`, `len` bytes long, so a STORED
/// zip inside a zip (the media pack's album archives) can be read with the
/// same tail-first parser. Seeks are translated, nothing is buffered.
pub struct OffsetReader<R> {
    inner: R,
    base: u64,
    len: u64,
    pos: u64,
    pending_seek: bool,
}

impl<R: AsyncRead + AsyncSeek + Unpin> OffsetReader<R> {
    pub fn new(inner: R, base: u64, len: u64) -> Self {
        Self { inner, base, len, pos: 0, pending_seek: true }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin> AsyncRead for OffsetReader<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        let this = self.get_mut();
        if this.pending_seek {
            let target = this.base + this.pos;
            std::pin::Pin::new(&mut this.inner).start_seek(std::io::SeekFrom::Start(target))?;
            match std::pin::Pin::new(&mut this.inner).poll_complete(cx) {
                Poll::Ready(Ok(_)) => this.pending_seek = false,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        let remaining = this.len.saturating_sub(this.pos);
        if remaining == 0 {
            return Poll::Ready(Ok(()));
        }
        let want = (buf.remaining() as u64).min(remaining) as usize;
        let got = {
            let mut limited = tokio::io::ReadBuf::new(buf.initialize_unfilled_to(want));
            match std::pin::Pin::new(&mut this.inner).poll_read(cx, &mut limited) {
                Poll::Ready(Ok(())) => limited.filled().len(),
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        };
        buf.advance(got);
        this.pos += got as u64;
        Poll::Ready(Ok(()))
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin> AsyncSeek for OffsetReader<R> {
    fn start_seek(mut self: std::pin::Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        let new_pos = match position {
            std::io::SeekFrom::Start(p) => p as i64,
            std::io::SeekFrom::End(d) => self.len as i64 + d,
            std::io::SeekFrom::Current(d) => self.pos as i64 + d,
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new_pos as u64;
        self.pending_seek = true;
        Ok(())
    }

    fn poll_complete(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<u64>> {
        std::task::Poll::Ready(Ok(self.pos))
    }
}

fn decompress(entry: &ZipEntry, raw: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    match entry.method {
        METHOD_STORE => Ok(raw),
        METHOD_DEFLATE => {
            // Videos are deflate-stored at a ~1.0 ratio (MP4 doesn't compress),
            // so this is a copy with extra steps - but it has to be correct for
            // the manuals and text files in the same archive.
            use std::io::Write;
            let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
            let mut dec = flate2::write::DeflateDecoder::new(&mut out);
            dec.write_all(&raw)?;
            dec.finish().context("inflating entry")?;
            Ok(out)
        }
        other => bail!("unsupported zip compression method {}", other),
    }
}

/// The central directory's name/extra lengths do NOT have to match the local
/// header's, so the data offset has to come from the local header itself.
pub async fn entry_data_offset<R>(reader: &mut R, entry: &ZipEntry) -> anyhow::Result<u64>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    reader
        .seek(std::io::SeekFrom::Start(entry.local_header_offset))
        .await?;
    let mut header = [0u8; 30];
    reader.read_exact(&mut header).await.context("reading local header")?;
    if u32_at(&header, 0) != LOCAL_FILE_SIGNATURE {
        bail!(
            "no local file header at offset {} - the archive is likely only \
             partially downloaded (missing regions read as zeros)",
            entry.local_header_offset
        );
    }
    let name_len = u16_at(&header, 26) as u64;
    let extra_len = u16_at(&header, 28) as u64;
    Ok(entry.local_header_offset + 30 + name_len + extra_len)
}

/// Read an entry without progress reporting.
pub async fn read_entry<R>(reader: &mut R, entry: &ZipEntry) -> anyhow::Result<Vec<u8>>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    read_entry_with(reader, entry, |_, _| true).await
}

/// Video extensions worth offering as a preview. Anything the webview cannot
/// decode is pointless to fetch.
const VIDEO_EXTENSIONS: &[&str] = &[".mp4", ".m4v", ".webm", ".mov"];

/// A preview outside the `Videos/` folder is a bonus extra (a trailer filed
/// with the game's other extras). Cap it: past this size it is not a preview
/// but a feature, and fetching it would defeat the point of streaming.
const MAX_FALLBACK_VIDEO_BYTES: u64 = 64 * 1024 * 1024;

fn is_video(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIDEO_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// The preview video: `Videos/` wins, any other playable file is a
/// size-bounded fallback (one archive keeps its trailer under Extras).
pub fn find_video(entries: &[ZipEntry]) -> Option<&ZipEntry> {
    let preferred = entries
        .iter()
        .filter(|e| {
            let lower = e.name.to_ascii_lowercase();
            lower.starts_with("videos/") && is_video(&lower)
        })
        .max_by_key(|e| e.uncompressed_size);
    if preferred.is_some() {
        return preferred;
    }
    entries
        .iter()
        .filter(|e| is_video(&e.name) && e.uncompressed_size <= MAX_FALLBACK_VIDEO_BYTES)
        .max_by_key(|e| e.uncompressed_size)
}

/// Audio formats the webview decodes. The catalogue also names tracker
/// modules (.mod/.xm/.s3m/.amf/.psm) and .m3u lists for a handful of games;
/// neither plays in an `<audio>` element, so they are simply not offered.
pub const MUSIC_EXTENSIONS: &[&str] = &[".mp3", ".ogg"];

/// A track outside `Music/` is a bonus extra; past this size it is a full
/// soundtrack rip, not a theme.
const MAX_FALLBACK_MUSIC_BYTES: u64 = 32 * 1024 * 1024;

pub fn is_music(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MUSIC_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

/// The theme track: `Music/` wins, anything else is a bounded fallback.
pub fn find_music(entries: &[ZipEntry]) -> Option<&ZipEntry> {
    let preferred = entries
        .iter()
        .filter(|e| {
            let lower = e.name.to_ascii_lowercase();
            lower.starts_with("music/") && is_music(&lower)
        })
        .max_by_key(|e| e.uncompressed_size);
    if preferred.is_some() {
        return preferred;
    }
    entries
        .iter()
        .filter(|e| is_music(&e.name) && e.uncompressed_size <= MAX_FALLBACK_MUSIC_BYTES)
        .max_by_key(|e| e.uncompressed_size)
}

/// Top-level folders of an archive, for diagnosing a "no video here" verdict -
/// otherwise a wrong matcher and an archive that genuinely has none look the
/// same in the log.
pub fn top_level_folders(entries: &[ZipEntry]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for e in entries {
        let top = e.name.split('/').next().unwrap_or("").to_string();
        if !top.is_empty() && !seen.contains(&top) {
            seen.push(top);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Build an archive shaped like a GameData zip: a manual, a "video" and
    /// some filler, so the video is neither first nor last.
    fn make_zip(video_body: &[u8], compress_video: bool) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let deflated: zip::write::FileOptions<'_, ()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            let stored: zip::write::FileOptions<'_, ()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

            zip.start_file("Manuals/MS-DOS/Some Game (1994).pdf", deflated).unwrap();
            zip.write_all(&vec![b'M'; 40_000]).unwrap();

            zip.start_file(
                "Videos/MS-DOS/Some Game (1994).mp4",
                if compress_video { deflated } else { stored },
            ).unwrap();
            zip.write_all(video_body).unwrap();

            zip.start_file("Music/MS-DOS/Some Game (1994)/01.mp3", deflated).unwrap();
            zip.write_all(&vec![b'S'; 10_000]).unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    async fn extract_video(zip_bytes: &[u8]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(zip_bytes.to_vec());
        let entries = read_central_directory(&mut cursor, zip_bytes.len() as u64)
            .await
            .unwrap();
        let video = find_video(&entries).expect("video entry");
        read_entry(&mut cursor, video).await.unwrap()
    }

    #[tokio::test]
    async fn extracts_a_deflated_video() {
        // Pseudo-random so deflate cannot collapse it, like a real MP4.
        let body: Vec<u8> = (0..200_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let zip = make_zip(&body, true);
        assert_eq!(extract_video(&zip).await, body);
    }

    #[tokio::test]
    async fn extracts_a_stored_video() {
        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let zip = make_zip(&body, false);
        assert_eq!(extract_video(&zip).await, body);
    }

    /// The directory listing must not require reading the archive body - that
    /// is what keeps the transfer small over a torrent stream.
    #[tokio::test]
    async fn listing_only_touches_the_tail() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let zip = make_zip(&body, true);
        let entries = {
            let mut cursor = std::io::Cursor::new(zip.clone());
            read_central_directory(&mut cursor, zip.len() as u64).await.unwrap()
        };
        assert_eq!(entries.len(), 3);
        let video = find_video(&entries).unwrap();
        assert!(video.name.starts_with("Videos/"));
        // Video is the largest entry, and it sits in the middle of the archive.
        assert!(video.local_header_offset > 0);
        assert!(video.local_header_offset < zip.len() as u64);
    }

    /// One sampled archive filed its trailer with the game's extras instead of
    /// in `Videos/`, so a folder-only matcher reported "no video" for it.
    #[test]
    fn finds_a_video_filed_outside_the_videos_folder() {
        let entries = vec![
            ZipEntry { name: "Manuals/MS-DOS/Braindead 13 (1995).pdf".into(), compressed_size: 10, uncompressed_size: 10, method: 8, local_header_offset: 0 },
            ZipEntry { name: "eXo/eXoDOS/!dos/BrainDea/Extras/Braindead 13 - Trailer (1995).mp4".into(), compressed_size: 9_000_000, uncompressed_size: 9_700_000, method: 8, local_header_offset: 100 },
        ];
        let found = find_video(&entries).expect("the trailer counts as a preview");
        assert!(found.name.ends_with("Trailer (1995).mp4"));
    }

    #[test]
    fn the_videos_folder_wins_over_a_stray_file() {
        let entries = vec![
            ZipEntry { name: "eXo/eXoDOS/!dos/x/Extras/bonus.mp4".into(), compressed_size: 50_000_000, uncompressed_size: 50_000_000, method: 8, local_header_offset: 0 },
            ZipEntry { name: "Videos/MS-DOS/Game (1995).mp4".into(), compressed_size: 2_000_000, uncompressed_size: 2_000_000, method: 8, local_header_offset: 100 },
        ];
        // Even though the stray file is far larger - the curated one is the preview.
        assert_eq!(find_video(&entries).unwrap().name, "Videos/MS-DOS/Game (1995).mp4");
    }

    /// A game's own full-length FMV is not a preview; fetching it would defeat
    /// the point of streaming a small clip.
    #[test]
    fn an_oversized_stray_video_is_not_offered() {
        let entries = vec![ZipEntry {
            name: "eXo/eXoDOS/!dos/x/movie.mp4".into(),
            compressed_size: 200_000_000,
            uncompressed_size: 200_000_000,
            method: 8,
            local_header_offset: 0,
        }];
        assert!(find_video(&entries).is_none());
    }

    #[test]
    fn formats_the_webview_cannot_play_are_ignored() {
        let entries = vec![ZipEntry {
            name: "Videos/MS-DOS/Game (1995).avi".into(),
            compressed_size: 5_000_000,
            uncompressed_size: 5_000_000,
            method: 8,
            local_header_offset: 0,
        }];
        assert!(find_video(&entries).is_none());
    }

    #[test]
    fn find_music_prefers_the_music_folder() {
        let entries = vec![
            ZipEntry { name: "eXo/eXoDOS/!dos/x/Extras/rip.mp3".into(), compressed_size: 9_000_000, uncompressed_size: 9_000_000, method: 8, local_header_offset: 0 },
            ZipEntry { name: "Music/MS-DOS/Game (1995).mp3".into(), compressed_size: 3_000_000, uncompressed_size: 3_000_000, method: 0, local_header_offset: 100 },
        ];
        assert_eq!(find_music(&entries).unwrap().name, "Music/MS-DOS/Game (1995).mp3");
    }

    /// The catalogue names tracker modules for a few games; the webview cannot
    /// play them, so they must read as "no theme", not as a broken track.
    #[test]
    fn tracker_formats_are_not_offered() {
        for ext in ["mod", "xm", "s3m", "amf", "psm", "m3u"] {
            let entries = vec![ZipEntry {
                name: format!("Music/MS-DOS/Game (1995).{}", ext),
                compressed_size: 100_000,
                uncompressed_size: 100_000,
                method: 8,
                local_header_offset: 0,
            }];
            assert!(find_music(&entries).is_none(), "{} was offered", ext);
        }
    }

    #[test]
    fn find_music_none_when_absent() {
        let entries = vec![ZipEntry {
            name: "Videos/MS-DOS/Game (1995).mp4".into(),
            compressed_size: 100,
            uncompressed_size: 100,
            method: 8,
            local_header_offset: 0,
        }];
        assert!(find_music(&entries).is_none());
    }

    #[tokio::test]
    async fn extracts_a_music_entry() {
        let zip = make_zip(b"video", true);
        let mut cursor = std::io::Cursor::new(zip.clone());
        let entries = read_central_directory(&mut cursor, zip.len() as u64).await.unwrap();
        let music = find_music(&entries).expect("music entry");
        assert!(music.name.starts_with("Music/"));
        let bytes = read_entry(&mut cursor, music).await.unwrap();
        assert_eq!(bytes, vec![b'S'; 10_000]);
    }

    #[tokio::test]
    async fn no_video_entry_is_not_an_error() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file::<_, ()>("Manuals/MS-DOS/x.pdf", Default::default()).unwrap();
            zip.write_all(b"pdf").unwrap();
            zip.finish().unwrap();
        }
        let bytes = buf.into_inner();
        let mut cursor = std::io::Cursor::new(bytes.clone());
        let entries = read_central_directory(&mut cursor, bytes.len() as u64).await.unwrap();
        assert!(find_video(&entries).is_none());
    }

    /// Half-downloaded archives are the norm on disk, so malformed input must
    /// produce errors, never panics inside a command.
    #[tokio::test]
    async fn truncated_input_errors_instead_of_panicking() {
        for len in [0usize, 3, 21] {
            let data = vec![0u8; len];
            let mut cursor = std::io::Cursor::new(data);
            assert!(read_central_directory(&mut cursor, len as u64).await.is_err(), "len {}", len);
        }
    }

    #[tokio::test]
    async fn a_truncated_central_directory_keeps_what_parsed() {
        let body: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let zip = make_zip(&body, true);
        let mut cursor = std::io::Cursor::new(zip.clone());
        let entries = read_central_directory(&mut cursor, zip.len() as u64).await.unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn an_absurd_entry_size_is_refused() {
        let entry = ZipEntry {
            name: "Videos/MS-DOS/x.mp4".into(),
            compressed_size: 8 * 1024 * 1024 * 1024,
            uncompressed_size: 8 * 1024 * 1024 * 1024,
            method: 8,
            local_header_offset: 0,
        };
        let mut cursor = std::io::Cursor::new(vec![0u8; 64]);
        let err = read_entry(&mut cursor, &entry).await.unwrap_err();
        assert!(err.to_string().contains("refusing"), "got: {}", err);
    }

    #[tokio::test]
    async fn a_non_zip_reports_why() {
        let junk = vec![0u8; 1000];
        let mut cursor = std::io::Cursor::new(junk.clone());
        let err = read_central_directory(&mut cursor, junk.len() as u64).await.unwrap_err();
        assert!(err.to_string().contains("end-of-central-directory"));
    }

    /// Same opt-in check for the theme tracks: every archive that lists one
    /// must yield it, and the bytes must be what the extension promises.
    ///   EXODIUM_GAMEDATA_DIR=/path/to/GameData/eXoDOS cargo test real_gamedata_music -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn real_gamedata_music() {
        let Ok(dir) = std::env::var("EXODIUM_GAMEDATA_DIR") else {
            eprintln!("set EXODIUM_GAMEDATA_DIR to run this");
            return;
        };
        let (mut listed, mut read, mut no_music, mut partial) = (0, 0, 0, 0);
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("zip") {
                continue;
            }
            let len = entry.metadata().unwrap().len();
            let mut file = tokio::fs::File::open(&path).await.unwrap();
            let entries = match read_central_directory(&mut file, len).await {
                Ok(e) if !e.is_empty() => e,
                _ => continue,
            };
            listed += 1;
            let Some(music) = find_music(&entries) else { no_music += 1; continue };
            match read_entry(&mut file, music).await {
                Ok(bytes) => {
                    assert_eq!(bytes.len() as u64, music.uncompressed_size, "{}", path.display());
                    let lower = music.name.to_ascii_lowercase();
                    if lower.ends_with(".ogg") {
                        assert_eq!(&bytes[..4], b"OggS", "not an ogg: {}", music.name);
                    } else {
                        // ID3v2 tag or a bare MPEG frame sync.
                        assert!(&bytes[..3] == b"ID3" || (bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0), "not an mp3: {}", music.name);
                    }
                    read += 1;
                    eprintln!("{} <- {} ({:.1} MB)", path.file_name().unwrap().to_string_lossy(), music.name, bytes.len() as f64 / 1048576.0);
                }
                Err(e) => {
                    assert!(e.to_string().contains("partially downloaded"), "unexpected failure on {}: {}", path.display(), e);
                    partial += 1;
                }
            }
        }
        eprintln!("\n{} archives listed, {} themes extracted, {} without theme, {} partially downloaded\n", listed, read, no_music, partial);
        assert!(read > 0, "no theme could be read from {}", dir);
    }

    /// Opt-in check against real eXoDOS GameData archives:
    ///   EXODIUM_GAMEDATA_DIR=/path/to/GameData/eXoDOS cargo test real_gamedata -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn real_gamedata_zips() {
        let Ok(dir) = std::env::var("EXODIUM_GAMEDATA_DIR") else {
            eprintln!("set EXODIUM_GAMEDATA_DIR to run this");
            return;
        };
        let (mut listed, mut read, mut no_video, mut partial, mut unreadable) = (0, 0, 0, 0, 0);
        let mut total_zip = 0u64;
        let mut total_video = 0u64;

        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("zip") {
                continue;
            }
            let len = entry.metadata().unwrap().len();
            let mut file = tokio::fs::File::open(&path).await.unwrap();
            let dir_entries = match read_central_directory(&mut file, len).await {
                Ok(e) if !e.is_empty() => e,
                // A GameData zip whose tail was never fetched is a normal state
                // on disk, not a parser bug.
                _ => { unreadable += 1; continue }
            };
            listed += 1;
            let Some(video) = find_video(&dir_entries) else { no_video += 1; continue };
            match read_entry(&mut file, video).await {
                Ok(bytes) => {
                    assert_eq!(bytes.len() as u64, video.uncompressed_size, "{}", path.display());
                    // Every MP4 carries an ftyp box right after the size field.
                    assert_eq!(&bytes[4..8], b"ftyp", "not an mp4: {}", video.name);
                    read += 1;
                    total_zip += len;
                    total_video += video.uncompressed_size;
                }
                Err(e) => {
                    assert!(e.to_string().contains("partially downloaded"),
                            "unexpected failure on {}: {}", path.display(), e);
                    partial += 1;
                }
            }
        }
        eprintln!(
            "\n{} archives listed, {} videos extracted, {} without video, {} partially downloaded, {} unreadable\n  \
             transferred {:.1} MB instead of {:.1} MB ({:.1}%)\n",
            listed, read, no_video, partial, unreadable,
            total_video as f64 / 1048576.0, total_zip as f64 / 1048576.0,
            100.0 * total_video as f64 / total_zip.max(1) as f64,
        );
        assert!(read > 0, "no video could be read from {}", dir);
    }

    // ── zip64 ────────────────────────────────────────────────────────────────

    /// Rewrite a plain archive's tail into the zip64 shape: the directory is
    /// left alone, a zip64 EOCD record and locator go after it, and the EOCD
    /// carries the 0xFFFF... sentinels. `with_locator: false` drops the
    /// locator, the shape of a truncated or corrupt archive.
    fn to_zip64_tail(zip: &[u8], with_locator: bool) -> Vec<u8> {
        let eocd = zip.len() - 22;
        assert_eq!(u32_at(zip, eocd), EOCD_SIGNATURE);
        let count = u16_at(zip, eocd + 10) as u64;
        let cd_size = u32_at(zip, eocd + 12) as u64;
        let cd_offset = u32_at(zip, eocd + 16) as u64;
        let record_offset = cd_offset + cd_size;

        let mut out = zip[..record_offset as usize].to_vec();
        out.extend_from_slice(&ZIP64_EOCD_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&44u64.to_le_bytes()); // size of the rest
        out.extend_from_slice(&[45, 0, 45, 0]); // version made by / needed
        out.extend_from_slice(&0u32.to_le_bytes()); // this disk
        out.extend_from_slice(&0u32.to_le_bytes()); // directory disk
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        if with_locator {
            out.extend_from_slice(&ZIP64_EOCD_LOCATOR_SIGNATURE.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&record_offset.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out.extend_from_slice(&EOCD_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&[0u8; 6]); // disks, disk of directory
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length
        out
    }

    /// Move one central entry's local header offset into a zip64 extra field,
    /// the way a writer records an entry that starts beyond 4 GB.
    fn offset_into_zip64_extra(zip: &[u8], name: &str) -> Vec<u8> {
        let eocd = zip.len() - 22;
        let cd_size = u32_at(zip, eocd + 12) as usize;
        let cd_offset = u32_at(zip, eocd + 16) as usize;
        let cd = &zip[cd_offset..cd_offset + cd_size];

        let mut new_cd = Vec::new();
        let mut pos = 0;
        while pos + 46 <= cd.len() {
            let name_len = u16_at(cd, pos + 28) as usize;
            let extra_len = u16_at(cd, pos + 30) as usize;
            let comment_len = u16_at(cd, pos + 32) as usize;
            let end = pos + 46 + name_len + extra_len + comment_len;
            let entry_name = &cd[pos + 46..pos + 46 + name_len];
            if entry_name == name.as_bytes() {
                let offset = u32_at(cd, pos + 42) as u64;
                let mut e = cd[pos..pos + 46 + name_len].to_vec();
                e[42..46].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
                let extra_total = extra_len + 12;
                e[30..32].copy_from_slice(&(extra_total as u16).to_le_bytes());
                e.extend_from_slice(&cd[pos + 46 + name_len..pos + 46 + name_len + extra_len]);
                e.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
                e.extend_from_slice(&8u16.to_le_bytes());
                e.extend_from_slice(&offset.to_le_bytes());
                e.extend_from_slice(&cd[pos + 46 + name_len + extra_len..end]);
                new_cd.extend_from_slice(&e);
            } else {
                new_cd.extend_from_slice(&cd[pos..end]);
            }
            pos = end;
        }

        let mut out = zip[..cd_offset].to_vec();
        out.extend_from_slice(&new_cd);
        let mut eocd_rec = zip[eocd..].to_vec();
        eocd_rec[12..16].copy_from_slice(&(new_cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&eocd_rec);
        out
    }

    #[tokio::test]
    async fn zip64_directory_is_read_through_the_locator() {
        let body: Vec<u8> = (0..60_000u32).map(|i| (i % 241) as u8).collect();
        let zip = to_zip64_tail(&make_zip(&body, false), true);
        let mut cursor = std::io::Cursor::new(zip.clone());
        let entries = read_central_directory(&mut cursor, zip.len() as u64).await.unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(extract_video(&zip).await, body);
    }

    #[tokio::test]
    async fn zip64_extra_field_supplies_the_local_header_offset() {
        let body: Vec<u8> = (0..30_000u32).map(|i| (i % 239) as u8).collect();
        let plain = make_zip(&body, false);
        let zip = offset_into_zip64_extra(&plain, "Videos/MS-DOS/Some Game (1994).mp4");
        let expected = {
            let mut c = std::io::Cursor::new(plain.clone());
            let e = read_central_directory(&mut c, plain.len() as u64).await.unwrap();
            find_video(&e).unwrap().local_header_offset
        };
        let mut cursor = std::io::Cursor::new(zip.clone());
        let entries = read_central_directory(&mut cursor, zip.len() as u64).await.unwrap();
        assert_eq!(find_video(&entries).unwrap().local_header_offset, expected);
        assert_eq!(extract_video(&zip).await, body);
    }

    #[tokio::test]
    async fn zip64_sentinels_without_a_locator_are_an_error() {
        let zip = to_zip64_tail(&make_zip(b"video", false), false);
        let mut cursor = std::io::Cursor::new(zip.clone());
        let err = read_central_directory(&mut cursor, zip.len() as u64).await.unwrap_err();
        assert!(err.to_string().contains("zip64"), "{err}");
    }

    /// The media pack stores whole album zips inside its 36 GB archive; a
    /// window onto the stored entry makes the inner directory readable with
    /// the same parser, and a track comes out without touching the rest.
    #[tokio::test]
    async fn nested_stored_zip_is_readable_through_a_window() {
        let body: Vec<u8> = (0..40_000u32).map(|i| (i % 233) as u8).collect();
        let inner_zip = make_zip(&body, true);
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let stored: zip::write::FileOptions<'_, ()> =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Images/cover.jpg", stored).unwrap();
            zip.write_all(&[0xFFu8; 5_000]).unwrap();
            zip.start_file("eXo/Soundtracks/Some Game.zip", stored).unwrap();
            zip.write_all(&inner_zip).unwrap();
            zip.finish().unwrap();
        }
        let outer = buf.into_inner();
        let mut cursor = std::io::Cursor::new(outer.clone());
        let entries = read_central_directory(&mut cursor, outer.len() as u64).await.unwrap();
        let album = entries.iter().find(|e| e.name.ends_with(".zip")).unwrap();
        assert_eq!(album.method, METHOD_STORE);
        let base = entry_data_offset(&mut cursor, album).await.unwrap();
        let mut window = OffsetReader::new(cursor, base, album.uncompressed_size);
        let tracks = read_central_directory(&mut window, album.uncompressed_size).await.unwrap();
        assert_eq!(tracks.len(), 3);
        let video = find_video(&tracks).unwrap();
        assert_eq!(read_entry(&mut window, video).await.unwrap(), body);
        // A window never reads past its end.
        window.seek(std::io::SeekFrom::Start(album.uncompressed_size - 4)).await.unwrap();
        let mut tail = Vec::new();
        window.read_to_end(&mut tail).await.unwrap();
        assert_eq!(tail.len(), 4);
    }
}
