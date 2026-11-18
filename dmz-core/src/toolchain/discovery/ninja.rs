use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    is_macos_usr_bin_stub, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Discovers standalone Ninja build tool distributions.
#[derive(Debug, Default, Clone)]
pub struct NinjaDiscoverer;

impl ToolchainDiscoverer for NinjaDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Ninja
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_ninja_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((ninja_bin, version)) =
            select_strict_standalone_verified(candidates, probe_ninja_version)
        else {
            return Ok(None);
        };

        let Some(root_dir) = resolve_prefix_root(&ninja_bin) else {
            return Ok(None);
        };

        let redacted_bin = redact_home_prefix(&ninja_bin);
        info!(
            ninja = %redacted_bin,
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

fn collect_ninja_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(root) = std::env::var_os("NINJA_HOME").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(root);
        if p.join("bin/ninja").is_file() {
            push_unique_path(&mut candidates, p.join("bin/ninja"));
        } else if p.join("ninja").is_file() {
            push_unique_path(&mut candidates, p.join("ninja"));
        }
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
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
    candidates.retain(|c| c.is_file());
    candidates
}

fn probe_ninja_version(candidate: &Path) -> Option<String> {
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
    Some(format!("ninja {}", version_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ninja_rejects_homebrew_and_root_fallback() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/ninja"),
            PathBuf::from("/home/linuxbrew/.linuxbrew/bin/ninja"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("ninja 1.12.1".to_string())),
            None
        );
        if cfg!(target_os = "macos") {
            assert!(is_macos_usr_bin_stub(Path::new("/usr/bin/ninja")));
            assert!(probe_ninja_version(Path::new("/usr/bin/ninja")).is_none());
        }
        assert_eq!(resolve_prefix_root(Path::new("/ninja")), None);
    }
}
