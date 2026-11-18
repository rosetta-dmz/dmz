//! Python PyPI artifact selection, PEP 425 ABI/platform evaluation, and `v2` store key formatting.

use crate::workspace::resolve_target_os_arch;
use tracing::warn;

const MAX_STORE_PLAT_TAG_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WheelTags<'a> {
    py_tags: &'a str,
    abi_tags: &'a str,
    plat_tags: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlatformMatch {
    UniversalAny,
    ExactArch,
    Universal2Mac,
    AltArch,
    ForeignOs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbiMatch {
    ExactCp,
    StableAbi3,
    NoneAbi,
    MismatchedCp,
}

fn strip_url_basename(raw: &str) -> &str {
    let without_frag = raw.split('#').next().unwrap_or(raw);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    without_query.rsplit('/').next().unwrap_or(without_query)
}

fn is_valid_py_tag_token(tok: &str) -> bool {
    tok.starts_with("py")
        || tok.starts_with("cp")
        || tok.starts_with("pp")
        || tok.starts_with("ip")
        || tok.starts_with("jy")
}

fn is_valid_abi_tag_token(tok: &str) -> bool {
    tok == "none"
        || tok == "abi3"
        || tok == "abi3t"
        || tok.starts_with("cp")
        || tok.starts_with("pypy")
}

fn parse_wheel_tags(filename_or_url: &str) -> Option<WheelTags<'_>> {
    let base = strip_url_basename(filename_or_url);
    let stem = base
        .strip_suffix(".whl")
        .or_else(|| base.strip_suffix(".archive"))?;
    let mut rev = stem.rsplit('-');
    let plat_tags = rev.next()?;
    let abi_tags = rev.next()?;
    let py_tags = rev.next()?;
    // Must still have at least distribution(-version) segments before py_tags
    let _rest = rev.next()?;
    if py_tags.is_empty() || abi_tags.is_empty() || plat_tags.is_empty() {
        return None;
    }
    if !py_tags.split('.').any(is_valid_py_tag_token)
        || !abi_tags.split('.').any(is_valid_abi_tag_token)
    {
        return None;
    }
    Some(WheelTags {
        py_tags,
        abi_tags,
        plat_tags,
    })
}

fn parse_cp_minor(tag: &str) -> Option<(u8, bool)> {
    let lower = tag.trim().to_ascii_lowercase();
    let rest = lower.strip_prefix("cp3")?;
    let (digits, freethreaded) = match rest.strip_suffix('t') {
        Some(d) => (d, true),
        None => (rest, false),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((digits.parse::<u8>().ok()?, freethreaded))
}

fn is_windows_wheel_plat(plat_lower: &str) -> bool {
    plat_lower
        .split('.')
        .any(|p| p == "win32" || p == "win_amd64" || p == "win_arm64" || p.starts_with("win_"))
}

fn classify_platform(plat_tags_lower: &str, target_os: &str, target_arch: &str) -> PlatformMatch {
    if plat_tags_lower.split('.').any(|p| p == "any") {
        return PlatformMatch::UniversalAny;
    }

    let is_macos = target_os == "macos";
    let is_linux = target_os == "linux";
    let is_windows = target_os == "windows";
    let is_arm64 = target_arch == "aarch64";
    let is_x86_64 = target_arch == "x86_64";

    if is_macos {
        if plat_tags_lower
            .split('.')
            .any(|p| p.contains("macosx") || p.contains("darwin"))
        {
            let has_arm64 = plat_tags_lower
                .split('.')
                .any(|p| p.ends_with("_arm64") || p.contains("aarch64"));
            let has_x86_64 = plat_tags_lower.split('.').any(|p| p.ends_with("_x86_64"));
            if (is_arm64 && has_arm64) || (is_x86_64 && has_x86_64) {
                return PlatformMatch::ExactArch;
            }
            if plat_tags_lower.split('.').any(|p| p.contains("universal2")) {
                return PlatformMatch::Universal2Mac;
            }
            return PlatformMatch::AltArch;
        }
        return PlatformMatch::ForeignOs;
    }

    if is_linux {
        if plat_tags_lower
            .split('.')
            .any(|p| p.contains("manylinux") || p.contains("musllinux") || p.contains("linux"))
        {
            let has_x86_64 = plat_tags_lower
                .split('.')
                .any(|p| p.ends_with("_x86_64") || p.ends_with("_amd64"));
            let has_arm64 = plat_tags_lower
                .split('.')
                .any(|p| p.ends_with("_aarch64") || p.ends_with("_arm64"));
            if (is_x86_64 && has_x86_64) || (is_arm64 && has_arm64) {
                return PlatformMatch::ExactArch;
            }
            return PlatformMatch::AltArch;
        }
        return PlatformMatch::ForeignOs;
    }

    if is_windows {
        if is_windows_wheel_plat(plat_tags_lower) {
            let has_amd64 = plat_tags_lower
                .split('.')
                .any(|p| p == "win_amd64" || p.contains("amd64"));
            let has_arm64 = plat_tags_lower
                .split('.')
                .any(|p| p == "win_arm64" || p.contains("arm64"));
            if (is_x86_64 && has_amd64) || (is_arm64 && has_arm64) {
                return PlatformMatch::ExactArch;
            }
            return PlatformMatch::AltArch;
        }
        return PlatformMatch::ForeignOs;
    }

    PlatformMatch::ForeignOs
}

fn classify_abi(
    py_tags_lower: &str,
    abi_tags_lower: &str,
    desired_cp: Option<(u8, bool)>,
) -> AbiMatch {
    if abi_tags_lower.split('.').any(|a| a == "none") {
        if py_tags_lower
            .split('.')
            .any(|p| p.starts_with("py3") || p.starts_with("py2") || p.starts_with("cp3"))
        {
            return AbiMatch::NoneAbi;
        }
        return AbiMatch::MismatchedCp;
    }

    let target_cp = desired_cp.unwrap_or((11, false));
    let exact_abi = abi_tags_lower
        .split('.')
        .any(|a| parse_cp_minor(a) == Some(target_cp));
    let exact_py = py_tags_lower
        .split('.')
        .any(|p| parse_cp_minor(p) == Some(target_cp) || p == "py3");
    if exact_abi && exact_py {
        return AbiMatch::ExactCp;
    }

    if abi_tags_lower.split('.').any(|a| a == "abi3") {
        let min_cp_ok = py_tags_lower.split('.').any(|p| {
            p == "py3" || parse_cp_minor(p).is_some_and(|(m, ft)| !ft && m <= target_cp.0)
        });
        if min_cp_ok {
            return AbiMatch::StableAbi3;
        }
    }

    // When no explicit preference is given, keep fallback ordering over alternate cp3X tags
    if desired_cp.is_none()
        && abi_tags_lower
            .split('.')
            .any(|a| parse_cp_minor(a).is_some())
    {
        return AbiMatch::MismatchedCp;
    }

    AbiMatch::MismatchedCp
}

fn score_wheel_tags(
    tags: &WheelTags<'_>,
    target_os: &str,
    target_arch: &str,
    desired_cp: Option<(u8, bool)>,
) -> Option<(i32, bool)> {
    let py_lower = tags.py_tags.to_ascii_lowercase();
    let abi_lower = tags.abi_tags.to_ascii_lowercase();
    let plat_lower = tags.plat_tags.to_ascii_lowercase();

    if target_os != "windows" && is_windows_wheel_plat(&plat_lower) {
        return None;
    }

    let plat_match = classify_platform(&plat_lower, target_os, target_arch);
    let abi_match = classify_abi(&py_lower, &abi_lower, desired_cp);

    if plat_match == PlatformMatch::UniversalAny && abi_match == AbiMatch::NoneAbi {
        return Some((800, false));
    }

    // Fallback minor preference bonus when `preferred_cp_tag` is None or comparing cp3X fallbacks
    let fallback_cp_bonus: i32 = if abi_lower.contains("cp311") {
        12
    } else if abi_lower.contains("cp312") {
        8
    } else if abi_lower.contains("cp313") || abi_lower.contains("cp310") {
        4
    } else {
        0
    };

    let arch_affinity: i32 = if (target_arch == "aarch64"
        && (plat_lower.contains("aarch64") || plat_lower.contains("arm64")))
        || (target_arch == "x86_64"
            && (plat_lower.contains("x86_64") || plat_lower.contains("amd64")))
    {
        5
    } else {
        0
    };

    let (score, is_fallback_mismatch) = match (plat_match, abi_match) {
        (PlatformMatch::ExactArch, AbiMatch::ExactCp) => (1100, false),
        (PlatformMatch::Universal2Mac, AbiMatch::ExactCp) => (1080, false),
        (PlatformMatch::ExactArch, AbiMatch::StableAbi3) => (1050, false),
        (PlatformMatch::Universal2Mac, AbiMatch::StableAbi3) => (1030, false),
        (PlatformMatch::ExactArch, AbiMatch::NoneAbi) => (1020, false),
        (PlatformMatch::Universal2Mac, AbiMatch::NoneAbi) => (1000, false),
        (PlatformMatch::AltArch, AbiMatch::ExactCp) => (650, false),
        (PlatformMatch::AltArch, AbiMatch::StableAbi3) => (630, false),
        (PlatformMatch::AltArch, AbiMatch::NoneAbi) => (610, false),
        (PlatformMatch::ExactArch, AbiMatch::MismatchedCp) => (200 + fallback_cp_bonus, true),
        (PlatformMatch::Universal2Mac, AbiMatch::MismatchedCp) => (180 + fallback_cp_bonus, true),
        (PlatformMatch::AltArch, AbiMatch::MismatchedCp) => (150 + fallback_cp_bonus, true),
        (
            PlatformMatch::ForeignOs,
            AbiMatch::ExactCp | AbiMatch::StableAbi3 | AbiMatch::NoneAbi,
        ) => (120 + arch_affinity + fallback_cp_bonus, true),
        (PlatformMatch::ForeignOs, AbiMatch::MismatchedCp) => {
            (90 + arch_affinity + fallback_cp_bonus, true)
        }
        (PlatformMatch::UniversalAny, _) => (100 + fallback_cp_bonus, true),
    };

    Some((score, is_fallback_mismatch))
}

/// Evaluates Python wheel ABI tags and selects the best matching artifact for target platform.
#[must_use]
pub fn select_pypi_artifact(
    urls: &[serde_json::Value],
    target_platform: Option<&str>,
) -> Option<String> {
    select_pypi_artifact_for_python(urls, target_platform, None)
}

/// Evaluates Python wheel ABI tags with an optional preferred CPython tag (e.g. `"cp312"`)
/// so native extension wheels align with the workspace/bundled Python runtime ABI.
#[must_use]
pub fn select_pypi_artifact_for_python(
    urls: &[serde_json::Value],
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
) -> Option<String> {
    select_pypi_artifact_with_digest_for_python(urls, target_platform, preferred_cp_tag)
        .map(|(url, _)| url)
}

fn extract_valid_sha256_digest(entry: &serde_json::Value) -> Option<String> {
    let raw = entry
        .get("digests")
        .and_then(|d| d.get("sha256"))
        .and_then(|s| s.as_str())
        .map(str::trim)?;
    if raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(raw.to_ascii_lowercase())
    } else {
        None
    }
}

/// Evaluates Python wheel ABI tags and returns `(url, optional_expected_sha256)` for supply-chain
/// verification against PyPI JSON `digests.sha256`.
#[must_use]
pub(crate) fn select_pypi_artifact_with_digest_for_python(
    urls: &[serde_json::Value],
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
) -> Option<(String, Option<String>)> {
    let (target_os, target_arch) = resolve_target_os_arch(target_platform);
    let desired_cp = preferred_cp_tag.and_then(parse_cp_minor);

    let mut best_score: i32 = -1;
    let mut best_selection: Option<(String, Option<String>)> = None;
    let mut best_is_mismatch = false;
    let mut best_name = String::new();

    for entry in urls {
        let Some(url_str) = entry.get("url").and_then(|u| u.as_str()) else {
            continue;
        };
        let raw_filename = entry.get("filename").and_then(|f| f.as_str()).unwrap_or("");
        let lower = raw_filename.to_ascii_lowercase();

        if lower.ends_with(".exe") || lower.ends_with(".msi") {
            continue;
        }

        let (score, is_mismatch) = if lower.ends_with(".whl") {
            if let Some(tags) = parse_wheel_tags(&lower) {
                match score_wheel_tags(&tags, target_os, target_arch, desired_cp) {
                    Some(res) => res,
                    None => continue,
                }
            } else {
                if target_os != "windows"
                    && (lower.contains("win_amd64")
                        || lower.contains("win_arm64")
                        || lower.contains("win32")
                        || lower.contains("-windows"))
                {
                    continue;
                }
                (300, false)
            }
        } else if lower.ends_with(".tar.gz") || lower.ends_with(".zip") {
            (400, false)
        } else {
            (10, true)
        };

        if score > best_score {
            best_score = score;
            best_selection = Some((url_str.to_string(), extract_valid_sha256_digest(entry)));
            best_is_mismatch = is_mismatch;
            best_name = raw_filename.to_string();
        }
    }

    if best_is_mismatch && best_selection.is_some() {
        warn!(
            filename = %best_name,
            target_os = %target_os,
            target_arch = %target_arch,
            preferred_cp = ?preferred_cp_tag,
            "Selected fallback PyPI wheel does not strictly match target runtime ABI/platform"
        );
    }

    best_selection
}

fn sanitize_store_tag_segment(raw: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_len));
    for c in raw.chars() {
        if out.len() >= max_len {
            break;
        }
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "any".to_string()
    } else {
        out
    }
}

/// Computes the immutable `v2` store archive filename encoding PEP 425 tags for a resolved PyPI artifact.
#[must_use]
pub(crate) fn pypi_v2_archive_filename(
    safe_pkg_name: &str,
    concrete_version: &str,
    url_or_filename: &str,
) -> String {
    let base = strip_url_basename(url_or_filename);
    if base.to_ascii_lowercase().ends_with(".whl") {
        if let Some(tags) = parse_wheel_tags(base) {
            let py = sanitize_store_tag_segment(tags.py_tags, 24);
            let abi = sanitize_store_tag_segment(tags.abi_tags, 24);
            let plat = sanitize_store_tag_segment(tags.plat_tags, MAX_STORE_PLAT_TAG_LEN);
            return format!("{safe_pkg_name}-{concrete_version}-{py}-{abi}-{plat}.archive");
        }
        return format!("{safe_pkg_name}-{concrete_version}-whl.archive");
    }
    format!("{safe_pkg_name}-{concrete_version}-sdist.archive")
}

/// Returns a deterministic compatibility priority score if the cached `v2` archive filename
/// matches the requested `(target_platform, preferred_cp_tag)` ABI contract; returns `None` otherwise.
#[must_use]
pub(crate) fn cached_pypi_v2_compatibility_score(
    archive_filename: &str,
    target_platform: Option<&str>,
    preferred_cp_tag: Option<&str>,
) -> Option<i32> {
    let Some(tags) = parse_wheel_tags(archive_filename) else {
        // Non-wheel source/test archive in `v2/`
        return Some(100);
    };
    let (target_os, target_arch) = resolve_target_os_arch(target_platform);
    let desired_cp = preferred_cp_tag.and_then(parse_cp_minor);
    let (score, is_mismatch) = score_wheel_tags(&tags, target_os, target_arch, desired_cp)?;
    if is_mismatch {
        return None;
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_pypi_artifact_for_python_prefers_workspace_abi_and_abi3() {
        let urls = vec![
            serde_json::json!({
                "filename": "pydantic_core-2.20.0-cp311-cp311-manylinux_2_17_x86_64.whl",
                "url": "https://example.invalid/cp311.whl"
            }),
            serde_json::json!({
                "filename": "pydantic_core-2.20.0-cp312-cp312-manylinux_2_17_x86_64.whl",
                "url": "https://example.invalid/cp312.whl"
            }),
            serde_json::json!({
                "filename": "pydantic_core-2.20.0-cp313-cp313-manylinux_2_17_x86_64.whl",
                "url": "https://example.invalid/cp313.whl"
            }),
        ];

        assert_eq!(
            select_pypi_artifact(&urls, Some("linux-x86_64")).as_deref(),
            Some("https://example.invalid/cp311.whl")
        );
        assert_eq!(
            select_pypi_artifact_for_python(&urls, Some("linux-x86_64"), Some("cp312")).as_deref(),
            Some("https://example.invalid/cp312.whl")
        );
        assert_eq!(
            select_pypi_artifact_for_python(&urls, Some("linux-x86_64"), Some("cp313")).as_deref(),
            Some("https://example.invalid/cp313.whl")
        );
    }

    #[test]
    fn test_select_pypi_artifact_never_picks_windows_wheels_for_unix_targets() {
        let win_only_urls = vec![
            serde_json::json!({
                "filename": "pywin32-308-cp312-cp312-win_amd64.whl",
                "url": "https://example.invalid/pywin32-win_amd64.whl"
            }),
            serde_json::json!({
                "filename": "pywin32-308-cp312-cp312-win32.whl",
                "url": "https://example.invalid/pywin32-win32.whl"
            }),
            serde_json::json!({
                "filename": "pywin32-308.win-amd64-py3.12.exe",
                "url": "https://example.invalid/pywin32.exe"
            }),
        ];

        assert!(select_pypi_artifact_for_python(
            &win_only_urls,
            Some("macos-arm64"),
            Some("cp312")
        )
        .is_none());
        assert!(select_pypi_artifact_for_python(
            &win_only_urls,
            Some("linux-x86_64"),
            Some("cp312")
        )
        .is_none());
        assert_eq!(
            select_pypi_artifact_for_python(&win_only_urls, Some("windows-x86_64"), Some("cp312"))
                .as_deref(),
            Some("https://example.invalid/pywin32-win_amd64.whl")
        );
    }

    #[test]
    fn test_pep425_tag_selection_prefers_abi3_and_rejects_cross_abi_in_cache() {
        let urls = vec![
            serde_json::json!({
                "filename": "pkg-1.0.0.tar.gz",
                "url": "https://example.invalid/pkg-1.0.0.tar.gz"
            }),
            serde_json::json!({
                "filename": "pkg-1.0.0-cp311-cp311-macosx_11_0_arm64.whl",
                "url": "https://example.invalid/cp311.whl"
            }),
            serde_json::json!({
                "filename": "pkg-1.0.0-cp38-abi3-macosx_11_0_arm64.whl",
                "url": "https://example.invalid/abi3.whl"
            }),
        ];

        assert_eq!(
            select_pypi_artifact_for_python(&urls, Some("macos-arm64"), Some("cp312")).as_deref(),
            Some("https://example.invalid/abi3.whl")
        );

        let cp311_archive = pypi_v2_archive_filename(
            "pydantic_core",
            "2.27.1",
            "https://files.pythonhosted.org/pydantic_core-2.27.1-cp311-cp311-macosx_11_0_arm64.whl",
        );
        let cp312_archive = pypi_v2_archive_filename(
            "pydantic_core",
            "2.27.1",
            "https://files.pythonhosted.org/pydantic_core-2.27.1-cp312-cp312-macosx_11_0_arm64.whl",
        );
        assert_ne!(cp311_archive, cp312_archive);
        assert_eq!(
            cp312_archive,
            "pydantic_core-2.27.1-cp312-cp312-macosx_11_0_arm64.archive"
        );
        assert!(cached_pypi_v2_compatibility_score(
            &cp311_archive,
            Some("macos-arm64"),
            Some("cp312")
        )
        .is_none());
        assert!(cached_pypi_v2_compatibility_score(
            &cp312_archive,
            Some("macos-arm64"),
            Some("cp312")
        )
        .is_some());
    }
}
