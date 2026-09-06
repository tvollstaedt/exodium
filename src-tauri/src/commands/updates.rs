use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Manifest schema (v2) ─────────────────────────────────────────────────────

/// One platform's download source for a pack whose payload differs per OS
/// (the emulator packs: a macOS .app is useless on Linux and vice versa).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlatformSource {
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// A content pack: an HTTP tar.gz (url + sha256) or a zip inside the
/// collection's torrent (`torrent_file_path`, integrity by librqbit).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContentPackInfo {
    pub display_name: String,
    pub description: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub version: u32,
    /// Install dir under data_dir, owned by this pack ALONE: the installer
    /// removes it before renaming staging onto it (§10).
    pub install_path: String,
    /// Pack IDs this pack replaces (e.g. media supersedes posters).
    #[serde(default)]
    pub supersedes: Vec<String>,
    /// Oldest installed version still usable; below it the pack is deleted
    /// at startup, at or above it an update is merely offered (§10).
    #[serde(default)]
    pub min_compatible_version: u32,
    /// If set, install via torrent selective-download instead of HTTP. Value
    /// is the file path inside the collection's torrent
    /// (e.g. "Content/XODOSMetadata.zip"). The extractor expects a .zip.
    #[serde(default)]
    pub torrent_file_path: Option<String>,
    /// Per-platform sources keyed by release-target token ("darwin-aarch64");
    /// a platform without an entry never sees the pack (§10).
    #[serde(default)]
    pub platforms: Option<HashMap<String, PlatformSource>>,
}

/// The release-target token for the running build, matching the platforms-map
/// keys in manifest.json.
pub(crate) fn current_platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-aarch64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-aarch64"
    } else if cfg!(target_os = "windows") {
        "windows-x86_64"
    } else {
        "unknown"
    }
}

impl ContentPackInfo {
    /// The pack with this platform's source substituted; None when the pack
    /// is not for this platform.
    pub fn for_current_platform(&self) -> Option<ContentPackInfo> {
        self.for_platform(current_platform())
    }

    fn for_platform(&self, platform: &str) -> Option<ContentPackInfo> {
        match &self.platforms {
            None => Some(self.clone()),
            Some(map) => map.get(platform).map(|src| {
                let mut out = self.clone();
                out.url = src.url.clone();
                out.sha256 = src.sha256.clone();
                out.size_bytes = src.size_bytes;
                out
            }),
        }
    }
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

// ── Manifest loading ──────────────────────────────────────────────────────────

/// The manifest: repo root in dev, bundled copy in production (§10).
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
    if let Some(res_dir) = super::paths::RESOURCE_DIR.get() {
        let bundled = res_dir.join("manifest.json");
        if bundled.exists() {
            let content = std::fs::read_to_string(&bundled)
                .map_err(|e| format!("cannot read bundled manifest.json: {}", e))?;
            return serde_json::from_str(&content)
                .map_err(|e| format!("cannot parse bundled manifest.json: {}", e));
        }
    }

    // TODO: HTTP fetch from manifest_url as final fallback.
    Err("manifest.json not found (dev path or resource_dir)".to_string())
}

#[cfg(test)]
mod manifest_load_tests {
    #[test]
    fn manifest_parses_with_packs() {
        let m = super::load_manifest().expect("load_manifest failed");
        let ex = m.collections.get("eXoDOS").expect("no eXoDOS collection");
        assert!(!ex.content_packs.is_empty(), "eXoDOS has no content packs");
        println!("packs: {:?}", ex.content_packs.keys().collect::<Vec<_>>());
    }

    #[test]
    fn platform_map_substitutes_source_triple() {
        let pack: super::ContentPackInfo = serde_json::from_str(
            r#"{
                "display_name": "DOSBox-X Emulator", "description": "",
                "url": "", "sha256": "", "size_bytes": 0, "version": 1,
                "install_path": "content/emulators/dosbox-x",
                "platforms": {
                    "linux-x86_64": { "url": "https://x/l.tar.gz", "sha256": "aa", "size_bytes": 55 },
                    "darwin-aarch64": { "url": "https://x/m.tar.gz", "sha256": "bb", "size_bytes": 52 }
                }
            }"#,
        )
        .expect("pack with platforms map fails to parse");

        let linux = pack.for_platform("linux-x86_64").expect("linux entry missing");
        assert_eq!(linux.url, "https://x/l.tar.gz");
        assert_eq!(linux.sha256, "aa");
        assert_eq!(linux.size_bytes, 55);
        // Non-source fields carry over untouched.
        assert_eq!(linux.install_path, "content/emulators/dosbox-x");
        assert_eq!(linux.version, 1);

        // No entry for the platform = the pack does not exist there.
        assert!(pack.for_platform("windows-x86_64").is_none());
    }

    #[test]
    fn packs_without_platform_map_pass_through() {
        let pack: super::ContentPackInfo = serde_json::from_str(
            r#"{
                "display_name": "Box Art", "description": "",
                "url": "https://x/p.tar.gz", "sha256": "cc", "size_bytes": 9,
                "version": 5, "install_path": "content/posters/eXoDOS"
            }"#,
        )
        .expect("plain pack fails to parse");
        let resolved = pack.for_platform("windows-x86_64").expect("plain pack filtered out");
        assert_eq!(resolved.url, "https://x/p.tar.gz");
        assert_eq!(resolved.size_bytes, 9);
    }

    /// A shared install_path means installing one pack deletes another's.
    #[test]
    fn manifest_install_paths_are_unique() {
        let m = super::load_manifest().expect("load_manifest failed");
        let mut seen = std::collections::HashMap::new();
        for (col, cm) in &m.collections {
            for (id, pack) in &cm.content_packs {
                let key = pack.install_path.trim_matches('/').to_string();
                assert!(
                    !key.is_empty(),
                    "{col}:{id} has an empty install_path"
                );
                if let Some(prev) = seen.insert(key.clone(), format!("{col}:{id}")) {
                    panic!("install_path {key:?} is shared by {prev} and {col}:{id}");
                }
            }
        }
    }
}
