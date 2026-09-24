//! Reclaiming the gaps that non-sparse torrent files hold on Windows (§21).
//! A file written far past its end without the sparse attribute allocates
//! everything before the write; those clusters read as zeros. Marking the
//! file sparse and deallocating its all-zero ranges frees them without
//! changing a byte a reader sees, so the ledger stays valid.

use std::path::Path;

use crate::torrent::TorrentFileEntry;

/// Below this a file is not worth a scan.
#[cfg_attr(not(windows), allow(dead_code))]
const MIN_SIZE: u64 = 64 << 20;

#[derive(Debug, Default, PartialEq)]
pub struct ReclaimReport {
    pub scanned: usize,
    pub freed_bytes: u64,
}

/// Coalesce chunk verdicts (`start`, `end`, all-zero) into the ranges to
/// deallocate.
#[cfg_attr(not(windows), allow(dead_code))]
fn zero_runs(chunks: impl Iterator<Item = (u64, u64, bool)>) -> Vec<(u64, u64)> {
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for (start, end, zero) in chunks {
        if !zero {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.1 == start => last.1 = end,
            _ => runs.push((start, end)),
        }
    }
    runs
}

/// Full-length, non-sparse torrent files under `root` become sparse with
/// their zero ranges deallocated. Reads every candidate once; `on_first`
/// fires before the first scan, so a UI can announce the wait.
#[cfg(windows)]
pub fn reclaim_allocated_gaps(root: &Path, files: &[TorrentFileEntry], chunk: u64, on_first: &mut dyn FnMut()) -> ReclaimReport {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
    let mut report = ReclaimReport::default();
    for f in files.iter().filter(|f| f.size >= MIN_SIZE) {
        let path = root.join(&f.path);
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.len() != f.size || meta.file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE != 0 {
            continue;
        }
        report.scanned += 1;
        if report.scanned == 1 {
            on_first();
        }
        match win::reclaim_file(&path, chunk.max(1 << 20) as usize) {
            Ok(freed) => {
                log::info!("sparse: {} freed {} MB", path.display(), freed >> 20);
                report.freed_bytes += freed;
            }
            Err(e) => log::warn!("sparse: {} left as is: {e}", path.display()),
        }
    }
    report
}

#[cfg(not(windows))]
pub fn reclaim_allocated_gaps(_root: &Path, _files: &[TorrentFileEntry], _chunk: u64, _on_first: &mut dyn FnMut()) -> ReclaimReport {
    ReclaimReport::default()
}

#[cfg(windows)]
mod win {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use fs4::FileExt;
    use windows_sys::Win32::System::Ioctl::{FILE_ZERO_DATA_INFORMATION, FSCTL_SET_SPARSE, FSCTL_SET_ZERO_DATA};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    pub(super) fn reclaim_file(path: &Path, chunk: usize) -> io::Result<u64> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let before = file.allocated_size()?;
        set_sparse(&file)?;
        let len = file.metadata()?.len();
        let zeros = vec![0u8; chunk];
        let mut buf = vec![0u8; chunk];
        let mut verdicts = Vec::new();
        let mut pos = 0u64;
        while pos < len {
            let n = (len - pos).min(chunk as u64) as usize;
            file.read_exact(&mut buf[..n])?;
            verdicts.push((pos, pos + n as u64, buf[..n] == zeros[..n]));
            pos += n as u64;
        }
        for (start, end) in super::zero_runs(verdicts.into_iter()) {
            zero_data(&file, start, end)?;
        }
        Ok(before.saturating_sub(file.allocated_size()?))
    }

    fn set_sparse(file: &File) -> io::Result<()> {
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_SPARSE,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    fn zero_data(file: &File, start: u64, end: u64) -> io::Result<()> {
        let info = FILE_ZERO_DATA_INFORMATION { FileOffset: start as i64, BeyondFinalZero: end as i64 };
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_ZERO_DATA,
                &info as *const _ as *const _,
                std::mem::size_of::<FILE_ZERO_DATA_INFORMATION>() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_runs_coalesce_neighbours_and_keep_data_gaps() {
        let chunks = [(0, 8, true), (8, 16, true), (16, 24, false), (24, 32, true), (32, 33, true)];
        assert_eq!(zero_runs(chunks.into_iter()), vec![(0, 16), (24, 33)]);
        assert_eq!(zero_runs([(0u64, 8u64, false)].into_iter()), Vec::<(u64, u64)>::new());
    }

    /// The migration on a file an earlier build left fully allocated: the
    /// data at the end survives, the gap before it is given back.
    #[cfg(windows)]
    #[test]
    fn reclaim_keeps_the_tail_and_frees_the_gap() {
        use fs4::FileExt;
        use std::io::{Read, Seek, SeekFrom, Write};
        use std::os::windows::fs::MetadataExt;
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("Content/big.zip");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let size = 300u64 << 20;
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.seek(SeekFrom::Start(size - 8)).unwrap();
            f.write_all(b"tailtail").unwrap();
        }
        assert!(std::fs::File::open(&path).unwrap().allocated_size().unwrap() >= size);
        let files = vec![TorrentFileEntry { index: 0, path: "Content/big.zip".into(), size, offset: 0 }];
        let mut announced = 0;
        let report = reclaim_allocated_gaps(td.path(), &files, 8 << 20, &mut || announced += 1);
        assert_eq!((report.scanned, announced), (1, 1));
        assert!(report.freed_bytes > size / 2, "freed {}", report.freed_bytes);
        let mut f = std::fs::File::open(&path).unwrap();
        assert!(f.metadata().unwrap().file_attributes() & 0x200 != 0);
        assert!(f.allocated_size().unwrap() < 16 << 20);
        let mut tail = [0u8; 8];
        f.seek(SeekFrom::Start(size - 8)).unwrap();
        f.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"tailtail");
        // A second pass finds nothing to do.
        assert_eq!(reclaim_allocated_gaps(td.path(), &files, 8 << 20, &mut || panic!("nothing to announce")), ReclaimReport::default());
    }
}
