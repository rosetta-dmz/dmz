//! Single package fetch orchestrator coordinating cache validation, URL resolution, and streamed transport.

use crate::resolver::fetch::cache::check_cached_artifact;
use crate::resolver::fetch::ecosystems::resolve_download_url;
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

    let mut clean_pkg_name = pkg.name.clone();
    if eco_str == "pypi" || eco_str == "python" {
        if let Some((n, v)) = ManifestScanner::parse_pep508_requirement(&clean_pkg_name) {
            clean_pkg_name = n;
            if (clean_version == "*" || clean_version.is_empty()) && v != "*" {
                clean_version = v;
            }
        }
    }

    let safe_pkg_name = clean_pkg_name.replace('/', "__");
    let pkg_dir = store_path.join(&safe_pkg_name);
    std::fs::create_dir_all(&pkg_dir).map_err(|e| {
        ResolverError::Io(format!(
            "Failed to create package directory at {:?}: {}",
            pkg_dir, e
        ))
    })?;

    // Fast-path: check if a verified artifact already exists in the store cache
    if let Some(cached) = check_cached_artifact(&pkg_dir, &safe_pkg_name, &clean_version, &map_key)
    {
        return Ok(Some(cached));
    }

    let resolved = resolve_download_url(
        client,
        &pkg,
        &clean_pkg_name,
        clean_version,
        target_platform,
        crates_api_base,
        crates_dl_base,
        npm_base,
        pypi_base,
        golang_proxy_base,
    )?;

    let artifact_filename = format!("{}-{}.archive", safe_pkg_name, resolved.concrete_version);
    let artifact_path = pkg_dir.join(&artifact_filename);
    let tmp_path = pkg_dir.join(format!("{}.download.tmp", artifact_filename));

    let computed_hash = stream_download_with_retry(
        client,
        &resolved.download_url,
        &tmp_path,
        &artifact_path,
        &pkg.name,
    )?;

    let mut updated_pkg = pkg;
    updated_pkg.name = clean_pkg_name;
    updated_pkg.version = resolved.concrete_version.clone();
    updated_pkg.sha256 = computed_hash;
    if !resolved.requires_dist.is_empty() && updated_pkg.dependencies.is_empty() {
        updated_pkg.dependencies = resolved.requires_dist;
    }

    let meta_path = pkg_dir.join("metadata.json");
    let meta_data = serde_json::to_string_pretty(&updated_pkg).unwrap_or_default();
    crate::workspace::atomic_write_file(&meta_path, meta_data).map_err(|e| {
        ResolverError::Io(format!(
            "Failed to write package metadata to {:?}: {}",
            meta_path, e
        ))
    })?;

    info!(
        name = %updated_pkg.name,
        version = %updated_pkg.version,
        "Successfully downloaded, verified, and cached artifact"
    );

    Ok(Some((map_key, updated_pkg)))
}
