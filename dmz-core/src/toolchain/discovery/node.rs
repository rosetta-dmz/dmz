use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path, push_unique_path,
    ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers host Node.js runtime and companion package managers (npm, npx, pnpm, yarn).
#[derive(Debug, Default, Clone)]
pub struct NodeDiscoverer;

impl ToolchainDiscoverer for NodeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Node
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut node_candidates: Vec<PathBuf> = Vec::new();

        // 1. Check official standalone NVM installations
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let nvm_dir = PathBuf::from(&home).join(".nvm/versions/node");
            if let Ok(entries) = fs::read_dir(&nvm_dir) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin/node");
                    if bin.exists() {
                        push_unique_path(&mut node_candidates, bin);
                    }
                }
            }
        }

        // 2. Check standard system locations
        let standard_paths = [
            PathBuf::from("/usr/local/bin/node"),
            PathBuf::from("/usr/bin/node"),
        ];
        for p in &standard_paths {
            if p.exists() {
                push_unique_path(&mut node_candidates, p.clone());
            }
        }

        // 3. Fallback to which node
        if let Ok(p) = which::which("node") {
            push_unique_path(&mut node_candidates, p);
        }

        if node_candidates.is_empty() {
            return Ok(None);
        }

        // Sort candidates prioritizing standalone paths
        node_candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_node: Option<PathBuf> = None;
        for candidate in node_candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }
            if chosen_node.is_none()
                || (!is_non_standalone_path(&candidate)
                    && is_non_standalone_path(chosen_node.as_ref().unwrap()))
            {
                chosen_node = Some(candidate.clone());
                if !is_non_standalone_path(&candidate) {
                    break;
                }
            }
        }

        let node_bin = match chosen_node {
            Some(b) => b,
            None => return Ok(None),
        };

        let node_root = node_bin
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));

        let version = Command::new(&node_bin)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_else(|| "node".to_string())
            .trim()
            .to_string();

        info!(
            node_root = ?node_root,
            version = %version,
            "Discovered host Node.js toolchain"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((node_bin.clone(), "bin/node".into()));

        // Companion package managers: npm, npx, pnpm, yarn
        for mgr in &["npm", "npx", "pnpm", "yarn"] {
            let companion = node_bin.parent().map(|p| p.join(mgr));
            if let Some(cp) = companion {
                if cp.exists() {
                    bin_files.push((cp, format!("bin/{}", mgr)));
                    continue;
                }
            }
            if cfg!(windows) {
                let cmd_companion = node_bin.parent().map(|p| p.join(format!("{}.cmd", mgr)));
                if let Some(cp) = cmd_companion {
                    if cp.exists() {
                        bin_files.push((cp, format!("bin/{}.cmd", mgr)));
                        continue;
                    }
                }
            }
            if let Ok(found) = which::which(mgr) {
                let name = found
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(mgr)
                    .to_string();
                bin_files.push((found, format!("bin/{}", name)));
            }
        }

        // Collect global lib/node_modules if present in the standalone node root
        let node_modules_dir = node_root.join("lib/node_modules");
        if node_modules_dir.exists() {
            collect_files_recursive(
                &node_modules_dir,
                &node_modules_dir,
                &mut lib_files,
                "lib/node_modules",
            );
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            version,
            root_dir: node_root,
            bin_files,
            lib_files,
        }))
    }
}
