//! Local store cache checking and fast-path artifact validation.

use crate::resolver::lockfile::LockedPackage;
use std::path::Path;
use tracing::info;

/// Checks if a verified, non-empty artifact and matching metadata already exist in the local store.
/// Returns `Some((map_key, cached_pkg))` if cache hit is valid, avoiding redundant network downloads.
pub fn check_cached_artifact(
    pkg_dir: &Path,
    safe_pkg_name: &str,
    clean_version: &str,
    map_key: &str,
) -> Option<(String, LockedPackage)> {
    let artifact_filename = format!("{}-{}.archive", safe_pkg_name, clean_version);
    let artifact_path = pkg_dir.join(&artifact_filename);
    let meta_path = pkg_dir.join("metadata.json");

    if !artifact_path.exists() || !meta_path.exists() {
        return None;
    }

    let meta_content = std::fs::read_to_string(&meta_path).ok()?;
    let cached_pkg = serde_json::from_str::<LockedPackage>(&meta_content).ok()?;

    if cached_pkg.version != clean_version || cached_pkg.sha256.is_empty() {
        return None;
    }

    let meta = std::fs::metadata(&artifact_path).ok()?;
    if meta.len() == 0 {
        return None;
    }

    info!(
        name = %cached_pkg.name,
        version = %clean_version,
        bytes = meta.len(),
        "Reusing existing valid cached artifact from store"
    );

    Some((map_key.to_string(), cached_pkg))
}
