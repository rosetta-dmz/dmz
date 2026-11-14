//! Local store cache checking and fast-path artifact validation.

use crate::diagnostics::CoreMetrics;
use crate::resolver::lockfile::LockedPackage;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use tracing::{info, warn};

const CACHE_HASH_CHUNK_SIZE: usize = 128 * 1024;

/// Marks a cached store file read-only (immutable store semantics akin to `/nix/store`).
pub fn mark_store_file_readonly(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() & !0o222);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

fn verify_file_sha256(path: &Path, expected_hex: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; CACHE_HASH_CHUNK_SIZE];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Some(chunk) = buf.get(..n) {
                    hasher.update(chunk);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    let actual_hex = crate::archive::exporter::hashing::format_sha256_hex(&hasher.finalize());
    actual_hex.eq_ignore_ascii_case(expected_hex.trim())
}

/// Checks if a verified, non-empty artifact and matching metadata already exist in the local store.
/// Verifies streaming SHA-256 integrity by default (mirroring `nix-store --verify --check-contents --repair`)
/// so corrupted store artifacts are treated as cache misses and repaired automatically.
pub fn check_cached_artifact(
    pkg_dir: &Path,
    safe_pkg_name: &str,
    clean_version: &str,
    map_key: &str,
) -> Option<(String, LockedPackage)> {
    check_cached_artifact_with_options(pkg_dir, safe_pkg_name, clean_version, map_key, true)
}

/// Checks the local store cache with configurable deep SHA-256 content verification.
pub fn check_cached_artifact_with_options(
    pkg_dir: &Path,
    safe_pkg_name: &str,
    clean_version: &str,
    map_key: &str,
    verify_contents: bool,
) -> Option<(String, LockedPackage)> {
    let artifact_filename = format!("{}-{}.archive", safe_pkg_name, clean_version);
    let artifact_path = pkg_dir.join(&artifact_filename);
    let meta_path = pkg_dir.join("metadata.json");

    if !artifact_path.exists() || !meta_path.exists() {
        CoreMetrics::record_cache_miss();
        return None;
    }

    let Ok(meta_content) = std::fs::read_to_string(&meta_path) else {
        CoreMetrics::record_cache_miss();
        return None;
    };
    let Ok(cached_pkg) = serde_json::from_str::<LockedPackage>(&meta_content) else {
        CoreMetrics::record_cache_miss();
        return None;
    };

    if cached_pkg.version != clean_version || cached_pkg.sha256.is_empty() {
        CoreMetrics::record_cache_miss();
        return None;
    }

    let Ok(meta) = std::fs::metadata(&artifact_path) else {
        CoreMetrics::record_cache_miss();
        return None;
    };
    if meta.len() == 0 {
        CoreMetrics::record_cache_miss();
        return None;
    }

    if verify_contents && !verify_file_sha256(&artifact_path, &cached_pkg.sha256) {
        CoreMetrics::record_cache_corruption();
        CoreMetrics::record_cache_miss();
        warn!(
            name = %cached_pkg.name,
            version = %clean_version,
            "Cached store artifact failed SHA-256 verification; marking for re-download repair"
        );
        return None;
    }

    CoreMetrics::record_cache_hit();
    info!(
        name = %cached_pkg.name,
        version = %clean_version,
        bytes = meta.len(),
        verified = verify_contents,
        "Reusing existing valid cached artifact from store"
    );

    Some((map_key.to_string(), cached_pkg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::manifest::Ecosystem;
    use tempfile::tempdir;

    #[test]
    fn test_check_cached_artifact_detects_corruption_and_repairs() {
        let dir = tempdir().unwrap();
        let pkg_dir = dir.path().join("demo");
        std::fs::create_dir_all(&pkg_dir).unwrap();

        let payload = b"valid-tarball-bytes";
        let expected_hash =
            crate::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(payload));

        let artifact_path = pkg_dir.join("demo-1.0.0.archive");
        std::fs::write(&artifact_path, payload).unwrap();

        let pkg = LockedPackage {
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: expected_hash,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };
        let meta_path = pkg_dir.join("metadata.json");
        std::fs::write(&meta_path, serde_json::to_string_pretty(&pkg).unwrap()).unwrap();

        assert!(check_cached_artifact(&pkg_dir, "demo", "1.0.0", "cargo:demo").is_some());

        // Corrupt the cached artifact on disk
        std::fs::write(&artifact_path, b"corrupted-payload!").unwrap();
        assert!(check_cached_artifact(&pkg_dir, "demo", "1.0.0", "cargo:demo").is_none());

        // Opt-in stat-only mode still hits when metadata and non-empty file exist
        assert!(
            check_cached_artifact_with_options(&pkg_dir, "demo", "1.0.0", "cargo:demo", false)
                .is_some()
        );
    }
}
