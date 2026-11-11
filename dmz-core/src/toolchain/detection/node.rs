use super::helpers::expand_workspace_member_dirs;
use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct NodeDetector;

impl NodeDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if dir.join("package.json").is_file()
            || dir.join("pnpm-workspace.yaml").is_file()
            || dir.join("pnpm-lock.yaml").is_file()
            || dir.join("yarn.lock").is_file()
            || dir.join("package-lock.json").is_file()
            || dir.join("bun.lockb").is_file()
            || dir.join("bunfig.toml").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Node);
            Self::check_node_workspace_members(dir, ecosystems);
        }
    }

    /// Check declared Node/pnpm/yarn workspace packages for explicit native build manifests.
    pub fn check_node_workspace_members(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        let pkg_json = workspace_root.join("package.json");
        if let Ok(content) = std::fs::read_to_string(&pkg_json) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                let pkgs_opt = val.get("workspaces").and_then(|w| {
                    w.as_array()
                        .cloned()
                        .or_else(|| w.get("packages").and_then(|p| p.as_array()).cloned())
                });

                if let Some(pkgs) = pkgs_opt {
                    for pkg in pkgs {
                        if let Some(pattern) = pkg.as_str() {
                            for candidate_dir in
                                expand_workspace_member_dirs(workspace_root, pattern)
                            {
                                super::helpers::check_member_native_manifests(
                                    &candidate_dir,
                                    ecosystems,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
