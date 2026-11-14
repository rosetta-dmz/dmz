use super::super::types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainProfile,
};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::collections::HashSet;
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
        self.discover_with_context(&ToolchainDiscoveryContext::default())
    }

    fn discover_with_context(
        &self,
        ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
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
            profile = %ctx.profile,
            "Discovered host Node.js toolchain"
        );

        let selected_managers = select_node_managers(ctx);
        let include_headers = should_include_node_headers(ctx);
        let bin_files = collect_node_binaries_for(&node_bin, &selected_managers);
        let lib_files =
            collect_node_libs_for(&node_root, ctx.profile, &selected_managers, include_headers);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            version,
            root_dir: node_root,
            bin_files,
            lib_files,
        }))
    }
}

fn should_include_node_headers(ctx: &ToolchainDiscoveryContext) -> bool {
    ctx.profile == ToolchainProfile::Full
        || ctx.ecosystems.contains(&ToolchainEcosystem::Cc)
        || ctx
            .workspace_root
            .as_deref()
            .is_some_and(|w| w.join("binding.gyp").is_file())
}

fn select_node_managers(ctx: &ToolchainDiscoveryContext) -> Vec<&'static str> {
    if ctx.profile == ToolchainProfile::Full {
        return NODE_MANAGERS.to_vec();
    }

    let Some(ws) = ctx.workspace_root.as_deref() else {
        return vec!["npm", "npx"];
    };

    let uses_pnpm = ws.join("pnpm-lock.yaml").is_file() || ws.join("pnpm-workspace.yaml").is_file();
    let uses_yarn = ws.join("yarn.lock").is_file();
    let uses_npm = ws.join("package-lock.json").is_file() || (!uses_pnpm && !uses_yarn);

    let mut managers = Vec::new();
    if uses_npm {
        managers.push("npm");
        managers.push("npx");
    }
    if uses_pnpm {
        managers.push("pnpm");
    }
    if uses_yarn {
        managers.push("yarn");
    }
    managers
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

fn collect_node_binaries_for(node_bin: &Path, managers: &[&str]) -> Vec<(PathBuf, String)> {
    let mut bin_files = vec![(node_bin.to_path_buf(), "bin/node".into())];
    let bin_dir = node_bin.parent().filter(|p| !p.as_os_str().is_empty());

    for mgr in managers {
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

fn collect_node_libs_for(
    node_root: &Path,
    profile: ToolchainProfile,
    selected_managers: &[&str],
    include_headers: bool,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let node_modules_dir = node_root.join("lib/node_modules");
    if node_modules_dir.is_dir() {
        if profile == ToolchainProfile::Full {
            collect_files_recursive(
                &node_modules_dir,
                &node_modules_dir,
                &mut lib_files,
                "lib/node_modules",
            );
        } else {
            let allowed_mods: HashSet<&str> = selected_managers.iter().copied().collect();
            if let Ok(entries) = fs::read_dir(&node_modules_dir) {
                for entry in entries.flatten() {
                    let mod_name = entry.file_name().to_string_lossy().into_owned();
                    let matches_mgr = allowed_mods.contains(mod_name.as_str())
                        || (mod_name == "npm" && allowed_mods.contains("npx"));
                    if matches_mgr && entry.path().is_dir() {
                        collect_files_recursive(
                            &entry.path(),
                            &node_modules_dir,
                            &mut lib_files,
                            "lib/node_modules",
                        );
                    }
                }
            }
        }
    }

    if include_headers {
        let include_node = node_root.join("include/node");
        if include_node.is_dir() {
            collect_files_recursive(&include_node, node_root, &mut lib_files, "");
        }
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

    #[test]
    fn test_node_minimal_profile_selects_only_requested_package_manager(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let node_root = temp.path().join("node_home");
        let bin = node_root.join("bin");
        let inc = node_root.join("include/node");
        let nm = node_root.join("lib/node_modules");

        fs::create_dir_all(&bin)?;
        fs::create_dir_all(&inc)?;
        fs::create_dir_all(nm.join("npm"))?;
        fs::create_dir_all(nm.join("pnpm"))?;
        fs::create_dir_all(nm.join("corepack"))?;

        fs::write(bin.join("node"), "node")?;
        fs::write(bin.join("npm"), "npm")?;
        fs::write(bin.join("pnpm"), "pnpm")?;
        fs::write(bin.join("corepack"), "corepack")?;
        fs::write(inc.join("node.h"), "#define NODE_H")?;
        fs::write(nm.join("npm/package.json"), "{}")?;
        fs::write(nm.join("pnpm/package.json"), "{}")?;
        fs::write(nm.join("corepack/package.json"), "{}")?;

        let minimal_bins: Vec<String> = collect_node_binaries_for(&bin, &["pnpm"])
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        assert!(minimal_bins.contains(&"bin/node".to_string()));
        assert!(minimal_bins.contains(&"bin/pnpm".to_string()));
        assert!(!minimal_bins.contains(&"bin/npm".to_string()));
        assert!(!minimal_bins.contains(&"bin/corepack".to_string()));

        let minimal_libs: Vec<String> =
            collect_node_libs_for(&node_root, ToolchainProfile::Minimal, &["pnpm"], false)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(minimal_libs.contains(&"lib/node_modules/pnpm/package.json".to_string()));
        assert!(!minimal_libs.contains(&"lib/node_modules/npm/package.json".to_string()));
        assert!(!minimal_libs.contains(&"lib/node_modules/corepack/package.json".to_string()));
        assert!(!minimal_libs.contains(&"include/node/node.h".to_string()));

        let full_libs: Vec<String> =
            collect_node_libs_for(&node_root, ToolchainProfile::Full, &["pnpm"], true)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(full_libs.contains(&"lib/node_modules/npm/package.json".to_string()));
        assert!(full_libs.contains(&"lib/node_modules/pnpm/package.json".to_string()));
        assert!(full_libs.contains(&"lib/node_modules/corepack/package.json".to_string()));
        assert!(full_libs.contains(&"include/node/node.h".to_string()));

        // Context helper includes headers when Cc or binding.gyp is active, or Full profile
        let ws_plain = temp.path().join("plain_js");
        fs::create_dir_all(&ws_plain)?;
        let ctx_plain = ToolchainDiscoveryContext {
            profile: ToolchainProfile::Minimal,
            workspace_root: Some(ws_plain.clone()),
            ..ToolchainDiscoveryContext::default()
        };
        assert!(!should_include_node_headers(&ctx_plain));

        let ctx_cc = ToolchainDiscoveryContext {
            ecosystems: [ToolchainEcosystem::Cc].into_iter().collect(),
            ..ctx_plain.clone()
        };
        assert!(should_include_node_headers(&ctx_cc));

        fs::write(ws_plain.join("binding.gyp"), "{}")?;
        assert!(should_include_node_headers(&ctx_plain));

        Ok(())
    }
}
