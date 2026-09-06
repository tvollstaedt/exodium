//! Minimal VHD (Virtual Hard Disk) writer: differencing children.
//!
//! eXoWin9x's 86Box games recreate a differencing child of a shared parent
//! OS image on EVERY launch (eXo ships a 4.6 KB Windows-only makevhd.exe for
//! this). A differencing VHD holds no data of its own - it is a footer, a
//! dynamic header pointing at the parent, an all-ones Block Allocation Table
//! and two parent locators. Layout mirrors what eXo's tool produces (verified
//! against a real child: empty parent-name field, W2ku absolute + W2ru
//! relative locators, both UTF-16LE).
//!
//! Spec: "Virtual Hard Disk Image Format Specification" v1.0 (Microsoft).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Seconds between the Unix epoch and the VHD epoch (2000-01-01 00:00 UTC).
const VHD_EPOCH_OFFSET: u64 = 946_684_800;

const FOOTER_SIZE: usize = 512;
const DYN_HEADER_SIZE: usize = 1024;

/// One's-complement checksum over a buffer with its checksum field zeroed.
fn checksum(buf: &[u8]) -> u32 {
    !buf.iter().map(|&b| b as u32).sum::<u32>()
}

/// Fields read from a parent VHD needed to author a child.
struct ParentInfo {
    original_size: u64,
    current_size: u64,
    geometry: [u8; 4],
    block_size: u32,
    uuid: [u8; 16],
}

fn read_parent_info(parent: &Path) -> Result<ParentInfo, String> {
    let mut f = std::fs::File::open(parent).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    if len < FOOTER_SIZE as u64 {
        return Err(format!("{} is too small to be a VHD", parent.display()));
    }
    let mut footer = [0u8; FOOTER_SIZE];
    f.seek(SeekFrom::End(-(FOOTER_SIZE as i64))).map_err(|e| e.to_string())?;
    f.read_exact(&mut footer).map_err(|e| e.to_string())?;
    if &footer[0..8] != b"conectix" {
        return Err(format!("{} has no VHD footer", parent.display()));
    }
    let disk_type = u32::from_be_bytes(footer[0x3C..0x40].try_into().unwrap());
    // Parents are dynamic (3) in this pack; fixed (2) would also be legal.
    if disk_type == 4 {
        return Err(format!(
            "{} is itself a differencing image - refusing to chain",
            parent.display()
        ));
    }

    // Block size lives in the dynamic header (dynamic parents only).
    let block_size = if disk_type == 3 {
        let data_offset = u64::from_be_bytes(footer[0x10..0x18].try_into().unwrap());
        let mut dh = [0u8; DYN_HEADER_SIZE];
        f.seek(SeekFrom::Start(data_offset)).map_err(|e| e.to_string())?;
        f.read_exact(&mut dh).map_err(|e| e.to_string())?;
        if &dh[0..8] != b"cxsparse" {
            return Err(format!("{}: bad dynamic header", parent.display()));
        }
        u32::from_be_bytes(dh[0x20..0x24].try_into().unwrap())
    } else {
        0x0020_0000 // 2 MB default
    };

    Ok(ParentInfo {
        original_size: u64::from_be_bytes(footer[0x28..0x30].try_into().unwrap()),
        current_size: u64::from_be_bytes(footer[0x30..0x38].try_into().unwrap()),
        geometry: footer[0x38..0x3C].try_into().unwrap(),
        uuid: footer[0x44..0x54].try_into().unwrap(),
        block_size,
    })
}

/// A path-derived pseudo-UUID: only "differs from the parent" is load-bearing.
fn child_uuid(child: &Path) -> [u8; 16] {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a
    for b in child.to_string_lossy().as_bytes() {
        h1 ^= *b as u64;
        h1 = h1.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let h2 = h1.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(31);
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(&h1.to_be_bytes());
    uuid[8..].copy_from_slice(&h2.to_be_bytes());
    // RFC 4122 version/variant bits so tools treating it as a real UUID
    // don't choke.
    uuid[6] = (uuid[6] & 0x0F) | 0x40;
    uuid[8] = (uuid[8] & 0x3F) | 0x80;
    uuid
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// Create a differencing VHD at `child` whose parent is `parent`.
/// `parent_rel` is the Windows-style relative locator recorded as W2ru
/// (e.g. `.\parent\W98-P.vhd`) - emulators resolve it against the child's
/// own directory, which keeps the extracted tree relocatable.
pub fn create_differencing(child: &Path, parent: &Path, parent_rel: &str) -> Result<(), String> {
    let info = read_parent_info(parent)?;

    let table_entries = info.current_size.div_ceil(info.block_size as u64) as u32;
    let bat_bytes = (table_entries as usize * 4).div_ceil(512) * 512;

    // Layout: [footer copy][dynamic header][BAT][W2ku sector][W2ru sector][footer]
    let bat_offset = (FOOTER_SIZE + DYN_HEADER_SIZE) as u64;
    let w2ku_offset = bat_offset + bat_bytes as u64;
    let w2ru_offset = w2ku_offset + 512;
    let footer_offset = w2ru_offset + 512;

    // Forward slashes: minivhd resolves locators in the child dir's own path
    // style, where a backslash is a filename character on POSIX. Windows
    // accepts '/' too.
    let parent_abs = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    let w2ku_data = utf16le(&parent_abs);
    let w2ru_data = utf16le(&parent_rel.replace('\\', "/"));
    if w2ku_data.len() > 512 || w2ru_data.len() > 512 {
        return Err("parent path too long for a locator sector".to_string());
    }

    // ── Footer ──
    let mut footer = [0u8; FOOTER_SIZE];
    footer[0..8].copy_from_slice(b"conectix");
    footer[0x08..0x0C].copy_from_slice(&2u32.to_be_bytes()); // features: reserved bit
    footer[0x0C..0x10].copy_from_slice(&0x0001_0000u32.to_be_bytes()); // version 1.0
    footer[0x10..0x18].copy_from_slice(&512u64.to_be_bytes()); // data offset → dyn header
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(VHD_EPOCH_OFFSET) as u32)
        .unwrap_or(0);
    footer[0x18..0x1C].copy_from_slice(&now.to_be_bytes());
    footer[0x1C..0x20].copy_from_slice(b"exod"); // creator app
    footer[0x20..0x24].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    footer[0x24..0x28].copy_from_slice(b"Wi2k");
    footer[0x28..0x30].copy_from_slice(&info.original_size.to_be_bytes());
    footer[0x30..0x38].copy_from_slice(&info.current_size.to_be_bytes());
    footer[0x38..0x3C].copy_from_slice(&info.geometry);
    footer[0x3C..0x40].copy_from_slice(&4u32.to_be_bytes()); // differencing
    footer[0x44..0x54].copy_from_slice(&child_uuid(child));
    let cs = checksum(&footer);
    footer[0x40..0x44].copy_from_slice(&cs.to_be_bytes());

    // ── Dynamic header ──
    let mut dh = [0u8; DYN_HEADER_SIZE];
    dh[0..8].copy_from_slice(b"cxsparse");
    dh[0x08..0x10].copy_from_slice(&u64::MAX.to_be_bytes()); // data offset: unused
    dh[0x10..0x18].copy_from_slice(&bat_offset.to_be_bytes());
    dh[0x18..0x1C].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    dh[0x1C..0x20].copy_from_slice(&table_entries.to_be_bytes());
    dh[0x20..0x24].copy_from_slice(&info.block_size.to_be_bytes());
    dh[0x28..0x38].copy_from_slice(&info.uuid); // parent UUID
    // Parent timestamp 0 like makevhd: extraction rewrites mtimes, and a
    // real value would read as "parent modified".
    dh[0x38..0x3C].copy_from_slice(&0u32.to_be_bytes());
    // Parent name (0x40, UTF-16BE) must not be empty: minivhd's fallback
    // probe joins <child dir> + name, and an empty name opens the directory.
    let parent_name: Vec<u8> = parent
        .file_name()
        .map(|n| n.to_string_lossy().encode_utf16().flat_map(|u| u.to_be_bytes()).collect())
        .unwrap_or_default();
    if parent_name.len() > 512 {
        return Err("parent file name too long for the sparse header".to_string());
    }
    dh[0x40..0x40 + parent_name.len()].copy_from_slice(&parent_name);
    // Parent locator entries start at 0x240: {code, data space, data length,
    // reserved, data offset}.
    let locators: [(&[u8; 4], usize, u64); 2] = [
        (b"W2ku", w2ku_data.len(), w2ku_offset),
        (b"W2ru", w2ru_data.len(), w2ru_offset),
    ];
    for (i, (code, len, off)) in locators.iter().enumerate() {
        let e = 0x240 + i * 24;
        dh[e..e + 4].copy_from_slice(*code);
        dh[e + 4..e + 8].copy_from_slice(&512u32.to_be_bytes());
        dh[e + 8..e + 12].copy_from_slice(&(*len as u32).to_be_bytes());
        dh[e + 16..e + 24].copy_from_slice(&off.to_be_bytes());
    }
    let cs = checksum(&dh);
    dh[0x24..0x28].copy_from_slice(&cs.to_be_bytes());

    // ── Write the file ──
    let mut out = std::fs::File::create(child).map_err(|e| e.to_string())?;
    out.write_all(&footer).map_err(|e| e.to_string())?;
    out.write_all(&dh).map_err(|e| e.to_string())?;
    out.write_all(&vec![0xFFu8; bat_bytes]).map_err(|e| e.to_string())?;
    let mut sector = [0u8; 512];
    sector[..w2ku_data.len()].copy_from_slice(&w2ku_data);
    out.write_all(&sector).map_err(|e| e.to_string())?;
    let mut sector = [0u8; 512];
    sector[..w2ru_data.len()].copy_from_slice(&w2ru_data);
    out.write_all(&sector).map_err(|e| e.to_string())?;
    out.write_all(&footer).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    debug_assert_eq!(footer_offset, FOOTER_SIZE as u64 + DYN_HEADER_SIZE as u64 + bat_bytes as u64 + 1024);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Write a minimal dynamic VHD (empty, 64 MB virtual) to serve as parent.
    fn write_dynamic_parent(path: &Path, virtual_size: u64) {
        let block_size: u32 = 0x0020_0000;
        let table_entries = virtual_size.div_ceil(block_size as u64) as u32;
        let bat_bytes = (table_entries as usize * 4).div_ceil(512) * 512;

        let mut footer = [0u8; 512];
        footer[0..8].copy_from_slice(b"conectix");
        footer[0x08..0x0C].copy_from_slice(&2u32.to_be_bytes());
        footer[0x0C..0x10].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        footer[0x10..0x18].copy_from_slice(&512u64.to_be_bytes());
        footer[0x28..0x30].copy_from_slice(&virtual_size.to_be_bytes());
        footer[0x30..0x38].copy_from_slice(&virtual_size.to_be_bytes());
        footer[0x38..0x3C].copy_from_slice(&[0x03, 0xF1, 0x10, 0x3F]);
        footer[0x3C..0x40].copy_from_slice(&3u32.to_be_bytes()); // dynamic
        footer[0x44..0x54].copy_from_slice(&[0xAB; 16]);
        let cs = checksum(&footer);
        footer[0x40..0x44].copy_from_slice(&cs.to_be_bytes());

        let mut dh = [0u8; 1024];
        dh[0..8].copy_from_slice(b"cxsparse");
        dh[0x08..0x10].copy_from_slice(&u64::MAX.to_be_bytes());
        dh[0x10..0x18].copy_from_slice(&1536u64.to_be_bytes());
        dh[0x18..0x1C].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        dh[0x1C..0x20].copy_from_slice(&table_entries.to_be_bytes());
        dh[0x20..0x24].copy_from_slice(&block_size.to_be_bytes());
        let cs = checksum(&dh);
        dh[0x24..0x28].copy_from_slice(&cs.to_be_bytes());

        let mut buf = Vec::new();
        buf.extend_from_slice(&footer);
        buf.extend_from_slice(&dh);
        buf.extend_from_slice(&vec![0xFFu8; bat_bytes]);
        buf.extend_from_slice(&footer);
        std::fs::write(path, buf).unwrap();
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("exodium_vhd_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("parent")).unwrap();
        d
    }

    #[test]
    fn differencing_child_references_its_parent() {
        let dir = tmp_dir("basic");
        let parent = dir.join("parent/W98-P.vhd");
        write_dynamic_parent(&parent, 64 * 1024 * 1024);
        let child = dir.join("W98-C.vhd");
        create_differencing(&child, &parent, r".\parent\W98-P.vhd").unwrap();

        let data = std::fs::read(&child).unwrap();
        // Footer copy at 0 and footer at end are identical.
        assert_eq!(&data[0..8], b"conectix");
        assert_eq!(&data[0..512], &data[data.len() - 512..]);
        // Disk type 4 (differencing).
        assert_eq!(u32::from_be_bytes(data[0x3C..0x40].try_into().unwrap()), 4);
        // Footer checksum validates.
        let mut f = data[0..512].to_vec();
        let stored = u32::from_be_bytes(f[0x40..0x44].try_into().unwrap());
        f[0x40..0x44].copy_from_slice(&[0; 4]);
        assert_eq!(stored, checksum(&f));

        // Dynamic header: parent UUID copied, checksum validates.
        let dh = &data[512..512 + 1024];
        assert_eq!(&dh[0..8], b"cxsparse");
        assert_eq!(&dh[0x28..0x38], &[0xAB; 16]);
        let mut h = dh.to_vec();
        let stored = u32::from_be_bytes(h[0x24..0x28].try_into().unwrap());
        h[0x24..0x28].copy_from_slice(&[0; 4]);
        assert_eq!(stored, checksum(&h));

        // W2ru locator holds the relative path in UTF-16LE, with FORWARD
        // slashes (backslashes dead-end in minivhd's POSIX path joining).
        let e = 0x240 + 24; // second entry
        assert_eq!(&dh[e..e + 4], b"W2ru");
        let len = u32::from_be_bytes(dh[e + 8..e + 12].try_into().unwrap()) as usize;
        let off = u64::from_be_bytes(dh[e + 16..e + 24].try_into().unwrap()) as usize;
        let decoded: String = data[off..off + len]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .map(|u| char::from_u32(u as u32).unwrap())
            .collect();
        assert_eq!(decoded, "./parent/W98-P.vhd");

        // Parent unicode name is the parent's file name (UTF-16BE), never
        // empty - minivhd's fallback probe otherwise opens the child's
        // DIRECTORY as the parent.
        let pname: String = dh[0x40..0x40 + 2 * "W98-P.vhd".len()]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .map(|u| char::from_u32(u as u32).unwrap())
            .collect();
        assert_eq!(pname, "W98-P.vhd");

        // BAT is all 0xFF (no blocks allocated).
        let bat_off = u64::from_be_bytes(dh[0x10..0x18].try_into().unwrap()) as usize;
        let entries = u32::from_be_bytes(dh[0x1C..0x20].try_into().unwrap()) as usize;
        assert!(data[bat_off..bat_off + entries * 4].iter().all(|&b| b == 0xFF));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_a_differencing_parent() {
        let dir = tmp_dir("chain");
        let parent = dir.join("parent/W98-P.vhd");
        write_dynamic_parent(&parent, 64 * 1024 * 1024);
        let child = dir.join("W98-C.vhd");
        create_differencing(&child, &parent, r".\parent\W98-P.vhd").unwrap();
        // Chaining off the child must be rejected, not silently produce a
        // grandchild the emulators would then fail on.
        let err = create_differencing(&dir.join("bad.vhd"), &child, r".\W98-C.vhd");
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
