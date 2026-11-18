use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, is_macos_usr_bin_stub, push_system_bin_candidates, push_unique_path,
    redact_home_prefix, resolve_prefix_root, select_strict_standalone_verified,
    ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Discovers standalone pkg-config / pkgconf distributions.
#[derive(Debug, Default, Clone)]
pub struct PkgConfigDiscoverer;

impl ToolchainDiscoverer for PkgConfigDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::PkgConfig
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_pkgconfig_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((pkg_bin, version)) =
            select_strict_standalone_verified(candidates, probe_pkgconfig_version)
        else {
            return Ok(None);
        };

        let Some(root_dir) = resolve_prefix_root(&pkg_bin) else {
            return Ok(None);
        };

        let redacted_bin = redact_home_prefix(&pkg_bin);
        let redacted_root = redact_home_prefix(&root_dir);
        info!(
            pkg_config = %redacted_bin,
            root = %redacted_root,
            version = %version,
            "Discovered standalone pkg-config distribution"
        );

        let mut lib_files = Vec::new();
        for sub in &["lib/pkgconfig", "share/pkgconfig", "share/aclocal"] {
            let p = root_dir.join(sub);
            if p.is_dir() {
                collect_files_recursive(&p, &root_dir, &mut lib_files, "");
            }
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::PkgConfig,
            version,
            root_dir,
            bin_files: vec![
                (pkg_bin.clone(), "bin/pkg-config".to_string()),
                (pkg_bin, "bin/pkgconf".to_string()),
            ],
            lib_files,
        }))
    }
}

fn collect_pkgconfig_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(root) = std::env::var_os("PKG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(root);
        push_unique_path(&mut candidates, p.join("bin/pkg-config"));
        push_unique_path(&mut candidates, p.join("bin/pkgconf"));
        push_unique_path(&mut candidates, p.join("pkg-config"));
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        push_unique_path(&mut candidates, home_path.join(".local/bin/pkgconf"));
        push_unique_path(&mut candidates, home_path.join(".local/bin/pkg-config"));
    }

    push_unique_path(&mut candidates, PathBuf::from("/opt/pkgconf/bin/pkgconf"));
    push_unique_path(
        &mut candidates,
        PathBuf::from("/opt/pkg-config/bin/pkg-config"),
    );

    push_system_bin_candidates(&mut candidates, "pkgconf");
    push_system_bin_candidates(&mut candidates, "pkg-config");

    candidates.retain(|c| c.is_file());
    candidates
}

fn probe_pkgconfig_version(candidate: &Path) -> Option<String> {
    if is_macos_usr_bin_stub(candidate) || !candidate.is_file() {
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

    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(format!("pkg-config {}", version_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_pkgconfig_collects_pc_dirs() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("fake_pkgconfig");
        let lib_pc = root.join("lib/pkgconfig");
        let share_pc = root.join("share/pkgconfig");

        fs::create_dir_all(&lib_pc).unwrap();
        fs::create_dir_all(&share_pc).unwrap();

        fs::write(lib_pc.join("zlib.pc"), "Name: zlib").unwrap();
        fs::write(share_pc.join("openssl.pc"), "Name: openssl").unwrap();

        let mut lib_files = Vec::new();
        for sub in &["lib/pkgconfig", "share/pkgconfig", "share/aclocal"] {
            let p = root.join(sub);
            if p.is_dir() {
                collect_files_recursive(&p, &root, &mut lib_files, "");
            }
        }

        let rel_paths: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(rel_paths.contains(&"lib/pkgconfig/zlib.pc".to_string()));
        assert!(rel_paths.contains(&"share/pkgconfig/openssl.pc".to_string()));
    }

    #[test]
    fn test_pkgconfig_discovers_standalone_when_present() {
        let discoverer = PkgConfigDiscoverer;
        if let Ok(Some(tc)) = discoverer.discover() {
            assert_eq!(tc.ecosystem, ToolchainEcosystem::PkgConfig);
            // If ~/.local/bin/pkgconf is installed, it should be selected and not be non-standalone
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
            {
                let local_pkgconf = std::path::PathBuf::from(home).join(".local/bin/pkgconf");
                if local_pkgconf.exists() {
                    assert!(!super::super::is_non_standalone_path(&tc.root_dir));
                }
            }
        }
    }

    #[test]
    fn test_pkgconfig_prefix_resolution_and_homebrew_rejection() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/pkg-config"),
            PathBuf::from("/usr/local/opt/pkgconf/bin/pkgconf"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("pkg-config 2.2.0".to_string())),
            None
        );

        let temp = tempdir().unwrap();
        let prefix = temp.path().join("pkgprefix");
        let bin = prefix.join("bin");
        let pc = prefix.join("lib/pkgconfig");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&pc).unwrap();
        let pkg_bin = bin.join("pkg-config");
        fs::write(&pkg_bin, "pkg").unwrap();

        let resolved = resolve_prefix_root(&pkg_bin).unwrap();
        assert_eq!(resolved, fs::canonicalize(&prefix).unwrap());
        assert!(resolved.join("lib/pkgconfig").is_dir());
    }
}
