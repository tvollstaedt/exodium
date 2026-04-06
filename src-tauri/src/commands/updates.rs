use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::queries;

use super::DbState;

// ── Manifest schema ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ThumbnailPackInfo {
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub version: u32,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CollectionManifest {
    pub torrent_infohash: String,
    pub game_count: u32,
    pub thumbnail_pack: Option<ThumbnailPackInfo>,
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

fn load_manifest() -> Result<Manifest, String> {
    // Dev: read from the project root next to Cargo.toml
    #[cfg(debug_assertions)]
    {
        let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("cannot resolve project root")?
            .join("manifest.json");
        if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path)
                .map_err(|e| format!("cannot read manifest.json: {}", e))?;
            return serde_json::from_str(&content)
                .map_err(|e| format!("cannot parse manifest.json: {}", e));
        }
        return Err("manifest.json not found in project root".to_string());
    }

    // Production: HTTP fetch — not yet implemented (needs reqwest).
    // When ready, read the URL from the manifest's own `manifest_url` field
    // or a compile-time constant, then deserialise the response.
    #[allow(unreachable_code)]
    Err("Remote manifest fetch not yet implemented".to_string())
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
