use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{emit_non_relocatable_notice, is_non_standalone_path, ToolchainDiscoverer};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Discovers host Ruby runtime and Bundler package manager.
#[derive(Debug, Default, Clone)]
pub struct RubyDiscoverer;

impl ToolchainDiscoverer for RubyDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Ruby
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let ruby_bin = match which::which("ruby") {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        if is_non_standalone_path(&ruby_bin) {
            emit_non_relocatable_notice(&ruby_bin);
        }

        let version = Command::new(&ruby_bin)
            .arg("-v")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "ruby".to_string())
            .trim()
            .to_string();

        let ruby_root = ruby_bin
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut bin_files = Vec::new();
        bin_files.push((ruby_bin, "bin/ruby".into()));

        if let Ok(bundle) = which::which("bundle") {
            bin_files.push((bundle, "bin/bundle".into()));
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ruby,
            version,
            root_dir: ruby_root,
            bin_files,
            lib_files: Vec::new(),
        }))
    }
}
