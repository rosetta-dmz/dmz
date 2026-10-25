use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    emit_non_relocatable_notice, is_non_standalone_path, push_unique_path, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers standalone Make toolchain distributions.
#[derive(Debug, Default, Clone)]
pub struct MakeDiscoverer;

impl ToolchainDiscoverer for MakeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Make
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Ok(root) = std::env::var("MAKE_HOME") {
            let p = PathBuf::from(root);
            if p.join("bin/make").exists() {
                push_unique_path(&mut candidates, p.join("bin/make"));
            } else if p.join("make").exists() {
                push_unique_path(&mut candidates, p.join("make"));
            }
        }

        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            push_unique_path(&mut candidates, home_path.join(".local/bin/make"));
            push_unique_path(&mut candidates, home_path.join(".local/bin/gmake"));
        }

        push_unique_path(&mut candidates, PathBuf::from("/opt/make/bin/make"));

        if let Ok(p) = which::which("gmake") {
            push_unique_path(&mut candidates, p);
        }
        if let Ok(p) = which::which("make") {
            push_unique_path(&mut candidates, p);
        }

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_make: Option<PathBuf> = None;
        let mut chosen_version: Option<String> = None;

        for candidate in candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }

            if let Ok(output) = Command::new(&candidate).arg("--version").output() {
                if output.status.success() {
                    let version_str = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("make")
                        .trim()
                        .to_string();

                    let is_non_standalone = is_non_standalone_path(&candidate);
                    if chosen_make.is_none()
                        || (!is_non_standalone
                            && is_non_standalone_path(chosen_make.as_ref().unwrap()))
                    {
                        chosen_make = Some(candidate);
                        chosen_version = Some(version_str);
                        if !is_non_standalone {
                            break;
                        }
                    }
                }
            }
        }

        let make_bin = match chosen_make {
            Some(bin) => bin,
            None => return Ok(None),
        };
        let version = chosen_version.unwrap_or_else(|| "make".to_string());
        let root_dir = make_bin.parent().unwrap_or(Path::new("")).to_path_buf();

        info!(
            make = ?make_bin,
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
