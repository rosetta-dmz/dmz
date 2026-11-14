use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

const NODE_MANAGERS: &[&str] = &["npm", "npx", "pnpm", "yarn", "corepack"];

/// Discovers host Node.js runtime and companion package managers (npm, npx, pnpm, yarn).
#[derive(Debug, Default, Clone)]
pub struct NodeDiscoverer;

impl ToolchainDiscoverer for NodeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Node
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let node_candidates = collect_node_candidates();
        if node_candidates.is_empty() {
            return Ok(None);
        }

        let Some((node_bin, node_root)) =
            select_strict_standalone_verified(node_candidates, resolve_node_root_for_candidate)
        else {
            return Ok(None);
        };

        let version = probe_node_version(&node_bin);
        let redacted_root = redact_home_prefix(&node_root);
        info!(
            node_root = %redacted_root,
            version = %version,
            "Discovered host Node.js toolchain"
        );

        let bin_files = collect_node_binaries(&node_bin);
        let lib_files = collect_node_libs(&node_root);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            version,
            root_dir: node_root,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_node_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let nvm_dir = PathBuf::from(&home).join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(&nvm_dir) {
            for entry in entries.flatten() {
                let bin = entry.path().join("bin/node");
                if bin.is_file() {
                    push_unique_path(&mut candidates, bin);
                }
            }
        }
    }

    let mut standard_paths = vec![PathBuf::from("/usr/local/bin/node")];
    if !cfg!(target_os = "macos") {
        standard_paths.push(PathBuf::from("/usr/bin/node"));
    }
    for p in standard_paths {
        if p.is_file() {
            push_unique_path(&mut candidates, p);
        }
    }

    push_system_bin_candidates(&mut candidates, "node");
    candidates
}

fn resolve_node_root_for_candidate(node_bin: &Path) -> Option<PathBuf> {
    if is_macos_usr_bin_stub(node_bin) || !node_bin.is_file() {
        return None;
    }
    resolve_prefix_root(node_bin)
}

fn probe_node_version(node_bin: &Path) -> String {
    Command::new(node_bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "node".to_string())
}

fn collect_node_binaries(node_bin: &Path) -> Vec<(PathBuf, String)> {
    let mut bin_files = vec![(node_bin.to_path_buf(), "bin/node".into())];
    let bin_dir = node_bin.parent().filter(|p| !p.as_os_str().is_empty());

    for mgr in NODE_MANAGERS {
        if let Some(dir) = bin_dir {
            let cp = dir.join(mgr);
            if cp.is_file() {
                bin_files.push((cp, format!("bin/{}", mgr)));
                continue;
            }
            if cfg!(windows) {
                let cmd_cp = dir.join(format!("{}.cmd", mgr));
                if cmd_cp.is_file() {
                    bin_files.push((cmd_cp, format!("bin/{}.cmd", mgr)));
                    continue;
                }
            }
        }
        if let Ok(found) = which::which(mgr) {
            if is_non_standalone_path(&found) {
                emit_non_relocatable_notice(&found);
                continue;
            }
            if is_macos_usr_bin_stub(&found) {
                continue;
            }
            let name = found
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(mgr)
                .to_string();
            bin_files.push((found, format!("bin/{}", name)));
        }
    }

    bin_files
}

fn collect_node_libs(node_root: &Path) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let node_modules_dir = node_root.join("lib/node_modules");
    if node_modules_dir.is_dir() {
        collect_files_recursive(
            &node_modules_dir,
            &node_modules_dir,
            &mut lib_files,
            "lib/node_modules",
        );
    }

    let include_node = node_root.join("include/node");
    if include_node.is_dir() {
        collect_files_recursive(&include_node, node_root, &mut lib_files, "");
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_node_collects_headers_and_modules() {
        let temp = tempdir().unwrap();
        let node_root = temp.path().join("fake_node");
        let bin = node_root.join("bin");
        let inc = node_root.join("include/node");
        let nm = node_root.join("lib/node_modules/npm");

        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&inc).unwrap();
        fs::create_dir_all(&nm).unwrap();

        fs::write(bin.join("node"), "node").unwrap();
        fs::write(bin.join("npm"), "npm").unwrap();
        fs::write(bin.join("corepack"), "corepack").unwrap();
        fs::write(inc.join("node.h"), "#define NODE_H").unwrap();
        fs::write(nm.join("package.json"), "{}").unwrap();

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((bin.join("node"), "bin/node".into()));
        for mgr in &["npm", "npx", "pnpm", "yarn", "corepack"] {
            let cp = bin.join(mgr);
            if cp.exists() {
                bin_files.push((cp, format!("bin/{}", mgr)));
            }
        }

        let node_modules_dir = node_root.join("lib/node_modules");
        if node_modules_dir.exists() {
            collect_files_recursive(
                &node_modules_dir,
                &node_modules_dir,
                &mut lib_files,
                "lib/node_modules",
            );
        }
        let include_node = node_root.join("include/node");
        if include_node.exists() {
            collect_files_recursive(&include_node, &node_root, &mut lib_files, "");
        }

        let bin_names: Vec<String> = bin_files.into_iter().map(|(_, r)| r).collect();
        assert!(bin_names.contains(&"bin/npm".to_string()));
        assert!(bin_names.contains(&"bin/corepack".to_string()));

        let lib_names: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(lib_names.contains(&"include/node/node.h".to_string()));
        assert!(lib_names.contains(&"lib/node_modules/npm/package.json".to_string()));
    }
}
