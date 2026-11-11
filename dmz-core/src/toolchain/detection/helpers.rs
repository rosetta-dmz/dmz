use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Expand workspace member pattern (supporting literal paths and `dir/*` globs) into directories.
pub fn expand_workspace_member_dirs(workspace_root: &Path, pattern: &str) -> Vec<PathBuf> {
    if let Some(prefix) = pattern
        .strip_suffix("/*")
        .or_else(|| pattern.strip_suffix('*'))
    {
        let parent = workspace_root.join(prefix.trim_end_matches('/'));
        let mut dirs = Vec::new();
        if parent.is_dir() {
            dirs.push(parent.clone());
            if let Ok(entries) = std::fs::read_dir(&parent) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        dirs.push(p);
                    }
                }
            }
        }
        dirs
    } else {
        vec![workspace_root.join(pattern)]
    }
}

/// Check declared workspace member directory for explicit native build manifests.
pub fn check_member_native_manifests(
    member_dir: &Path,
    ecosystems: &mut HashSet<ToolchainEcosystem>,
) {
    super::native::NativeBuildDetector::detect(member_dir, ecosystems);
}
