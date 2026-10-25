use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{emit_non_relocatable_notice, is_non_standalone_path, ToolchainDiscoverer};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Discovers host PHP runtime and Composer package manager.
#[derive(Debug, Default, Clone)]
pub struct PhpDiscoverer;

impl ToolchainDiscoverer for PhpDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Php
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let php_bin = match which::which("php") {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        if is_non_standalone_path(&php_bin) {
            emit_non_relocatable_notice(&php_bin);
        }

        let version = Command::new(&php_bin)
            .arg("-v")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.to_string()))
            .unwrap_or_else(|| "php".to_string());

        let php_root = php_bin
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut bin_files = Vec::new();
        bin_files.push((php_bin, "bin/php".into()));

        if let Ok(composer) = which::which("composer") {
            bin_files.push((composer, "bin/composer".into()));
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Php,
            version,
            root_dir: php_root,
            bin_files,
            lib_files: Vec::new(),
        }))
    }
}
