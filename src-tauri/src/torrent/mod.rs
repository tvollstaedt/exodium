pub mod manager;

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
}

/// Parsed index of all files in a torrent.
#[derive(Debug, Clone)]
pub struct TorrentIndex {
    pub name: String,
    pub files: Vec<TorrentFileEntry>,
    pub total_size: u64,
}

impl TorrentIndex {
    /// Parse a .torrent file and build the file index.
    pub fn from_file(path: &Path) -> TorrentResult<Self> {
        let torrent = Torrent::read_from_file(path)?;
        let name = torrent.name.clone();

        let files: Vec<TorrentFileEntry> = match torrent.files {
            Some(ref file_list) => file_list
                .iter()
                .enumerate()
                .map(|(i, f)| TorrentFileEntry {
                    index: i,
                    path: f.path.to_string_lossy().replace('\\', "/"),
                    size: f.length as u64,
                })
                .collect(),
            None => {
                // Single-file torrent
                vec![TorrentFileEntry {
                    index: 0,
                    path: torrent.name.clone(),
                    size: torrent.length as u64,
                }]
            }
        };

        let total_size = files.iter().map(|f| f.size).sum();

        Ok(Self {
            name,
            files,
            total_size,
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

    /// Find the game ZIP and optional GameData ZIP for a given game title.
    /// Game title format: "Capitalism (1995)"
    /// Game ZIP path: "eXo/eXoDOS/Capitalism (1995).zip"
    /// GameData ZIP path: "Content/GameData/eXoDOS/Capitalism (1995).zip"
    pub fn find_game_files(
        &self,
        game_title: &str,
    ) -> (Option<&TorrentFileEntry>, Option<&TorrentFileEntry>) {
        let game_zip = format!("{}.zip", game_title);
        let gamedata_prefix = "Content/GameData/eXoDOS/";

        let game = self.files.iter().find(|f| {
            f.path.ends_with(&game_zip) && !f.path.starts_with(gamedata_prefix)
        });

        let gamedata = self
            .files
            .iter()
            .find(|f| f.path == format!("{}{}", gamedata_prefix, game_zip));

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
}

// Compile-time check that DownloadManager can be used in Tauri State<>.
const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    fn check() {
        assert_send_sync::<manager::DownloadManager>();
    }
};

#[cfg(test)]
mod tests {
    use super::*;

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
