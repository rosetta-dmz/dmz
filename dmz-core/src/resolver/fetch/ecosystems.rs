//! Multi-ecosystem registry querying and download URL resolution.

use crate::resolver::fetch::pypi::select_pypi_artifact;
use crate::resolver::lockfile::LockedPackage;
use crate::resolver::ResolverError;

/// Resolved download parameters including the artifact URL, resolved concrete version, and discovered transitive dependencies.
pub struct ResolvedPackageUrl {
    pub download_url: String,
    pub concrete_version: String,
    pub requires_dist: Vec<String>,
}

/// Resolves the concrete download URL and version for a package across Cargo, NPM, PyPI, and Go.
#[allow(clippy::too_many_arguments)]
pub fn resolve_download_url(
    client: &reqwest::blocking::Client,
    pkg: &LockedPackage,
    clean_pkg_name: &str,
    mut clean_version: String,
    target_platform: Option<&str>,
    crates_api_base: &str,
    crates_dl_base: &str,
    npm_base: &str,
    pypi_base: &str,
    golang_proxy_base: &str,
) -> Result<ResolvedPackageUrl, ResolverError> {
    let eco_str = format!("{:?}", pkg.ecosystem).to_lowercase();
    let mut discovered_requires_dist = Vec::new();

    if let Some(ref url) = pkg.source_url {
        return Ok(ResolvedPackageUrl {
            download_url: url.clone(),
            concrete_version: clean_version,
            requires_dist: discovered_requires_dist,
        });
    }

    let download_url = match eco_str.as_str() {
        "cargo" | "rust" => {
            resolve_cargo_url(
                client,
                crates_api_base,
                crates_dl_base,
                pkg,
                &mut clean_version,
            );
            format!(
                "{}/{}/{}-{}.crate",
                crates_dl_base.trim_end_matches('/'),
                pkg.name,
                pkg.name,
                clean_version
            )
        }
        "npm" | "node" => resolve_npm_url(client, npm_base, pkg, &mut clean_version),
        "pypi" | "python" => resolve_pypi_url(
            client,
            pypi_base,
            pkg,
            clean_pkg_name,
            &mut clean_version,
            target_platform,
            &mut discovered_requires_dist,
        ),
        "go" | "golang" => {
            format!(
                "{}/{}/@v/{}.zip",
                golang_proxy_base.trim_end_matches('/'),
                pkg.name,
                clean_version
            )
        }
        _ => {
            return Err(ResolverError::Parse(format!(
                "No source URL and unsupported ecosystem '{:?}' for package '{}'",
                pkg.ecosystem, pkg.name
            )));
        }
    };

    Ok(ResolvedPackageUrl {
        download_url,
        concrete_version: clean_version,
        requires_dist: discovered_requires_dist,
    })
}

fn resolve_cargo_url(
    client: &reqwest::blocking::Client,
    crates_api_base: &str,
    _crates_dl_base: &str,
    pkg: &LockedPackage,
    clean_version: &mut String,
) {
    if !clean_version.contains('.') || clean_version.matches('.').count() < 2 {
        let meta_url = format!("{}/{}", crates_api_base.trim_end_matches('/'), pkg.name);
        if let Ok(resp) = client.get(&meta_url).send() {
            if resp.status().is_success() {
                if let Ok(text) = resp.text() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(versions) = json.get("versions").and_then(|v| v.as_array()) {
                            for v in versions {
                                if let Some(num) = v.get("num").and_then(|n| n.as_str()) {
                                    if num.starts_with(clean_version.as_str())
                                        || *clean_version == "*"
                                    {
                                        *clean_version = num.to_string();
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn resolve_npm_url(
    client: &reqwest::blocking::Client,
    npm_base: &str,
    pkg: &LockedPackage,
    clean_version: &mut String,
) -> String {
    let encoded_name = pkg.name.replace('/', "%2F");
    let meta_url = format!("{}/{}", npm_base.trim_end_matches('/'), encoded_name);
    let mut resolved_tarball = String::new();

    if let Ok(resp) = client.get(&meta_url).send() {
        if resp.status().is_success() {
            if let Ok(text) = resp.text() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    let versions_obj = json.get("versions").and_then(|v| v.as_object());
                    if let Some(versions) = versions_obj {
                        if !versions.contains_key(clean_version.as_str()) {
                            if *clean_version == "*" || *clean_version == "latest" {
                                if let Some(latest) = json
                                    .get("dist-tags")
                                    .and_then(|dt| dt.get("latest"))
                                    .and_then(|v| v.as_str())
                                {
                                    *clean_version = latest.to_string();
                                }
                            } else if let Some(matched_ver) = versions
                                .keys()
                                .rev()
                                .find(|k| k.starts_with(clean_version.as_str()))
                            {
                                *clean_version = matched_ver.clone();
                            }
                        }
                    }
                    if let Some(tarball) = json
                        .get("versions")
                        .and_then(|v| v.get(clean_version.as_str()))
                        .and_then(|ver| ver.get("dist"))
                        .and_then(|d| d.get("tarball"))
                        .and_then(|t| t.as_str())
                    {
                        resolved_tarball = tarball.to_string();
                    }
                }
            }
        }
    }

    if resolved_tarball.is_empty() {
        let npm_root = npm_base.trim_end_matches('/');
        if pkg.name.starts_with('@') {
            if let Some(idx) = pkg.name.find('/') {
                let scope = pkg.name.get(..idx).unwrap_or("");
                let short_name = pkg.name.get(idx.saturating_add(1)..).unwrap_or("");
                format!(
                    "{}/{}%2F{}/-/{}-{}.tgz",
                    npm_root, scope, short_name, short_name, clean_version
                )
            } else {
                format!(
                    "{}/{}/-/{}-{}.tgz",
                    npm_root, pkg.name, pkg.name, clean_version
                )
            }
        } else {
            format!(
                "{}/{}/-/{}-{}.tgz",
                npm_root, pkg.name, pkg.name, clean_version
            )
        }
    } else {
        resolved_tarball
    }
}

fn resolve_pypi_url(
    client: &reqwest::blocking::Client,
    pypi_base: &str,
    _pkg: &LockedPackage,
    clean_pkg_name: &str,
    clean_version: &mut String,
    target_platform: Option<&str>,
    discovered_requires_dist: &mut Vec<String>,
) -> String {
    let mut resolved_url = String::new();
    let pypi_root = pypi_base.trim_end_matches('/');

    let candidate_urls = if *clean_version == "*" || clean_version.is_empty() {
        vec![format!("{}/{}/json", pypi_root, clean_pkg_name)]
    } else {
        vec![
            format!("{}/{}/{}/json", pypi_root, clean_pkg_name, clean_version),
            format!("{}/{}/json", pypi_root, clean_pkg_name),
        ]
    };

    for pypi_url in candidate_urls {
        if let Ok(resp) = client.get(&pypi_url).send() {
            if resp.status().is_success() {
                if let Ok(text) = resp.text() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(info_ver) = json
                            .get("info")
                            .and_then(|i| i.get("version"))
                            .and_then(|v| v.as_str())
                        {
                            *clean_version = info_ver.to_string();
                        }
                        if let Some(reqs) = json
                            .get("info")
                            .and_then(|i| i.get("requires_dist"))
                            .and_then(|r| r.as_array())
                        {
                            for req_val in reqs {
                                if let Some(req_str) = req_val.as_str() {
                                    discovered_requires_dist.push(req_str.to_string());
                                }
                            }
                        }
                        if let Some(urls) = json.get("urls").and_then(|u| u.as_array()) {
                            if let Some(best) = select_pypi_artifact(urls, target_platform) {
                                resolved_url = best;
                                break;
                            } else if let Some(first_entry) = urls.first() {
                                if let Some(u) = first_entry.get("url").and_then(|val| val.as_str())
                                {
                                    resolved_url = u.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if resolved_url.is_empty() {
        let first_char = clean_pkg_name
            .chars()
            .next()
            .map(|c| c.to_ascii_lowercase())
            .unwrap_or('a');
        format!(
            "https://files.pythonhosted.org/packages/source/{}/{}/{}-{}.tar.gz",
            first_char, clean_pkg_name, clean_pkg_name, clean_version
        )
    } else {
        resolved_url
    }
}
