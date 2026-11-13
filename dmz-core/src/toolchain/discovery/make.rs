use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    push_system_bin_candidates, push_unique_path, select_preferred_verified, ToolchainDiscoverer,
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

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
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

        #[cfg(target_os = "macos")]
        {
            push_unique_path(
                &mut candidates,
                PathBuf::from("/Library/Developer/CommandLineTools/usr/bin/make"),
            );
            push_unique_path(
                &mut candidates,
                PathBuf::from("/Applications/Xcode.app/Contents/Developer/usr/bin/make"),
            );
        }

        push_system_bin_candidates(&mut candidates, "gmake");
        push_system_bin_candidates(&mut candidates, "make");

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((make_bin, version)) = select_preferred_verified(candidates, |candidate| {
            let output = Command::new(candidate).arg("--version").output().ok()?;
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
        }) else {
            return Ok(None);
        };
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
