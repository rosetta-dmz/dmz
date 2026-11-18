use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    is_apple_system_toolchain_path, push_system_bin_candidates, push_unique_path,
    redact_home_prefix, resolve_prefix_root, select_strict_standalone_verified,
    ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Discovers standalone Make toolchain distributions.
#[derive(Debug, Default, Clone)]
pub struct MakeDiscoverer;

impl ToolchainDiscoverer for MakeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Make
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_make_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((make_bin, version)) =
            select_strict_standalone_verified(candidates, probe_make_version)
        else {
            return Ok(None);
        };

        let Some(root_dir) = resolve_prefix_root(&make_bin) else {
            return Ok(None);
        };

        let redacted_bin = redact_home_prefix(&make_bin);
        info!(
            make = %redacted_bin,
            version = %version,
            "Discovered standalone Make distribution"
        );

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Make,
            version,
            root_dir,
            bin_files: vec![(make_bin, "bin/make".to_string())],
            lib_files: vec![],
        }))
    }
}

fn collect_make_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(root) = std::env::var_os("MAKE_HOME").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(root);
        for rel in ["bin/gmake", "bin/make", "gmake", "make"] {
            push_unique_path(&mut candidates, p.join(rel));
        }
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        push_unique_path(&mut candidates, home_path.join(".local/bin/gmake"));
        push_unique_path(&mut candidates, home_path.join(".local/bin/make"));
    }

    for dir in ["/opt/make/bin", "/usr/local/make/bin"] {
        let dir_path = Path::new(dir);
        push_unique_path(&mut candidates, dir_path.join("gmake"));
        push_unique_path(&mut candidates, dir_path.join("make"));
    }

    push_system_bin_candidates(&mut candidates, "gmake");
    push_system_bin_candidates(&mut candidates, "make");

    candidates.retain(|c| c.is_file());
    candidates
}

fn probe_make_version(candidate: &Path) -> Option<String> {
    if is_apple_system_toolchain_path(candidate) || !candidate.is_file() {
        return None;
    }

    let output = Command::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("make")
            .trim()
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_rejects_homebrew_stub_and_apple_clt() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/gmake"),
            PathBuf::from("/usr/local/Cellar/make/4.4.1/bin/gmake"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("GNU Make 4.4.1".to_string())),
            None
        );

        let clt_make = Path::new("/Library/Developer/CommandLineTools/usr/bin/make");
        assert!(is_apple_system_toolchain_path(clt_make));
        assert!(probe_make_version(clt_make).is_none());

        let xcode_make = Path::new("/Applications/Xcode.app/Contents/Developer/usr/bin/make");
        assert!(is_apple_system_toolchain_path(xcode_make));
        assert!(probe_make_version(xcode_make).is_none());

        if cfg!(target_os = "macos") {
            assert!(is_apple_system_toolchain_path(Path::new("/usr/bin/make")));
            assert!(probe_make_version(Path::new("/usr/bin/make")).is_none());
        }
        assert_eq!(resolve_prefix_root(Path::new("/make")), None);
    }

    #[test]
    fn test_make_select_prefers_standalone_after_homebrew() {
        let temp = tempfile::tempdir().unwrap();
        let make_bin = temp.path().join("bin/gmake");
        std::fs::create_dir_all(make_bin.parent().unwrap()).unwrap();
        std::fs::write(&make_bin, "").unwrap();

        let candidates = vec![
            PathBuf::from("/opt/homebrew/bin/gmake"),
            PathBuf::from("/Applications/Xcode.app/Contents/Developer/usr/bin/make"),
            make_bin.clone(),
        ];
        let selected = select_strict_standalone_verified(candidates, |p| {
            if p == make_bin {
                Some("GNU Make 4.4.1".to_string())
            } else {
                None
            }
        });
        assert_eq!(selected, Some((make_bin, "GNU Make 4.4.1".to_string())));
    }
}
