// Workspace manifest and multi-workspace configuration discovery.
// Climbs parent directories to locate `dmz-workspace.toml` or reconstructs
// multi-workspace member configurations from `dmz.lock` metadata.

use super::schema::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use std::path::{Path, PathBuf};

/// Searches for `dmz-workspace.toml` starting in `start_dir` and traversing parents.
pub fn discover_workspace_manifest(start_dir: &Path) -> Option<(WorkspaceProjectConfig, PathBuf)> {
    let abs_start = if start_dir.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(start_dir))
            .unwrap_or_else(|_| start_dir.to_path_buf())
    } else {
        start_dir.to_path_buf()
    };

    let mut curr = if abs_start.is_file() {
        abs_start.parent().unwrap_or(Path::new("/")).to_path_buf()
    } else {
        abs_start
    };

    loop {
        let candidate = curr.join("dmz-workspace.toml");
        if candidate.is_file() {
            if let Ok(config) = WorkspaceProjectConfig::load_from_file(&candidate) {
                return Some((config, candidate));
            }
        }

        if !curr.pop() {
            break;
        }
    }

    None
}

/// Discovers `dmz-workspace.toml` or reconstructs multi-workspace configuration from `dmz.lock` metadata.
///
/// # Async Runtime Note
/// Performs synchronous lockfile reading and manifest discovery.
/// When invoking from an asynchronous Tokio runtime context, offload via `tokio::task::spawn_blocking`.
pub fn discover_or_from_lockfile(
    start_dir: &Path,
    lockfile_path: &Path,
) -> Option<WorkspaceProjectConfig> {
    if let Some((cfg, _)) = discover_workspace_manifest(start_dir) {
        return Some(cfg);
    }
    if let Ok(lf) = crate::resolver::lockfile::DmzLockfile::load_and_verify(lockfile_path) {
        if lf.workspaces.len() > 1 {
            let mut members = Vec::new();
            for ws_meta in lf.workspaces.values() {
                let role = match ws_meta.role.to_lowercase().as_str() {
                    "primary" => MemberRole::Primary,
                    "sidecar" => MemberRole::Sidecar,
                    "service" => MemberRole::Service,
                    _ => MemberRole::Member,
                };
                let mut m = WorkspaceMember::new(&ws_meta.id, &ws_meta.path, role);
                m.ecosystem = ws_meta.ecosystem.clone();
                members.push(m);
            }
            let abs_dir = fs::canonicalize(start_dir).unwrap_or_else(|_| start_dir.to_path_buf());
            let dir_for_name = if abs_dir.is_file() {
                abs_dir.parent().unwrap_or(&abs_dir)
            } else {
                &abs_dir
            };
            let name = dir_for_name
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workspace");
            return Some(WorkspaceProjectConfig::new(name, members));
        }
    }
    None
}

impl WorkspaceProjectConfig {
    pub fn discover_workspace_manifest(start_dir: &Path) -> Option<(Self, PathBuf)> {
        discover_workspace_manifest(start_dir)
    }

    pub fn discover_or_from_lockfile(start_dir: &Path, lockfile_path: &Path) -> Option<Self> {
        discover_or_from_lockfile(start_dir, lockfile_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use tempfile::tempdir;

    #[test]
    fn test_discover_workspace_manifest_in_current_dir() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        let manifest = dir.path().join("dmz-workspace.toml");
        fs::write(
            &manifest,
            "name = \"test-ws\"\n[[members]]\nid = \"m1\"\npath = \".\"\nrole = \"primary\"\n",
        )?;

        let discovered = discover_workspace_manifest(dir.path());
        assert!(discovered.is_some());
        let (cfg, path) = discovered.ok_or("expected discovered manifest")?;
        assert_eq!(cfg.name, "test-ws");
        assert_eq!(path.canonicalize()?, manifest.canonicalize()?);
        Ok(())
    }

    #[test]
    fn test_discover_workspace_manifest_climbs_parents() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let manifest = root.path().join("dmz-workspace.toml");
        fs::write(
            &manifest,
            "name = \"parent-ws\"\n[[members]]\nid = \"root\"\npath = \".\"\nrole = \"primary\"\n",
        )?;

        let deep_nested = root.path().join("a").join("b").join("c");
        fs::create_dir_all(&deep_nested)?;

        let discovered = discover_workspace_manifest(&deep_nested);
        assert!(discovered.is_some());
        let (cfg, path) = discovered.ok_or("expected discovered manifest from deep child")?;
        assert_eq!(cfg.name, "parent-ws");
        assert_eq!(path.canonicalize()?, manifest.canonicalize()?);
        Ok(())
    }

    #[test]
    fn test_discover_workspace_manifest_from_file_path() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let manifest = root.path().join("dmz-workspace.toml");
        fs::write(
            &manifest,
            "name = \"file-ws\"\n[[members]]\nid = \"root\"\npath = \".\"\nrole = \"primary\"\n",
        )?;

        let nested_file = root.path().join("src").join("main.rs");
        fs::create_dir_all(nested_file.parent().ok_or("missing parent")?)?;
        fs::write(&nested_file, "fn main() {}")?;

        let discovered = discover_workspace_manifest(&nested_file);
        assert!(discovered.is_some());
        let (cfg, path) = discovered.ok_or("expected discovered manifest starting from file")?;
        assert_eq!(cfg.name, "file-ws");
        assert_eq!(path.canonicalize()?, manifest.canonicalize()?);
        Ok(())
    }

    #[test]
    fn test_discover_workspace_manifest_not_found() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        assert!(discover_workspace_manifest(dir.path()).is_none());
        Ok(())
    }
}
