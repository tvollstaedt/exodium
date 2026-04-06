use std::path::Path;

use super::ImportResult;

/// Extract a ZIP archive to the given destination directory.
pub fn extract(zip_path: &Path, dest: &Path) -> ImportResult<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    archive.extract(dest)?;
    Ok(())
}
