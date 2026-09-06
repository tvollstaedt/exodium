pub mod manager;
pub mod zip_range;

use std::path::Path;

use lava_torrent::torrent::v1::Torrent;
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TorrentError {
    #[error("Torrent parse error: {0}")]
    Parse(#[from] lava_torrent::LavaTorrentError),
    #[error("Torrent error: {0}")]
    Other(String),
}

pub type TorrentResult<T> = Result<T, TorrentError>;

/// A single file entry from the torrent.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentFileEntry {
    /// 0-based index within the torrent's file list.
    pub index: usize,
    /// Relative path within the torrent (forward slashes).
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Byte offset of this file within the torrent's contiguous piece space
    /// (cumulative size of all preceding files). Piece index of a byte =
    /// (offset + n) / piece_length.
    pub offset: u64,
}

/// Parsed index of all files in a torrent.
#[derive(Debug, Clone)]
pub struct TorrentIndex {
    pub name: String,
    pub files: Vec<TorrentFileEntry>,
    pub total_size: u64,
    /// Piece length in bytes (from the torrent's info dict).
    pub piece_length: u64,
}

impl TorrentIndex {
    /// Parse a .torrent file and build the file index.
    pub fn from_file(path: &Path) -> TorrentResult<Self> {
        let torrent = Torrent::read_from_file(path)?;
        let name = torrent.name.clone();

        let files: Vec<TorrentFileEntry> = match torrent.files {
            Some(ref file_list) => {
                let mut offset = 0u64;
                file_list
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let entry = TorrentFileEntry {
                            index: i,
                            path: f.path.to_string_lossy().replace('\\', "/"),
                            size: f.length as u64,
                            offset,
                        };
                        offset += f.length as u64;
                        entry
                    })
                    .collect()
            }
            None => {
                // Single-file torrent
                vec![TorrentFileEntry {
                    index: 0,
                    path: torrent.name.clone(),
                    size: torrent.length as u64,
                    offset: 0,
                }]
            }
        };

        let total_size = files.iter().map(|f| f.size).sum();

        Ok(Self {
            name,
            files,
            total_size,
            piece_length: torrent.piece_length as u64,
        })
    }

    /// Find a file by exact path.
    pub fn find_by_path(&self, path: &str) -> Option<&TorrentFileEntry> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Find a file whose path ends with the given suffix.
    pub fn find_by_suffix(&self, suffix: &str) -> Option<&TorrentFileEntry> {
        self.files.iter().find(|f| f.path.ends_with(suffix))
    }

    /// The game zip (`eXo/eXoDOS/<Title (Year)>.zip`) and, if any, its GameData
    /// zip (`Content/GameData/eXoDOS/…`).
    pub fn find_game_files(
        &self,
        game_title: &str,
    ) -> (Option<&TorrentFileEntry>, Option<&TorrentFileEntry>) {
        let game_zip = format!("{}.zip", game_title);
        let gamedata_prefix = "Content/GameData/eXoDOS/";

        // Anchored on a path boundary ("Billiards" must not match "4 Balls
        // Billiards") and case-insensitive (bat and zip disagree in places).
        let game_zip_anchored = format!("/{}", game_zip);
        let game = self.files.iter().find(|f| {
            (f.path.eq_ignore_ascii_case(&game_zip)
                || ends_with_ignore_ascii_case(&f.path, &game_zip_anchored))
                && !f.path.starts_with(gamedata_prefix)
        });

        let gamedata_path = format!("{}{}", gamedata_prefix, game_zip);
        let gamedata = self
            .files
            .iter()
            .find(|f| f.path.eq_ignore_ascii_case(&gamedata_path));

        (game, gamedata)
    }

    /// Find the metadata ZIP (XODOSMetadata.zip).
    pub fn find_metadata_zip(&self) -> Option<&TorrentFileEntry> {
        self.files
            .iter()
            .find(|f| f.path.ends_with("XODOSMetadata.zip"))
    }

    /// Find the DOSBox metadata ZIP (!DOSmetadata.zip).
    pub fn find_dosbox_metadata_zip(&self) -> Option<&TorrentFileEntry> {
        self.files
            .iter()
            .find(|f| f.path.ends_with("!DOSmetadata.zip"))
    }

    /// Return the SHA1 info-hash (hex) of a torrent file.
    pub fn infohash(path: &Path) -> TorrentResult<String> {
        let torrent = Torrent::read_from_file(path)?;
        Ok(torrent.info_hash())
    }
}

// Compile-time check that DownloadManager can be used in Tauri State<>.
const _: () = {
    #[allow(dead_code)]
    fn assert_send_sync<T: Send + Sync>() {}
    #[allow(dead_code)]
    fn check() {
        assert_send_sync::<manager::DownloadManager>();
    }
};

/// ASCII-case-insensitive `ends_with`, comparing bytes so a multi-byte char at
/// the boundary can't panic a slice (non-ASCII bytes compare verbatim).
fn ends_with_ignore_ascii_case(hay: &str, needle: &str) -> bool {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    h.len() >= n.len() && h[h.len() - n.len()..].eq_ignore_ascii_case(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// eXo's launcher bats and zips disagree in case for a handful of games
    /// ("I can be a..." bat vs "I Can be a..." zip) - matching must not care.
    #[test]
    fn find_game_files_ignores_case() {
        let index = TorrentIndex {
            name: "eXoWin3x".to_string(),
            files: vec![TorrentFileEntry {
                index: 0,
                path: "eXo/eXoWin3x/I Can be a Dinosaur Finder (1997).zip".to_string(),
                size: 1,
                offset: 0,
            }],
            total_size: 1,
            piece_length: 16384,
        };
        let (game, _) = index.find_game_files("I can be a Dinosaur Finder (1997)");
        assert!(game.is_some(), "case difference must not break the match");
        // The path-boundary anchor still holds: a suffix of a LONGER title
        // must not match regardless of case.
        let (partial, _) = index.find_game_files("Dinosaur Finder (1997)");
        assert!(partial.is_none(), "anchored match must not allow suffixes");
    }

    #[test]
    fn test_parse_exodos_torrent() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("torrents/eXoDOS.torrent");
        if !path.exists() {
            eprintln!("Skipping: torrent file not found at {:?}", path);
            return;
        }

        let index = TorrentIndex::from_file(&path).unwrap();
        assert_eq!(index.name, "eXoDOS");
        assert_eq!(index.files.len(), 14011);

        // Check metadata ZIP exists
        let meta = index.find_metadata_zip().unwrap();
        assert!(meta.path.ends_with("XODOSMetadata.zip"));
        assert_eq!(meta.index, 8); // 0-based, so file #9 is index 8

        // Check game lookup
        let (game, gamedata) = index.find_game_files("Capitalism (1995)");
        let game = game.unwrap();
        assert!(game.path.contains("Capitalism (1995).zip"));
        assert!(gamedata.is_some());

        println!("Torrent: {} files, {:.1} GB", index.files.len(), index.total_size as f64 / 1e9);
        println!("Metadata ZIP: index={}, size={}", meta.index, meta.size);
        println!("Capitalism: game index={}, gamedata index={}", game.index, gamedata.unwrap().index);
    }

    #[test]
    fn test_find_game_files_is_path_anchored() {
        // Regression: an unanchored ends_with() let short titles match longer
        // ones ("Billiards (1993).zip" -> "4 Balls Billiards (1993).zip"),
        // mapping ~34 games to the wrong torrent file.
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("torrents/eXoDOS.torrent");
        if !path.exists() {
            eprintln!("Skipping: torrent file not found at {:?}", path);
            return;
        }

        let index = TorrentIndex::from_file(&path).unwrap();
        for title in [
            "Billiards (1993)",
            "Gods (1991)",
            "Tetris (1991)",
            "Pac-Man (1983)",
            "Quake (1996)",
            "Incredible Machine, The (1993)",
        ] {
            let (game, _) = index.find_game_files(title);
            let game = game.unwrap_or_else(|| panic!("{title} not found"));
            assert_eq!(
                game.path,
                format!("eXo/eXoDOS/{title}.zip"),
                "wrong file matched for {title}"
            );
        }
    }

    #[test]
    fn test_parse_glp_torrent() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("torrents/eXoDOS_GLP.torrent");
        if !path.exists() {
            eprintln!("Skipping: GLP torrent file not found");
            return;
        }

        let index = TorrentIndex::from_file(&path).unwrap();
        assert_eq!(index.files.len(), 660);
        println!("GLP torrent: {} files, {:.1} GB", index.files.len(), index.total_size as f64 / 1e9);
    }
}
