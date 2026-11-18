//! Local store cache checking and fast-path artifact validation.

use crate::diagnostics::CoreMetrics;
use crate::resolver::lockfile::LockedPackage;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
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

pub(crate) fn verify_file_sha256(path: &Path, expected_hex: &str) -> bool {
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

pub(crate) fn pypi_archive_matches_name_and_version(
    archive_filename: &str,
    pkg_name: &str,
    version: &str,
) -> bool {
    let Some(stem) = archive_filename.strip_suffix(".archive") else {
        return false;
    };
    let exact_ver_suffix = format!("-{version}");
    let tagged_ver_marker = format!("-{version}-");
    let candidate_pkg_part = if let Some(prefix) = stem.strip_suffix(&exact_ver_suffix) {
        prefix
    } else if let Some((prefix, _)) = stem.split_once(&tagged_ver_marker) {
        prefix
    } else {
        return false;
    };
    crate::resolver::manifest::ManifestScanner::normalize_pypi_name(candidate_pkg_part)
        == crate::resolver::manifest::ManifestScanner::normalize_pypi_name(pkg_name)
}

/// Checks the immutable `v2/` PyPI cache bucket inside `pkg_dir`, validating both
/// PEP 425 ABI/platform filename tags and SHA-256 integrity.
#[must_use]
pub fn check_cached_pypi_artifact(
    pkg_dir: &Path,
    safe_pkg_name: &str,
    clean_version: &str,
    map_key: &str,
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
) -> Option<(String, LockedPackage)> {
    let v2_dir = pkg_dir.join("v2");
    let Ok(entries) = std::fs::read_dir(&v2_dir) else {
        CoreMetrics::record_cache_miss();
        return None;
    };

    let mut best: Option<(i32, PathBuf, LockedPackage)> = None;

    for entry in entries.flatten() {
        let meta_path = entry.path();
        let Some(meta_name) = meta_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = meta_name.strip_suffix(".meta.json") else {
            continue;
        };
        let archive_filename = format!("{stem}.archive");
        if !pypi_archive_matches_name_and_version(&archive_filename, safe_pkg_name, clean_version) {
            continue;
        }
        let Some(compat_score) = crate::resolver::fetch::pypi::cached_pypi_v2_compatibility_score(
            &archive_filename,
            target_platform,
            preferred_cp_tag,
        ) else {
            continue;
        };
        let archive_path = v2_dir.join(&archive_filename);
        let Ok(file_meta) = std::fs::metadata(&archive_path) else {
            continue;
        };
        if file_meta.len() == 0 {
            continue;
        }
        let Ok(meta_content) = std::fs::read_to_string(&meta_path) else {
            continue;
        };
        let Ok(cached_pkg) = serde_json::from_str::<LockedPackage>(&meta_content) else {
            continue;
        };
        if cached_pkg.version != clean_version || cached_pkg.sha256.is_empty() {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(best_score, _, _)| compat_score > *best_score)
        {
            best = Some((compat_score, archive_path, cached_pkg));
        }
    }

    let Some((_, artifact_path, cached_pkg)) = best else {
        CoreMetrics::record_cache_miss();
        return None;
    };

    if !verify_file_sha256(&artifact_path, &cached_pkg.sha256) {
        CoreMetrics::record_cache_corruption();
        CoreMetrics::record_cache_miss();
        warn!(
            name = %cached_pkg.name,
            version = %clean_version,
            file = ?artifact_path.file_name(),
            "Cached PyPI v2 store artifact failed SHA-256 verification; marking for re-download repair"
        );
        return None;
    }

    CoreMetrics::record_cache_hit();
    info!(
        name = %cached_pkg.name,
        version = %clean_version,
        file = ?artifact_path.file_name(),
        "Reusing existing valid immutable v2 PyPI artifact from store"
    );
    Some((map_key.to_string(), cached_pkg))
}

/// Checks if a verified, non-empty artifact and matching metadata already exist in the local store.
/// Verifies streaming SHA-256 integrity by default (mirroring `nix-store --verify --check-contents --repair`)
/// so corrupted store artifacts are treated as cache misses and repaired automatically.
#[must_use]
pub fn check_cached_artifact(
    pkg_dir: &Path,
    safe_pkg_name: &str,
    clean_version: &str,
    map_key: &str,
) -> Option<(String, LockedPackage)> {
    check_cached_artifact_with_options(pkg_dir, safe_pkg_name, clean_version, map_key, true)
}

/// Checks the local store cache with configurable deep SHA-256 content verification.
#[must_use]
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

    #[test]
    fn test_check_cached_pypi_v2_isolates_abis_and_ignores_v1_files() {
        let dir = tempdir().unwrap();
        let pkg_dir = dir.path().join("pydantic_core");
        let v2_dir = pkg_dir.join("v2");
        std::fs::create_dir_all(&v2_dir).unwrap();

        // Legacy un-tagged v1 cache file must be ignored by v2 lookups.
        let v1_bytes = b"legacy-cp311-payload";
        std::fs::write(pkg_dir.join("pydantic_core-2.27.1.archive"), v1_bytes).unwrap();
        assert!(check_cached_pypi_artifact(
            &pkg_dir,
            "pydantic_core",
            "2.27.1",
            "pypi:pydantic_core",
            Some("macos-arm64"),
            Some("cp312"),
        )
        .is_none());

        // Populate immutable cp311 wheel in v2/: must miss for cp312, hit for cp311.
        let cp311_bytes = b"PK\x03\x04cp311-wheel";
        let cp311_sha =
            crate::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(cp311_bytes));
        let cp311_stem = "pydantic_core-2.27.1-cp311-cp311-macosx_11_0_arm64";
        std::fs::write(v2_dir.join(format!("{cp311_stem}.archive")), cp311_bytes).unwrap();
        let cp311_pkg = LockedPackage {
            name: "pydantic_core".to_string(),
            version: "2.27.1".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: cp311_sha,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };
        std::fs::write(
            v2_dir.join(format!("{cp311_stem}.meta.json")),
            serde_json::to_string_pretty(&cp311_pkg).unwrap(),
        )
        .unwrap();

        assert!(check_cached_pypi_artifact(
            &pkg_dir,
            "pydantic_core",
            "2.27.1",
            "pypi:pydantic_core",
            Some("macos-arm64"),
            Some("cp312"),
        )
        .is_none());
        assert!(check_cached_pypi_artifact(
            &pkg_dir,
            "pydantic_core",
            "2.27.1",
            "pypi:pydantic_core",
            Some("macos-arm64"),
            Some("cp311"),
        )
        .is_some());

        // Add cp312 wheel alongside cp311 cleanly: both coexist and resolve independently.
        let cp312_bytes = b"PK\x03\x04cp312-wheel";
        let cp312_sha =
            crate::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(cp312_bytes));
        let cp312_stem = "pydantic_core-2.27.1-cp312-cp312-macosx_11_0_arm64";
        std::fs::write(v2_dir.join(format!("{cp312_stem}.archive")), cp312_bytes).unwrap();
        let cp312_pkg = LockedPackage {
            sha256: cp312_sha.clone(),
            ..cp311_pkg
        };
        std::fs::write(
            v2_dir.join(format!("{cp312_stem}.meta.json")),
            serde_json::to_string_pretty(&cp312_pkg).unwrap(),
        )
        .unwrap();

        let hit_312 = check_cached_pypi_artifact(
            &pkg_dir,
            "pydantic_core",
            "2.27.1",
            "pypi:pydantic_core",
            Some("macos-arm64"),
            Some("cp312"),
        )
        .expect("cp312 lookup should hit immutable v2 entry");
        assert_eq!(hit_312.1.sha256, cp312_sha);
    }
}
