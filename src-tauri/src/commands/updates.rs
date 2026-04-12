use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::queries;

use super::DbState;

// ── Manifest schema (v2) ─────────────────────────────────────────────────────

/// A downloadable content pack (posters, media, etc.) hosted as a tar.gz asset.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContentPackInfo {
    pub display_name: String,
    pub description: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub version: u32,
    /// Relative path under data_dir where the pack extracts to.
    pub install_path: String,
    /// Pack IDs this pack replaces (e.g. media supersedes posters).
    #[serde(default)]
    pub supersedes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CollectionManifest {
    pub torrent_infohash: String,
    pub game_count: u32,
    /// Available content packs keyed by pack ID (e.g. "posters", "media").
    #[serde(default)]
    pub content_packs: HashMap<String, ContentPackInfo>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Manifest {
    pub schema_version: u32,
    pub generated_at: String,
    pub collections: HashMap<String, CollectionManifest>,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CollectionUpdate {
    pub collection: String,
    pub current_hash: String,
    pub latest_hash: String,
    pub new_game_count: u32,
}

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub updates: Vec<CollectionUpdate>,
}

// ── Manifest loading ──────────────────────────────────────────────────────────

/// Load the manifest from the best available source.
/// Dev mode reads from the project root. Production reads the bundled copy
/// from resource_dir (shipped via bundle.resources). HTTP fetch from a remote
/// manifest_url is a future improvement (v0.2+).
pub(crate) fn load_manifest() -> Result<Manifest, String> {
    // Dev: read from the project root next to Cargo.toml
    let dev_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("manifest.json"))
        .unwrap_or_default();
    if dev_path.exists() {
        let content = std::fs::read_to_string(&dev_path)
            .map_err(|e| format!("cannot read manifest.json: {}", e))?;
        return serde_json::from_str(&content)
            .map_err(|e| format!("cannot parse manifest.json: {}", e));
    }

    // Production: read the bundled copy from resource_dir.
    if let Some(res_dir) = super::setup::RESOURCE_DIR.get() {
        let bundled = res_dir.join("manifest.json");
        if bundled.exists() {
            let content = std::fs::read_to_string(&bundled)
                .map_err(|e| format!("cannot read bundled manifest.json: {}", e))?;
            return serde_json::from_str(&content)
                .map_err(|e| format!("cannot parse bundled manifest.json: {}", e));
        }
    }

    // TODO (v0.2): HTTP fetch from manifest_url as final fallback.
    Err("manifest.json not found (dev path or resource_dir)".to_string())
}

// ── Tauri command ─────────────────────────────────────────────────────────────

/// Compare the locally stored torrent infohashes against the manifest.
/// Returns a list of collections that have a newer version available.
#[tauri::command]
pub fn check_for_updates(db_state: State<'_, DbState>) -> Result<UpdateInfo, String> {
    let manifest = load_manifest()?;
    let conn = db_state.0.lock().map_err(|e| e.to_string())?;

    let mut updates = Vec::new();

    for (col_id, col_manifest) in &manifest.collections {
        // Ignore placeholder values left in the dev manifest
        if col_manifest.torrent_infohash.starts_with("REPLACE") {
            continue;
        }

        let stored = queries::get_config(&conn, &format!("{}_infohash", col_id))
            .map_err(|e| e.to_string())?;

        if let Some(current_hash) = stored {
            if current_hash != col_manifest.torrent_infohash {
                updates.push(CollectionUpdate {
                    collection: col_id.clone(),
                    current_hash,
                    latest_hash: col_manifest.torrent_infohash.clone(),
                    new_game_count: col_manifest.game_count,
                });
            }
        }
        // If no stored hash yet (collection not initialised), skip silently.
    }

    Ok(UpdateInfo { updates })
}
