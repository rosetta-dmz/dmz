use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    emit_non_relocatable_notice, is_non_standalone_path, push_system_bin_candidates,
    push_unique_path, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers standalone Ninja build tool distributions.
#[derive(Debug, Default, Clone)]
pub struct NinjaDiscoverer;

impl ToolchainDiscoverer for NinjaDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Ninja
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Ok(root) = std::env::var("NINJA_HOME") {
            let p = PathBuf::from(root);
            if p.join("bin/ninja").exists() {
                push_unique_path(&mut candidates, p.join("bin/ninja"));
            } else if p.join("ninja").exists() {
                push_unique_path(&mut candidates, p.join("ninja"));
            }
        }

        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            push_unique_path(&mut candidates, home_path.join(".local/bin/ninja"));
            push_unique_path(&mut candidates, home_path.join(".ninja/bin/ninja"));
            push_unique_path(&mut candidates, home_path.join(".cargo/bin/ninja"));
        }

        push_unique_path(&mut candidates, PathBuf::from("/opt/ninja/bin/ninja"));
        push_unique_path(
            &mut candidates,
            PathBuf::from("C:\\Program Files\\Ninja\\ninja.exe"),
        );

        push_system_bin_candidates(&mut candidates, "ninja");

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_ninja: Option<PathBuf> = None;
        let mut chosen_version: Option<String> = None;

        for candidate in candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }

            if let Ok(output) = Command::new(&candidate).arg("--version").output() {
                if output.status.success() {
                    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let is_non_standalone = is_non_standalone_path(&candidate);
                    if chosen_ninja.is_none()
                        || (!is_non_standalone
                            && is_non_standalone_path(chosen_ninja.as_ref().unwrap()))
                    {
                        chosen_ninja = Some(candidate);
                        chosen_version = Some(format!("ninja {}", version_str));
                        if !is_non_standalone {
                            break;
                        }
                    }
                }
            }
        }

        let ninja_bin = match chosen_ninja {
            Some(bin) => bin,
            None => return Ok(None),
        };
        let version = chosen_version.unwrap_or_else(|| "ninja".to_string());
        let root_dir = ninja_bin.parent().unwrap_or(Path::new("")).to_path_buf();

        info!(
            ninja = ?ninja_bin,
            version = %version,
            "Discovered standalone Ninja distribution"
        );

        let bin_name = ninja_bin
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("ninja")
            .to_string();

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ninja,
            version,
            root_dir,
            bin_files: vec![(ninja_bin, format!("bin/{}", bin_name))],
            lib_files: vec![],
        }))
    }
}
