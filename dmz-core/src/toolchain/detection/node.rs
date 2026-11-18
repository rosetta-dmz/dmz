use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct NodeDetector;

impl NodeDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        let node_dirs = crate::workspace::discover_node_project_roots(dir);
        if !node_dirs.is_empty() || crate::workspace::has_node_project_manifest(dir) {
            ecosystems.insert(ToolchainEcosystem::Node);
            for node_dir in node_dirs {
                Self::check_node_workspace_members(&node_dir, ecosystems);
            }
            if crate::workspace::has_node_project_manifest(dir) {
                Self::check_node_workspace_members(dir, ecosystems);
            }
        }
    }

    /// Check declared Node/pnpm/yarn workspace packages for explicit native build manifests.
    pub fn check_node_workspace_members(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        for candidate_dir in
            crate::resolver::manifest::npm::NpmManifestScanner::discover_npm_workspace_package_dirs(
                workspace_root,
            )
        {
            super::helpers::check_member_native_manifests(&candidate_dir, ecosystems);
        }
    }
}
