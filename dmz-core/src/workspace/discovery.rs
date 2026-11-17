// Workspace manifest and multi-workspace configuration discovery.
// Climbs parent directories to locate `dmz-workspace.toml` or reconstructs
// multi-workspace member configurations from `dmz.lock` metadata.

use super::schema::{MemberRole, WorkspaceManifestError, WorkspaceMember, WorkspaceProjectConfig};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_PROJECT_DISCOVERY_DEPTH: usize = 64;

fn canonical_excluded_roots(member_dir: &Path, excluded_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let member_canon = fs::canonicalize(member_dir).unwrap_or_else(|_| member_dir.to_path_buf());
    excluded_dirs
        .iter()
        .filter_map(|p| {
            let canon = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            // Exclude only sibling members strictly nested inside `member_dir`;
            // never exclude an ancestor (e.g. `.`) from scanning its child members.
            (canon != member_canon && canon.starts_with(&member_canon)).then_some(canon)
        })
        .collect()
}

fn is_excluded_path(canon_path: &Path, excluded_canon: &[PathBuf]) -> bool {
    excluded_canon
        .iter()
        .any(|ex| canon_path == *ex || canon_path.starts_with(ex))
}

/// Expands workspace glob patterns (`*`, `**`, exact directory names) into directories
/// containing `manifest_file`, skipping ignored build/VCS artifact directories.
pub(crate) fn expand_manifest_glob_pattern(
    root_dir: &Path,
    pattern: &str,
    manifest_file: &str,
    out: &mut Vec<PathBuf>,
) {
    if pattern.contains("**") {
        let base = pattern
            .split("**")
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        let start = if base.is_empty() {
            root_dir.to_path_buf()
        } else {
            root_dir.join(base)
        };
        collect_recursive_manifest_dirs(&start, manifest_file, 0, out);
        return;
    }

    if let Some(prefix) = pattern
        .strip_suffix("/*")
        .or_else(|| pattern.strip_suffix('*'))
    {
        let base_dir = if prefix.is_empty() {
            root_dir.to_path_buf()
        } else {
            root_dir.join(prefix.trim_end_matches('/'))
        };
        if let Ok(entries) = fs::read_dir(base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join(manifest_file).is_file() {
                    out.push(path);
                }
            }
        }
        return;
    }

    let exact_dir = root_dir.join(pattern);
    if exact_dir.is_dir() && exact_dir.join(manifest_file).is_file() {
        out.push(exact_dir);
    }
}

fn collect_recursive_manifest_dirs(
    dir: &Path,
    manifest_file: &str,
    depth: usize,
    out: &mut Vec<PathBuf>,
) {
    if depth > 6 || !dir.is_dir() {
        return;
    }
    if depth > 0 && dir.join(manifest_file).is_file() {
        out.push(dir.to_path_buf());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if super::layout::should_skip_workspace_artifact_dir(&path, &name_str, None) {
            continue;
        }
        collect_recursive_manifest_dirs(&path, manifest_file, depth + 1, out);
    }
}

fn collect_cargo_table_path_deps(
    table: Option<&toml::Value>,
    cargo_root: &Path,
    dirs: &mut HashSet<PathBuf>,
) {
    let Some(tbl) = table.and_then(|v| v.as_table()) else {
        return;
    };
    for val in tbl.values() {
        if let Some(rel) = val.get("path").and_then(|p| p.as_str()) {
            let candidate = cargo_root.join(rel);
            if candidate.is_dir() && candidate.join("Cargo.toml").is_file() {
                dirs.insert(fs::canonicalize(&candidate).unwrap_or(candidate));
            }
        }
    }
}

pub(crate) fn discover_cargo_workspace_member_dirs(cargo_root: &Path) -> HashSet<PathBuf> {
    let toml_path = cargo_root.join("Cargo.toml");
    let Ok(content) = fs::read_to_string(&toml_path) else {
        return HashSet::new();
    };
    let Ok(val) = toml::from_str::<toml::Value>(&content) else {
        return HashSet::new();
    };

    let mut dirs = HashSet::new();
    if let Some(members) = val
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        let mut matched = Vec::new();
        for pattern in members.iter().filter_map(|v| v.as_str()) {
            let trimmed = pattern
                .trim()
                .trim_start_matches("./")
                .trim_end_matches('/');
            if trimmed.is_empty() || trimmed == "." {
                continue;
            }
            matched.clear();
            expand_manifest_glob_pattern(cargo_root, trimmed, "Cargo.toml", &mut matched);
            for p in &matched {
                dirs.insert(fs::canonicalize(p).unwrap_or_else(|_| p.clone()));
            }
        }
    }

    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        collect_cargo_table_path_deps(val.get(key), cargo_root, &mut dirs);
    }
    if let Some(targets) = val.get("target").and_then(|t| t.as_table()) {
        for target_tbl in targets.values() {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                collect_cargo_table_path_deps(target_tbl.get(key), cargo_root, &mut dirs);
            }
        }
    }

    dirs
}

/// Returns `true` when `dir` directly contains a Node/npm/pnpm/yarn project manifest.
pub fn has_node_project_manifest(dir: &Path) -> bool {
    dir.join("package.json").is_file()
        || dir.join("pnpm-workspace.yaml").is_file()
        || dir.join("pnpm-lock.yaml").is_file()
        || dir.join("package-lock.json").is_file()
        || dir.join("yarn.lock").is_file()
}

/// Discovers Node project roots inside `member_dir`: includes `member_dir` if it holds a
/// Node manifest/lockfile AND performs a cycle-safe walk to locate any independent nested
/// Node roots (e.g., `frontend/`) outside declared workspace globs.
pub fn discover_node_project_roots(member_dir: &Path) -> Vec<PathBuf> {
    discover_node_project_roots_excluding(member_dir, &[])
}

/// Like [`discover_node_project_roots`], excluding any directories belonging to `excluded_dirs`
/// (e.g., nested declared umbrella workspace members).
pub fn discover_node_project_roots_excluding(
    member_dir: &Path,
    excluded_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    if !member_dir.is_dir() {
        return Vec::new();
    }
    let Ok(canon_root) = fs::canonicalize(member_dir) else {
        return Vec::new();
    };
    let excluded_canon = canonical_excluded_roots(member_dir, excluded_dirs);
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
    collect_nested_project_roots(
        member_dir,
        &canon_root,
        &excluded_canon,
        0,
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

/// Discovers Cargo manifest directories (`Cargo.toml`, `src-tauri/Cargo.toml`, and nested
/// Cargo/Tauri subprojects) inside `member_dir` using cycle-safe traversal and deduplication.
pub fn discover_tauri_cargo_dirs(member_dir: &Path) -> Vec<PathBuf> {
    discover_tauri_cargo_dirs_excluding(member_dir, &[])
}

/// Like [`discover_tauri_cargo_dirs`], excluding any directories belonging to `excluded_dirs`
/// (e.g., nested declared umbrella workspace members).
pub fn discover_tauri_cargo_dirs_excluding(
    member_dir: &Path,
    excluded_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    if !member_dir.is_dir() {
        return Vec::new();
    }
    let Ok(canon_root) = fs::canonicalize(member_dir) else {
        return Vec::new();
    };
    let excluded_canon = canonical_excluded_roots(member_dir, excluded_dirs);
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

    let declared_ws_members = if member_dir.join("Cargo.toml").is_file() {
        discover_cargo_workspace_member_dirs(member_dir)
    } else {
        HashSet::new()
    };

    let mut visited = HashSet::new();
    visited.insert(canon_root.clone());
    let mut nested = Vec::new();
    collect_nested_project_roots(
        member_dir,
        &canon_root,
        &excluded_canon,
        0,
        &mut visited,
        &mut nested,
        &|dir| dir.join("Cargo.toml").is_file(),
        true,
    );
    nested.sort();
    for cargo_dir in nested {
        let canon_dir = fs::canonicalize(&cargo_dir).unwrap_or_else(|_| cargo_dir.clone());
        let is_declared_ws_member = declared_ws_members
            .iter()
            .any(|ws_dir| canon_dir == *ws_dir || canon_dir.starts_with(ws_dir));
        if is_declared_ws_member {
            continue;
        }
        let toml = cargo_dir.join("Cargo.toml");
        let canon_toml = fs::canonicalize(&toml).unwrap_or(toml);
        if seen_manifests.insert(canon_toml) {
            dirs.push(cargo_dir);
        }
    }

    dirs
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_project_roots<F>(
    dir: &Path,
    canon_root: &Path,
    excluded_canon: &[PathBuf],
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
    predicate: &F,
    stop_on_match: bool,
) where
    F: Fn(&Path) -> bool,
{
    if depth > MAX_PROJECT_DISCOVERY_DEPTH {
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
        if !canon_child.starts_with(canon_root)
            || is_excluded_path(&canon_child, excluded_canon)
            || !visited.insert(canon_child)
        {
            continue;
        }
        child_dirs.push(path);
    }
    child_dirs.sort();
    for child in child_dirs {
        collect_nested_project_roots(
            &child,
            canon_root,
            excluded_canon,
            depth.saturating_add(1),
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
#[path = "discovery_tests.rs"]
mod tests;
