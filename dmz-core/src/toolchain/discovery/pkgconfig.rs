use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    emit_non_relocatable_notice, is_non_standalone_path, push_unique_path, ToolchainDiscoverer,
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

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
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

        if let Ok(p) = which::which("pkgconf") {
            push_unique_path(&mut candidates, p);
        }
        if let Ok(p) = which::which("pkg-config") {
            push_unique_path(&mut candidates, p);
        }

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_bin: Option<PathBuf> = None;
        let mut chosen_version: Option<String> = None;

        for candidate in candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }

            if let Ok(output) = Command::new(&candidate).arg("--version").output() {
                if output.status.success() {
                    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let is_non_standalone = is_non_standalone_path(&candidate);
                    if chosen_bin.is_none()
                        || (!is_non_standalone
                            && is_non_standalone_path(chosen_bin.as_ref().unwrap()))
                    {
                        chosen_bin = Some(candidate);
                        chosen_version = Some(format!("pkg-config {}", version_str));
                        if !is_non_standalone {
                            break;
                        }
                    }
                }
            }
        }

        let pkg_bin = match chosen_bin {
            Some(bin) => bin,
            None => return Ok(None),
        };
        let version = chosen_version.unwrap_or_else(|| "pkg-config".to_string());
        let root_dir = pkg_bin.parent().unwrap_or(Path::new("")).to_path_buf();

        info!(
            pkg_config = ?pkg_bin,
            version = %version,
            "Discovered standalone pkg-config distribution"
        );

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::PkgConfig,
            version,
            root_dir,
            bin_files: vec![
                (pkg_bin.clone(), "bin/pkg-config".to_string()),
                (pkg_bin, "bin/pkgconf".to_string()),
            ],
            lib_files: vec![],
        }))
    }
}
