//! Single package fetch orchestrator coordinating cache validation, URL resolution, and streamed transport.

use crate::resolver::fetch::cache::{check_cached_artifact, check_cached_pypi_artifact};
use crate::resolver::fetch::ecosystems::resolve_download_url_with_python_tag;
use crate::resolver::fetch::pypi::pypi_v2_archive_filename;
use crate::resolver::fetch::stream::stream_download_with_retry;
use crate::resolver::lockfile::LockedPackage;
use crate::resolver::manifest::ManifestScanner;
use crate::resolver::ResolverError;
use std::path::Path;
use tracing::info;

/// Fetches, verifies, and stores an individual package artifact into the local store cache.
#[allow(clippy::too_many_arguments)]
pub fn fetch_single_package(
    client: &reqwest::blocking::Client,
    store_path: &Path,
    crates_api_base: &str,
    crates_dl_base: &str,
    npm_base: &str,
    pypi_base: &str,
    golang_proxy_base: &str,
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
    map_key: String,
    pkg: LockedPackage,
) -> Result<Option<(String, LockedPackage)>, ResolverError> {
    if pkg.is_local_path() {
        info!(
            name = %pkg.name,
            version = %pkg.version,
            "Skipping remote download for local path dependency/workspace member"
        );
        return Ok(None);
    }

    info!(
        name = %pkg.name,
        version_req = %pkg.version,
        "Resolving concrete version and downloading package artifact"
    );

    let eco_str = format!("{:?}", pkg.ecosystem).to_lowercase();
    let mut clean_version = pkg
        .version
        .trim_start_matches('=')
        .trim_start_matches('^')
        .trim_start_matches('~')
        .trim()
        .to_string();

    let is_pypi = eco_str == "pypi" || eco_str == "python";
    let mut clean_pkg_name = pkg.name.clone();
    if is_pypi {
        if let Some((n, v)) = ManifestScanner::parse_pep508_requirement(&clean_pkg_name) {
            clean_pkg_name = n;
            if (clean_version == "*" || clean_version.is_empty()) && v != "*" {
                clean_version = v;
            }
        }
    }

    let safe_pkg_name = if is_pypi {
        clean_pkg_name.replace('/', "__").to_ascii_lowercase()
    } else {
        clean_pkg_name.replace('/', "__")
    };
    let pkg_dir = store_path.join(&safe_pkg_name);
    std::fs::create_dir_all(&pkg_dir).map_err(|e| {
        ResolverError::Io(format!(
            "Failed to create package directory at {:?}: {}",
            pkg_dir, e
        ))
    })?;

    // Fast-path: check if a verified artifact already exists in the store cache
    let cached_hit = if is_pypi {
        check_cached_pypi_artifact(
            &pkg_dir,
            &safe_pkg_name,
            &clean_version,
            &map_key,
            target_platform,
            preferred_cp_tag,
        )
    } else {
        check_cached_artifact(&pkg_dir, &safe_pkg_name, &clean_version, &map_key)
    };
    if let Some((cached_key, mut cached_pkg)) = cached_hit {
        if pkg.member_id.is_some() {
            cached_pkg.member_id = pkg.member_id.clone();
        }
        if cached_pkg.dependencies.is_empty() && !pkg.dependencies.is_empty() {
            cached_pkg.dependencies = pkg.dependencies.clone();
        }
        return Ok(Some((cached_key, cached_pkg)));
    }

    let resolved = resolve_download_url_with_python_tag(
        client,
        &pkg,
        &clean_pkg_name,
        clean_version,
        target_platform,
        preferred_cp_tag,
        crates_api_base,
        crates_dl_base,
        npm_base,
        pypi_base,
        golang_proxy_base,
    )?;

    let (target_dir, artifact_filename, meta_path) = if is_pypi {
        let v2_dir = pkg_dir.join("v2");
        std::fs::create_dir_all(&v2_dir).map_err(|e| {
            ResolverError::Io(format!(
                "Failed to create PyPI v2 store directory at {:?}: {}",
                v2_dir, e
            ))
        })?;
        let fname = pypi_v2_archive_filename(
            &safe_pkg_name,
            &resolved.concrete_version,
            &resolved.download_url,
        );
        let stem = fname.strip_suffix(".archive").unwrap_or(&fname);
        let m_path = v2_dir.join(format!("{stem}.meta.json"));
        (v2_dir, fname, m_path)
    } else {
        let fname = format!("{}-{}.archive", safe_pkg_name, resolved.concrete_version);
        let m_path = pkg_dir.join("metadata.json");
        (pkg_dir.clone(), fname, m_path)
    };

    let artifact_path = target_dir.join(&artifact_filename);
    let temp_guard = tempfile::Builder::new()
        .prefix(&format!(".{}.download.", artifact_filename))
        .tempfile_in(&target_dir)
        .map_err(|e| {
            ResolverError::Io(format!(
                "Failed to allocate unique temporary download file in {:?}: {}",
                target_dir, e
            ))
        })?
        .into_temp_path();

    let computed_hash = stream_download_with_retry(
        client,
        &resolved.download_url,
        &temp_guard,
        &artifact_path,
        &pkg.name,
    )?;

    if let Some(ref expected) = resolved.expected_sha256 {
        if !computed_hash.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(&artifact_path);
            return Err(crate::resolver::lockfile::LockError::IntegrityMismatch {
                expected: expected.clone(),
                actual: computed_hash,
            }
            .into());
        }
    }

    let mut updated_pkg = pkg;
    updated_pkg.name = clean_pkg_name;
    updated_pkg.version = resolved.concrete_version.clone();
    updated_pkg.sha256 = computed_hash;
    if !resolved.requires_dist.is_empty() && updated_pkg.dependencies.is_empty() {
        updated_pkg.dependencies = resolved.requires_dist;
    }

    let meta_data = serde_json::to_string_pretty(&updated_pkg).map_err(|e| {
        ResolverError::Io(format!(
            "Failed to serialize metadata for {}: {}",
            updated_pkg.name, e
        ))
    })?;
    crate::install::fast_clone::FastClone::prepare_destination(&meta_path).map_err(|e| {
        ResolverError::Io(format!(
            "Failed to prepare metadata path {:?}: {}",
            meta_path, e
        ))
    })?;
    crate::workspace::atomic_write_file(&meta_path, meta_data).map_err(|e| {
        ResolverError::Io(format!(
            "Failed to write package metadata to {:?}: {}",
            meta_path, e
        ))
    })?;

    crate::resolver::fetch::cache::mark_store_file_readonly(&artifact_path);
    crate::resolver::fetch::cache::mark_store_file_readonly(&meta_path);

    info!(
        name = %updated_pkg.name,
        version = %updated_pkg.version,
        "Successfully downloaded, verified, and cached artifact"
    );

    Ok(Some((map_key, updated_pkg)))
}
