pub mod extract;
pub mod xml;

use std::io::BufReader;
use std::path::Path;

use thiserror::Error;

use crate::db;
use crate::models::Game;

#[derive(Error, Debug)]
pub enum ImportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::DeError),
    #[error("ZIP error: {0}")]
    Zip(#[from] ::zip::result::ZipError),
    #[error("Database error: {0}")]
    Db(#[from] db::DbError),
    #[error("Import failed: {0}")]
    Other(String),
}

pub type ImportResult<T> = Result<T, ImportError>;

/// Known paths where MS-DOS.xml might live inside various eXoDOS ZIPs.
const XML_CANDIDATES: &[&str] = &[
    "xml/all/MS-DOS.xml",
    "Metadata/MS-DOS.xml",
    "MS-DOS.xml",
];

/// Import games from an eXo metadata ZIP (XODOSMetadata.zip, GLP, etc.).
/// Searches for MS-DOS.xml inside the archive, parses it, and inserts into the DB.
///
/// `shortcode_segment` is the collection-specific path component used to extract
/// shortcodes from application_path (e.g. "!dos" for eXoDOS).
pub fn import_from_zip(
    zip_path: &Path,
    conn: &rusqlite::Connection,
    shortcode_segment: &str,
) -> ImportResult<usize> {
    log::info!("Opening ZIP: {}", zip_path.display());
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    // Collect all entry names so we can search without holding a borrow
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
        .collect();

    // Try known paths first, then fall back to any entry ending in MS-DOS.xml
    let xml_name = XML_CANDIDATES
        .iter()
        .find(|c| names.iter().any(|n| n == **c))
        .map(|s| s.to_string())
        .or_else(|| names.into_iter().find(|n| n.ends_with("MS-DOS.xml")));

    let xml_name = xml_name.ok_or_else(|| {
        let first: Vec<_> = (0..archive.len().min(20))
            .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
            .collect();
        ImportError::Other(format!(
            "No MS-DOS.xml found in ZIP. First entries: {:?}",
            first
        ))
    })?;

    log::info!("Reading XML from: {}", xml_name);
    let xml_entry = archive.by_name(&xml_name)?;
    let reader = BufReader::new(xml_entry);
    let games: Vec<Game> = xml::parse_games_xml(reader, shortcode_segment)?;

    let count = db::queries::insert_games(conn, &games)?;
    Ok(count)
}
