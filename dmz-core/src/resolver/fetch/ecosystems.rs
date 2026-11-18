//! Multi-ecosystem registry querying and download URL resolution.

use crate::resolver::fetch::pypi::select_pypi_artifact_with_digest_for_python;
use crate::resolver::lockfile::LockedPackage;
use crate::resolver::ResolverError;

/// Resolved download parameters including the artifact URL, concrete version, expected SHA-256 digest,
/// and discovered transitive dependencies.
pub struct ResolvedPackageUrl {
    pub download_url: String,
    pub concrete_version: String,
    pub expected_sha256: Option<String>,
    pub requires_dist: Vec<String>,
}

/// Resolves the concrete download URL and version for a package across Cargo, NPM, PyPI, and Go.
#[allow(clippy::too_many_arguments)]
pub fn resolve_download_url(
    client: &reqwest::blocking::Client,
    pkg: &LockedPackage,
    clean_pkg_name: &str,
    clean_version: String,
    target_platform: Option<&str>,
    crates_api_base: &str,
    crates_dl_base: &str,
    npm_base: &str,
    pypi_base: &str,
    golang_proxy_base: &str,
) -> Result<ResolvedPackageUrl, ResolverError> {
    resolve_download_url_with_python_tag(
        client,
        pkg,
        clean_pkg_name,
        clean_version,
        target_platform,
        None,
        crates_api_base,
        crates_dl_base,
        npm_base,
        pypi_base,
        golang_proxy_base,
    )
}

/// Resolves the concrete download URL and version with an optional preferred CPython wheel tag (`"cp312"`).
#[allow(clippy::too_many_arguments)]
pub fn resolve_download_url_with_python_tag(
    client: &reqwest::blocking::Client,
    pkg: &LockedPackage,
    clean_pkg_name: &str,
    mut clean_version: String,
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
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
            expected_sha256: None,
            requires_dist: discovered_requires_dist,
        });
    }

    let mut expected_sha256 = None;
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
        "npm" | "node" => resolve_npm_url(
            client,
            npm_base,
            pkg,
            &mut clean_version,
            target_platform,
            &mut discovered_requires_dist,
        ),
        "pypi" | "python" => {
            let (url, digest) = resolve_pypi_url(
                client,
                pypi_base,
                pkg,
                clean_pkg_name,
                &mut clean_version,
                target_platform,
                preferred_cp_tag,
                &mut discovered_requires_dist,
            );
            expected_sha256 = digest;
            url
        }
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
        expected_sha256,
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

#[derive(Debug, Clone, Eq, PartialEq)]
struct NpmSemver {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl NpmSemver {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim().trim_start_matches(['v', 'V', '=']);
        let no_build = trimmed.split('+').next().unwrap_or("");
        let (core, pre) = match no_build.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            _ => (no_build, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse::<u64>().ok()?;
        let minor = parts.next()?.parse::<u64>().ok()?;
        let patch = parts.next()?.parse::<u64>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    fn cmp_ver(&self, other: &Self) -> std::cmp::Ordering {
        match (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)) {
            std::cmp::Ordering::Equal => match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => {
                    let mut a_it = a.split('.');
                    let mut b_it = b.split('.');
                    loop {
                        match (a_it.next(), b_it.next()) {
                            (None, None) => return std::cmp::Ordering::Equal,
                            (None, Some(_)) => return std::cmp::Ordering::Less,
                            (Some(_), None) => return std::cmp::Ordering::Greater,
                            (Some(x), Some(y)) => {
                                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                                    (Err(_), Err(_)) => x.cmp(y),
                                };
                                if ord != std::cmp::Ordering::Equal {
                                    return ord;
                                }
                            }
                        }
                    }
                }
            },
            ord => ord,
        }
    }
}

#[derive(Debug, Clone)]
enum NpmComparatorOp {
    Gte(NpmSemver),
    Gt(NpmSemver),
    Lte(NpmSemver),
    Lt(NpmSemver),
    Eq(NpmSemver),
    Any,
}

impl NpmComparatorOp {
    fn matches(&self, candidate: &NpmSemver) -> bool {
        match self {
            Self::Any => true,
            Self::Gte(v) => candidate.cmp_ver(v) != std::cmp::Ordering::Less,
            Self::Gt(v) => candidate.cmp_ver(v) == std::cmp::Ordering::Greater,
            Self::Lte(v) => candidate.cmp_ver(v) != std::cmp::Ordering::Greater,
            Self::Lt(v) => candidate.cmp_ver(v) == std::cmp::Ordering::Less,
            Self::Eq(v) => candidate.cmp_ver(v) == std::cmp::Ordering::Equal,
        }
    }

    fn pre_tuple(&self) -> Option<(u64, u64, u64)> {
        match self {
            Self::Gte(v) | Self::Gt(v) | Self::Lte(v) | Self::Lt(v) | Self::Eq(v)
                if v.pre.is_some() =>
            {
                Some((v.major, v.minor, v.patch))
            }
            _ => None,
        }
    }
}

struct PartialNpmVersion {
    major: Option<u64>,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Option<String>,
}

fn parse_partial_npm_version(raw: &str) -> Option<PartialNpmVersion> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    let no_build = trimmed.split('+').next().unwrap_or("");
    let (core, pre) = match no_build.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
        _ => (no_build, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let parse_part = |s: Option<&&str>| -> Option<Option<u64>> {
        match s {
            None => Some(None),
            Some(&"*" | &"x" | &"X") => Some(None),
            Some(val) => Some(Some(val.parse::<u64>().ok()?)),
        }
    };
    let major = parse_part(parts.first())?;
    let minor = parse_part(parts.get(1))?;
    let patch = parse_part(parts.get(2))?;
    Some(PartialNpmVersion {
        major,
        minor,
        patch,
        pre,
    })
}

fn zero_pre_bound(major: u64, minor: u64, patch: u64) -> NpmSemver {
    NpmSemver {
        major,
        minor,
        patch,
        pre: Some("0".to_string()),
    }
}

fn expand_npm_comparator_token(token: &str) -> Option<Vec<NpmComparatorOp>> {
    let t = token.trim();
    if t.is_empty() || t == "*" || t.eq_ignore_ascii_case("x") || t == "latest" {
        return Some(vec![NpmComparatorOp::Any]);
    }
    if let Some(rest) = t.strip_prefix('^') {
        let p = parse_partial_npm_version(rest)?;
        let Some(maj) = p.major else {
            return Some(vec![NpmComparatorOp::Any]);
        };
        let lower = NpmSemver {
            major: maj,
            minor: p.minor.unwrap_or(0),
            patch: p.patch.unwrap_or(0),
            pre: p.pre,
        };
        let upper = if maj > 0 {
            zero_pre_bound(maj + 1, 0, 0)
        } else if let Some(min) = p.minor {
            if min > 0 {
                zero_pre_bound(0, min + 1, 0)
            } else if let Some(pat) = p.patch {
                zero_pre_bound(0, 0, pat + 1)
            } else {
                zero_pre_bound(0, 1, 0)
            }
        } else {
            zero_pre_bound(1, 0, 0)
        };
        return Some(vec![
            NpmComparatorOp::Gte(lower),
            NpmComparatorOp::Lt(upper),
        ]);
    }
    if let Some(rest) = t.strip_prefix('~') {
        let p = parse_partial_npm_version(rest)?;
        let Some(maj) = p.major else {
            return Some(vec![NpmComparatorOp::Any]);
        };
        let lower = NpmSemver {
            major: maj,
            minor: p.minor.unwrap_or(0),
            patch: p.patch.unwrap_or(0),
            pre: p.pre,
        };
        let upper = match p.minor {
            Some(min) => zero_pre_bound(maj, min + 1, 0),
            None => zero_pre_bound(maj + 1, 0, 0),
        };
        return Some(vec![
            NpmComparatorOp::Gte(lower),
            NpmComparatorOp::Lt(upper),
        ]);
    }

    let (op_prefix, rest) = if let Some(r) = t.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = t.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = t.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = t.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = t.strip_prefix('=') {
        ("=", r)
    } else {
        ("", t)
    };

    let p = parse_partial_npm_version(rest)?;
    let Some(maj) = p.major else {
        return match op_prefix {
            "" | "=" | ">=" | "<=" => Some(vec![NpmComparatorOp::Any]),
            _ => None,
        };
    };

    let low = NpmSemver {
        major: maj,
        minor: p.minor.unwrap_or(0),
        patch: p.patch.unwrap_or(0),
        pre: p.pre,
    };
    let next_upper = match (p.minor, p.patch) {
        (None, _) => zero_pre_bound(maj + 1, 0, 0),
        (Some(min), None) => zero_pre_bound(maj, min + 1, 0),
        (Some(min), Some(pat)) => zero_pre_bound(maj, min, pat + 1),
    };

    Some(match op_prefix {
        ">=" => vec![NpmComparatorOp::Gte(low)],
        ">" => {
            if p.minor.is_none() || p.patch.is_none() {
                vec![NpmComparatorOp::Gte(next_upper)]
            } else {
                vec![NpmComparatorOp::Gt(low)]
            }
        }
        "<=" => {
            if p.minor.is_none() || p.patch.is_none() {
                vec![NpmComparatorOp::Lt(next_upper)]
            } else {
                vec![NpmComparatorOp::Lte(low)]
            }
        }
        "<" => vec![NpmComparatorOp::Lt(zero_pre_bound(
            low.major, low.minor, low.patch,
        ))],
        "" | "=" => {
            if p.minor.is_none() || p.patch.is_none() {
                vec![NpmComparatorOp::Gte(low), NpmComparatorOp::Lt(next_upper)]
            } else {
                vec![NpmComparatorOp::Eq(low)]
            }
        }
        _ => return None,
    })
}

fn npm_range_matches(range_expr: &str, candidate: &NpmSemver) -> bool {
    for alt in range_expr.split("||") {
        let alt = alt.trim();
        let ops: Vec<NpmComparatorOp> = if let Some((left, right)) = alt.split_once(" - ") {
            let Some(mut lower_ops) = expand_npm_comparator_token(&format!(">={}", left.trim()))
            else {
                continue;
            };
            let Some(upper_ops) = expand_npm_comparator_token(&format!("<={}", right.trim()))
            else {
                continue;
            };
            lower_ops.extend(upper_ops);
            lower_ops
        } else {
            // Merge whitespace between an operator prefix and its version (`>= 1.0.0` -> `>=1.0.0`).
            let mut raw_tokens = alt.split_whitespace();
            let mut merged = Vec::new();
            while let Some(tok) = raw_tokens.next() {
                if matches!(tok, "^" | "~" | ">=" | "<=" | ">" | "<" | "=") {
                    if let Some(next_tok) = raw_tokens.next() {
                        merged.push(format!("{}{}", tok, next_tok));
                        continue;
                    }
                }
                merged.push(tok.to_string());
            }
            let mut parsed = Vec::new();
            let mut valid = true;
            for tok in &merged {
                if let Some(c) = expand_npm_comparator_token(tok) {
                    parsed.extend(c);
                } else {
                    valid = false;
                    break;
                }
            }
            if !valid || parsed.is_empty() {
                continue;
            }
            parsed
        };

        if !ops.iter().all(|op| op.matches(candidate)) {
            continue;
        }
        if candidate.pre.is_some() {
            let allow_pre = ops.iter().any(|op| {
                op.pre_tuple() == Some((candidate.major, candidate.minor, candidate.patch))
            });
            if !allow_pre {
                continue;
            }
        }
        return true;
    }
    false
}

fn select_best_npm_version(
    versions: &serde_json::Map<String, serde_json::Value>,
    dist_tags: Option<&serde_json::Map<String, serde_json::Value>>,
    version_req: &str,
) -> Option<String> {
    let req = version_req.trim();
    let has_range_syntax = req.starts_with('^')
        || req.starts_with('~')
        || req.starts_with('>')
        || req.starts_with('<')
        || req.contains("||")
        || req.contains(" - ")
        || req.contains('*')
        || req.contains(".x")
        || req.contains(".X");

    if !has_range_syntax && versions.contains_key(req) {
        return Some(req.to_string());
    }

    if let Some(tag_ver) = dist_tags
        .and_then(|dt| dt.get(req))
        .and_then(|v| v.as_str())
    {
        if versions.contains_key(tag_ver) {
            return Some(tag_ver.to_string());
        }
    }

    if req.is_empty() || req == "*" || req == "latest" {
        if let Some(latest) = dist_tags
            .and_then(|dt| dt.get("latest"))
            .and_then(|v| v.as_str())
        {
            if versions.contains_key(latest) {
                return Some(latest.to_string());
            }
        }
    }

    let mut parsed_versions: Vec<(&str, NpmSemver)> = versions
        .keys()
        .filter_map(|k| NpmSemver::parse(k).map(|sv| (k.as_str(), sv)))
        .collect();
    parsed_versions.sort_by(|a, b| b.1.cmp_ver(&a.1));

    if req.is_empty() || req == "*" || req == "latest" {
        return parsed_versions
            .iter()
            .find(|(_, sv)| sv.pre.is_none())
            .or_else(|| parsed_versions.first())
            .map(|(k, _)| (*k).to_string());
    }

    if let Some((best, _)) = parsed_versions
        .iter()
        .find(|(_, sv)| npm_range_matches(req, sv))
    {
        return Some((*best).to_string());
    }

    // Fallback: handle ranges that were previously sanitized (`4.17.0` stripped of `^`)
    // when the exact patch isn't present in registry metadata.
    if !has_range_syntax {
        let caret_req = format!("^{}", req.trim_start_matches('='));
        if let Some((best, _)) = parsed_versions
            .iter()
            .find(|(_, sv)| npm_range_matches(&caret_req, sv))
        {
            return Some((*best).to_string());
        }
    }

    parsed_versions
        .iter()
        .find(|(k, _)| k.starts_with(req))
        .map(|(k, _)| (*k).to_string())
}

fn resolve_npm_url(
    client: &reqwest::blocking::Client,
    npm_base: &str,
    pkg: &LockedPackage,
    clean_version: &mut String,
    target_platform: Option<&str>,
    discovered_requires_dist: &mut Vec<String>,
) -> String {
    let encoded_name = pkg.name.replace('/', "%2F");
    let meta_url = format!("{}/{}", npm_base.trim_end_matches('/'), encoded_name);
    let mut resolved_tarball = String::new();

    if let Ok(resp) = client.get(&meta_url).send() {
        if resp.status().is_success() {
            if let Ok(text) = resp.text() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    let dist_tags = json.get("dist-tags").and_then(|dt| dt.as_object());
                    if let Some(versions) = json.get("versions").and_then(|v| v.as_object()) {
                        let requested = if pkg.version.trim().is_empty() {
                            clean_version.as_str()
                        } else {
                            pkg.version.trim()
                        };
                        if let Some(selected) =
                            select_best_npm_version(versions, dist_tags, requested)
                        {
                            *clean_version = selected;
                        } else if !versions.contains_key(clean_version.as_str()) {
                            if let Some(selected) =
                                select_best_npm_version(versions, dist_tags, clean_version.as_str())
                            {
                                *clean_version = selected;
                            }
                        }

                        if let Some(ver_obj) = versions.get(clean_version.as_str()) {
                            if let Some(tarball) = ver_obj
                                .get("dist")
                                .and_then(|d| d.get("tarball"))
                                .and_then(|t| t.as_str())
                            {
                                resolved_tarball = tarball.to_string();
                            }

                            for section in ["dependencies", "optionalDependencies"] {
                                if let Some(deps_map) =
                                    ver_obj.get(section).and_then(|d| d.as_object())
                                {
                                    for (dep_name, dep_ver_val) in deps_map {
                                        if section == "optionalDependencies"
                                            && !crate::resolver::manifest::ManifestScanner::is_compatible_npm_platform_pkg_for_target(
                                                dep_name,
                                                target_platform,
                                            )
                                        {
                                            continue;
                                        }
                                        let raw_ver = dep_ver_val.as_str().unwrap_or("*").trim();
                                        let ver_str =
                                            if raw_ver.is_empty() { "*" } else { raw_ver };
                                        if ver_str.starts_with("file:")
                                            || ver_str.starts_with("workspace:")
                                            || ver_str.starts_with("link:")
                                            || ver_str.starts_with("git+")
                                            || ver_str.contains("://")
                                        {
                                            continue;
                                        }
                                        let entry = format!("{}@{}", dep_name, ver_str);
                                        if !discovered_requires_dist.contains(&entry) {
                                            discovered_requires_dist.push(entry);
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

#[allow(clippy::too_many_arguments)]
fn resolve_pypi_url(
    client: &reqwest::blocking::Client,
    pypi_base: &str,
    _pkg: &LockedPackage,
    clean_pkg_name: &str,
    clean_version: &mut String,
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
    discovered_requires_dist: &mut Vec<String>,
) -> (String, Option<String>) {
    let mut resolved_url = String::new();
    let mut resolved_sha256: Option<String> = None;
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
                            if let Some((best, digest)) =
                                select_pypi_artifact_with_digest_for_python(
                                    urls,
                                    target_platform,
                                    preferred_cp_tag,
                                )
                            {
                                resolved_url = best;
                                resolved_sha256 = digest;
                                break;
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
        (
            format!(
                "https://files.pythonhosted.org/packages/source/{}/{}/{}-{}.tar.gz",
                first_char, clean_pkg_name, clean_pkg_name, clean_version
            ),
            None,
        )
    } else {
        (resolved_url, resolved_sha256)
    }
}
