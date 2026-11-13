use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, push_system_bin_candidates, push_unique_path,
    select_preferred_verified, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers standalone pkg-config / pkgconf distributions.
#[derive(Debug, Default, Clone)]
pub struct PkgConfigDiscoverer;

impl ToolchainDiscoverer for PkgConfigDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::PkgConfig
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Ok(root) = std::env::var("PKG_CONFIG_HOME") {
            let p = PathBuf::from(root);
            push_unique_path(&mut candidates, p.join("bin/pkg-config"));
            push_unique_path(&mut candidates, p.join("bin/pkgconf"));
            push_unique_path(&mut candidates, p.join("pkg-config"));
        }

        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
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

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((pkg_bin, version)) = select_preferred_verified(candidates, |candidate| {
            let output = Command::new(candidate).arg("--version").output().ok()?;
            if !output.status.success() {
                return None;
            }
            let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Some(format!("pkg-config {}", version_str))
        }) else {
            return Ok(None);
        };
        let root_dir = pkg_bin.parent().unwrap_or(Path::new("")).to_path_buf();

        info!(
            pkg_config = ?pkg_bin,
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
}
