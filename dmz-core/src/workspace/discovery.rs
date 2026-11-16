// Workspace manifest and multi-workspace configuration discovery.
// Climbs parent directories to locate `dmz-workspace.toml` or reconstructs
// multi-workspace member configurations from `dmz.lock` metadata.

use super::schema::{MemberRole, WorkspaceManifestError, WorkspaceMember, WorkspaceProjectConfig};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_PROJECT_DISCOVERY_DEPTH: usize = 4;
const MAX_PROJECT_DISCOVERY_ENTRIES: usize = 512;

/// Returns `true` when `dir` directly contains a Node/npm/pnpm/yarn project manifest.
pub fn has_node_project_manifest(dir: &Path) -> bool {
    dir.join("package.json").is_file()
        || dir.join("pnpm-workspace.yaml").is_file()
        || dir.join("pnpm-lock.yaml").is_file()
        || dir.join("package-lock.json").is_file()
        || dir.join("yarn.lock").is_file()
}

/// Discovers Node project roots inside `member_dir`: includes `member_dir` if it holds a
/// Node manifest/lockfile AND performs a bounded, symlink-safe walk (`depth <= 4`, cap `<= 512`)
/// to locate any independent nested Node roots (e.g., `frontend/`) outside declared workspace globs.
pub fn discover_node_project_roots(member_dir: &Path) -> Vec<PathBuf> {
    if !member_dir.is_dir() {
        return Vec::new();
    }
    let Ok(canon_root) = fs::canonicalize(member_dir) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    if has_node_project_manifest(member_dir) {
        roots.push(member_dir.to_path_buf());
    }

    let declared_ws_dirs: HashSet<PathBuf> = if member_dir.join("package.json").is_file()
        || member_dir.join("pnpm-workspace.yaml").is_file()
    {
        crate::resolver::manifest::npm::NpmManifestScanner::discover_npm_workspace_package_dirs(
            member_dir,
        )
        .into_iter()
        .map(|d| fs::canonicalize(&d).unwrap_or(d))
        .collect()
    } else {
        HashSet::new()
    };

    let mut visited = HashSet::new();
    visited.insert(canon_root.clone());
    let mut nested = Vec::new();
    let mut scanned = 0usize;
    collect_nested_project_roots(
        member_dir,
        &canon_root,
        0,
        &mut scanned,
        &mut visited,
        &mut nested,
        &|dir| has_node_project_manifest(dir),
        true,
    );
    nested.sort();
    for dir in nested {
        let canon = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let is_declared_ws_pkg = declared_ws_dirs
            .iter()
            .any(|ws_dir| canon == *ws_dir || canon.starts_with(ws_dir));
        if !is_declared_ws_pkg && !roots.contains(&dir) {
            roots.push(dir);
        }
    }
    roots
}

/// Discovers Cargo manifest directories (`Cargo.toml` and nested `src-tauri/Cargo.toml`)
/// inside `member_dir` using bounded, symlink-safe traversal and canonical deduplication.
pub fn discover_tauri_cargo_dirs(member_dir: &Path) -> Vec<PathBuf> {
    if !member_dir.is_dir() {
        return Vec::new();
    }
    let Ok(canon_root) = fs::canonicalize(member_dir) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    let mut seen_manifests = HashSet::new();

    for candidate in [member_dir.to_path_buf(), member_dir.join("src-tauri")] {
        let toml = candidate.join("Cargo.toml");
        if toml.is_file() {
            let canon_toml = fs::canonicalize(&toml).unwrap_or(toml);
            if seen_manifests.insert(canon_toml) {
                dirs.push(candidate);
            }
        }
    }

    let mut visited = HashSet::new();
    visited.insert(canon_root.clone());
    let mut nested = Vec::new();
    let mut scanned = 0usize;
    collect_nested_project_roots(
        member_dir,
        &canon_root,
        0,
        &mut scanned,
        &mut visited,
        &mut nested,
        &|dir| dir.join("src-tauri").join("Cargo.toml").is_file(),
        false,
    );
    nested.sort();
    for sub in nested {
        let tauri_dir = sub.join("src-tauri");
        let toml = tauri_dir.join("Cargo.toml");
        let canon_toml = fs::canonicalize(&toml).unwrap_or(toml);
        if seen_manifests.insert(canon_toml) {
            dirs.push(tauri_dir);
        }
    }

    dirs
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_project_roots<F>(
    dir: &Path,
    canon_root: &Path,
    depth: usize,
    scanned: &mut usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
    predicate: &F,
    stop_on_match: bool,
) where
    F: Fn(&Path) -> bool,
{
    if depth > MAX_PROJECT_DISCOVERY_DEPTH || *scanned >= MAX_PROJECT_DISCOVERY_ENTRIES {
        return;
    }

    if depth > 0 && predicate(dir) {
        out.push(dir.to_path_buf());
        if stop_on_match {
            return;
        }
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut child_dirs = Vec::new();
    for entry in entries.flatten() {
        *scanned = scanned.saturating_add(1);
        if *scanned > MAX_PROJECT_DISCOVERY_ENTRIES {
            break;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let path = entry.path();
        if name_str.starts_with('.')
            || super::layout::should_skip_workspace_artifact_dir(&path, &name_str, None)
        {
            continue;
        }
        let Ok(canon_child) = fs::canonicalize(&path) else {
            continue;
        };
        if !canon_child.starts_with(canon_root) || !visited.insert(canon_child) {
            continue;
        }
        child_dirs.push(path);
    }
    child_dirs.sort();
    for child in child_dirs {
        collect_nested_project_roots(
            &child,
            canon_root,
            depth.saturating_add(1),
            scanned,
            visited,
            out,
            predicate,
            stop_on_match,
        );
    }
}

/// Searches for `dmz-workspace.toml` starting in `start_dir` and traversing parents.
///
/// Returns an error if a manifest file exists but cannot be read or parsed.
pub fn discover_workspace_manifest_checked(
    start_dir: &Path,
) -> Result<Option<(WorkspaceProjectConfig, PathBuf)>, WorkspaceManifestError> {
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
            let config = WorkspaceProjectConfig::load_from_file(&candidate)?;
            return Ok(Some((config, candidate)));
        }

        if !curr.pop() {
            break;
        }
    }

    Ok(None)
}

/// Searches for `dmz-workspace.toml` starting in `start_dir` and traversing parents.
///
/// Logs a warning and returns `None` when a manifest fails validation or parsing;
/// prefer [`discover_workspace_manifest_checked`] when errors should be surfaced to the caller.
pub fn discover_workspace_manifest(start_dir: &Path) -> Option<(WorkspaceProjectConfig, PathBuf)> {
    match discover_workspace_manifest_checked(start_dir) {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse discovered dmz-workspace.toml");
            None
        }
    }
}

/// Discovers `dmz-workspace.toml` starting in `selected_root`, climbing ancestors ONLY when
/// an ancestor manifest explicitly declares a non-root member matching or enclosing `selected_root`.
///
/// Prevents a standalone child project from accidentally pulling in an unrelated ancestor umbrella.
pub fn discover_workspace_manifest_scoped_checked(
    selected_root: &Path,
) -> Result<Option<(WorkspaceProjectConfig, PathBuf)>, WorkspaceManifestError> {
    let abs_start = if selected_root.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(selected_root))
            .unwrap_or_else(|_| selected_root.to_path_buf())
    } else {
        selected_root.to_path_buf()
    };
    let start_dir = if abs_start.is_file() {
        abs_start.parent().unwrap_or(Path::new("/")).to_path_buf()
    } else {
        abs_start
    };
    let canon_start = fs::canonicalize(&start_dir).unwrap_or_else(|_| start_dir.clone());

    let direct_candidate = start_dir.join("dmz-workspace.toml");
    if direct_candidate.is_file() {
        let config = WorkspaceProjectConfig::load_from_file(&direct_candidate)?;
        return Ok(Some((config, direct_candidate)));
    }

    let mut curr = start_dir;
    while curr.pop() {
        let candidate = curr.join("dmz-workspace.toml");
        if !candidate.is_file() {
            continue;
        }
        let config = WorkspaceProjectConfig::load_from_file(&candidate)?;
        let ancestor_canon = fs::canonicalize(&curr).unwrap_or_else(|_| curr.clone());
        let matches_declared_member = config.members.iter().any(|member| {
            let resolved_member = if member.path.is_absolute() {
                member.path.clone()
            } else {
                curr.join(&member.path)
            };
            let member_canon = fs::canonicalize(&resolved_member).unwrap_or(resolved_member);
            member_canon == canon_start
                || (member_canon != ancestor_canon && canon_start.starts_with(&member_canon))
        });
        if matches_declared_member {
            return Ok(Some((config, candidate)));
        }
        break;
    }

    Ok(None)
}

/// Searches for `dmz-workspace.toml` scoped to `selected_root` (no unrelated parent bleed).
pub fn discover_workspace_manifest_scoped(
    selected_root: &Path,
) -> Option<(WorkspaceProjectConfig, PathBuf)> {
    match discover_workspace_manifest_scoped_checked(selected_root) {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse scoped dmz-workspace.toml");
            None
        }
    }
}

/// Scoped counterpart of [`discover_or_from_lockfile`] that prevents an unrelated ancestor
/// umbrella manifest from expanding a standalone selected repository.
pub fn discover_or_from_lockfile_scoped(
    selected_root: &Path,
    lockfile_path: &Path,
) -> Option<WorkspaceProjectConfig> {
    if let Some((cfg, _)) = discover_workspace_manifest_scoped(selected_root) {
        return Some(cfg);
    }
    reconstruct_multi_workspace_from_lockfile(selected_root, lockfile_path)
}

fn reconstruct_multi_workspace_from_lockfile(
    start_dir: &Path,
    lockfile_path: &Path,
) -> Option<WorkspaceProjectConfig> {
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
    reconstruct_multi_workspace_from_lockfile(start_dir, lockfile_path)
}

impl WorkspaceProjectConfig {
    pub fn discover_workspace_manifest(start_dir: &Path) -> Option<(Self, PathBuf)> {
        discover_workspace_manifest(start_dir)
    }

    pub fn discover_workspace_manifest_checked(
        start_dir: &Path,
    ) -> Result<Option<(Self, PathBuf)>, WorkspaceManifestError> {
        discover_workspace_manifest_checked(start_dir)
    }

    pub fn discover_workspace_manifest_scoped(selected_root: &Path) -> Option<(Self, PathBuf)> {
        discover_workspace_manifest_scoped(selected_root)
    }

    pub fn discover_workspace_manifest_scoped_checked(
        selected_root: &Path,
    ) -> Result<Option<(Self, PathBuf)>, WorkspaceManifestError> {
        discover_workspace_manifest_scoped_checked(selected_root)
    }

    pub fn discover_or_from_lockfile(start_dir: &Path, lockfile_path: &Path) -> Option<Self> {
        discover_or_from_lockfile(start_dir, lockfile_path)
    }

    pub fn discover_or_from_lockfile_scoped(
        selected_root: &Path,
        lockfile_path: &Path,
    ) -> Option<Self> {
        discover_or_from_lockfile_scoped(selected_root, lockfile_path)
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
        assert!(discover_workspace_manifest_checked(dir.path())?.is_none());
        Ok(())
    }

    #[test]
    fn test_discover_workspace_manifest_checked_surfaces_parse_error() -> Result<(), Box<dyn Error>>
    {
        let dir = tempdir()?;
        let manifest = dir.path().join("dmz-workspace.toml");
        fs::write(&manifest, "name = [invalid toml syntax\n")?;

        let err = discover_workspace_manifest_checked(dir.path())
            .err()
            .ok_or("expected parse error for malformed dmz-workspace.toml")?;
        assert!(err.to_string().contains("Invalid workspace manifest"));
        Ok(())
    }

    #[test]
    fn test_scoped_discovery_ignores_unrelated_ancestor_umbrella() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let sibling_sidecar = root.path().join("sidecar");
        let standalone_child = root.path().join("standalone-child");
        let declared_member = root.path().join("declared-member");
        fs::create_dir_all(&sibling_sidecar)?;
        fs::create_dir_all(&standalone_child)?;
        fs::create_dir_all(&declared_member)?;

        fs::write(
            root.path().join("dmz-workspace.toml"),
            r#"name = "parent-umbrella"
[[members]]
id = "primary"
path = "."
role = "primary"

[[members]]
id = "declared-member"
path = "declared-member"
role = "sidecar"
"#,
        )?;

        // Standalone sibling child not listed in ancestor dmz-workspace.toml must stay isolated
        assert!(discover_workspace_manifest_scoped(&standalone_child).is_none());
        assert!(discover_workspace_manifest_scoped_checked(&standalone_child)?.is_none());

        // Explicitly declared non-root member resolves back to its umbrella manifest
        let found = discover_workspace_manifest_scoped(&declared_member)
            .ok_or("expected declared member to resolve to parent umbrella")?;
        assert_eq!(found.0.name, "parent-umbrella");

        // Root itself finds its direct dmz-workspace.toml
        let direct = discover_workspace_manifest_scoped(root.path())
            .ok_or("expected root manifest to be discovered")?;
        assert_eq!(direct.0.name, "parent-umbrella");
        Ok(())
    }

    #[test]
    fn test_discover_nested_node_and_tauri_roots_skips_symlinks() -> Result<(), Box<dyn Error>> {
        let tmp = tempdir()?;
        let outside = tmp.path().join("outside-pkg");
        fs::create_dir_all(outside.join("src-tauri"))?;
        fs::write(outside.join("package.json"), r#"{"name":"outside"}"#)?;
        fs::write(
            outside.join("src-tauri/Cargo.toml"),
            "[package]\nname=\"out\"\n",
        )?;

        let member = tmp.path().join("member");
        let frontend = member.join("frontend");
        fs::create_dir_all(frontend.join("src-tauri"))?;
        fs::create_dir_all(member.join("node_modules/ignored-pkg"))?;
        fs::write(frontend.join("package.json"), r#"{"name":"frontend"}"#)?;
        fs::write(
            frontend.join("src-tauri/Cargo.toml"),
            "[package]\nname=\"fe\"\n",
        )?;
        fs::write(
            member.join("node_modules/ignored-pkg/package.json"),
            r#"{"name":"ignored"}"#,
        )?;

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, member.join("escape-link"))?;
            std::os::unix::fs::symlink(&member, frontend.join("loop-link"))?;
        }

        let node_roots = discover_node_project_roots(&member);
        assert_eq!(node_roots, vec![frontend.clone()]);

        let cargo_dirs = discover_tauri_cargo_dirs(&member);
        assert_eq!(cargo_dirs, vec![frontend.join("src-tauri")]);
        Ok(())
    }

    #[test]
    fn test_discover_node_roots_includes_root_plus_undeclared_frontend_and_excludes_monorepo_globs(
    ) -> Result<(), Box<dyn Error>> {
        let tmp = tempdir()?;
        let root = tmp.path();
        let ws_pkg = root.join("packages/ui");
        let standalone_fe = root.join("frontend");
        fs::create_dir_all(&ws_pkg)?;
        fs::create_dir_all(&standalone_fe)?;

        fs::write(
            root.join("package.json"),
            r#"{"name":"wrapper","workspaces":["packages/*"]}"#,
        )?;
        fs::write(ws_pkg.join("package.json"), r#"{"name":"ui"}"#)?;
        fs::write(standalone_fe.join("package.json"), r#"{"name":"frontend"}"#)?;

        let roots = discover_node_project_roots(root);
        assert!(roots.contains(&root.to_path_buf()));
        assert!(roots.contains(&standalone_fe));
        assert!(!roots.contains(&ws_pkg));
        Ok(())
    }
}
