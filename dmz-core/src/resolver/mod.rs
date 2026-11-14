pub mod fetch;
pub mod lockfile;
pub mod manifest;

use crate::resolver::manifest::Ecosystem;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use tracing::info;

pub use fetch::DependencyFetcher;
pub use lockfile::{
    DmzLockfile, LockError, LockedPackage, LockfileReader, LockfileWriter, MAX_LOCKFILE_SIZE,
};
pub use manifest::{DependencySpec, ManifestError, ManifestInfo, ManifestScanner};

#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("Resolver IO Error: {0}")]
    Io(String),
    #[error("Resolver Lockfile Error: {0}")]
    Lockfile(String),
    #[error("Resolver Network Error: {0}")]
    Network(String),
    #[error("Resolver Parse Error: {0}")]
    Parse(String),
    #[error("Resolver Command Error: {0}")]
    Command(String),
    #[error("Fail-Fast Guardrail: Workspace scan detected manifests, but resolved 0 packages.")]
    EmptyResolution,
    #[error("Manifest Scan Error: {0}")]
    Manifest(#[from] ManifestError),
    #[error("Lockfile Error: {0}")]
    Lock(#[from] LockError),
}

impl From<ResolverError> for String {
    fn from(err: ResolverError) -> Self {
        err.to_string()
    }
}

/// Configuration options controlling dependency resolution scope and filtering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveOptions {
    pub target_platform: Option<String>,
    pub include_dev: bool,
}

pub struct ResolverEngine;

impl ResolverEngine {
    /// Scans project workspace, auto-resolves dependencies across ecosystems, and produces a cryptographic lockfile
    pub fn resolve_workspace(workspace_dir: &Path) -> Result<DmzLockfile, ResolverError> {
        Self::resolve_workspace_with_options(workspace_dir, ResolveOptions::default())
    }

    /// Scans project workspace, auto-resolves dependencies, and tags the target platform
    pub fn resolve_workspace_with_target(
        workspace_dir: &Path,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, ResolverError> {
        Self::resolve_workspace_with_options(
            workspace_dir,
            ResolveOptions {
                target_platform,
                include_dev: false,
            },
        )
    }

    /// Scans project workspace with full control over target platform and dev-dependency inclusion.
    pub fn resolve_workspace_with_options(
        workspace_dir: &Path,
        options: ResolveOptions,
    ) -> Result<DmzLockfile, ResolverError> {
        let manifest_path = workspace_dir.join("dmz-workspace.toml");
        if manifest_path.is_file() {
            if let Ok(manifest) =
                crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
            {
                if !manifest.members.is_empty() {
                    return Self::resolve_multi_workspace_with_options(
                        workspace_dir,
                        &manifest,
                        None,
                        &options,
                    );
                }
            }
        }

        Self::resolve_single_workspace_with_options(workspace_dir, &options)
    }

    /// Resolves dependencies across all members of a multi-repository `WorkspaceProjectConfig`,
    /// tagging each package with its canonical `member_id` and recording workspace metadata.
    pub fn resolve_multi_workspace(
        workspace_dir: &Path,
        project: &crate::workspace::WorkspaceProjectConfig,
        manifest_path: Option<&Path>,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, ResolverError> {
        let options = ResolveOptions {
            target_platform,
            include_dev: false,
        };
        Self::resolve_multi_workspace_with_options(workspace_dir, project, manifest_path, &options)
    }

    /// Resolves dependencies across all members of a multi-repository workspace with explicit options.
    pub fn resolve_multi_workspace_with_options(
        workspace_dir: &Path,
        project: &crate::workspace::WorkspaceProjectConfig,
        manifest_path: Option<&Path>,
        options: &ResolveOptions,
    ) -> Result<DmzLockfile, ResolverError> {
        let layouts = project.resolve_member_layouts(workspace_dir, manifest_path);
        if layouts.is_empty() {
            return Self::resolve_single_workspace_with_options(workspace_dir, options);
        }

        info!(
            project = %project.name,
            members = layouts.len(),
            include_dev = options.include_dev,
            "Resolving multi-workspace dependencies across all declared members"
        );

        let mut merged_packages = Vec::new();
        let mut workspaces_meta = Vec::new();

        for layout in &layouts {
            if !layout.host_path.is_dir() {
                return Err(ResolverError::Io(format!(
                    "Workspace member '{}' directory does not exist at {:?}",
                    layout.id, layout.host_path
                )));
            }

            let member_cfg = project.find_member(&layout.id);
            match Self::resolve_single_workspace_with_options(&layout.host_path, options) {
                Ok(member_lock) => {
                    let member_sig = member_lock.closure_signature.clone();
                    for (_, mut pkg) in member_lock.packages {
                        pkg.member_id = Some(layout.id.clone());
                        merged_packages.push(pkg);
                    }
                    workspaces_meta.push(crate::resolver::lockfile::WorkspaceMetadata {
                        id: layout.id.clone(),
                        path: member_cfg
                            .map(|m| m.path.display().to_string())
                            .unwrap_or_else(|| layout.host_path.display().to_string()),
                        role: layout.role.to_string(),
                        ecosystem: member_cfg.and_then(|m| m.ecosystem.clone()),
                        closure_signature: Some(member_sig),
                    });
                }
                Err(ResolverError::EmptyResolution) if layouts.len() > 1 => {
                    info!(
                        member = %layout.id,
                        path = ?layout.host_path,
                        "Member has 0 external dependencies; continuing multi-workspace resolution"
                    );
                    workspaces_meta.push(crate::resolver::lockfile::WorkspaceMetadata {
                        id: layout.id.clone(),
                        path: member_cfg
                            .map(|m| m.path.display().to_string())
                            .unwrap_or_else(|| layout.host_path.display().to_string()),
                        role: layout.role.to_string(),
                        ecosystem: member_cfg.and_then(|m| m.ecosystem.clone()),
                        closure_signature: None,
                    });
                }
                Err(e) => {
                    return Err(ResolverError::Lockfile(format!(
                        "Failed to resolve member '{}' at {:?}: {}",
                        layout.id, layout.host_path, e
                    )));
                }
            }
        }

        let mut unified_lock =
            DmzLockfile::with_platform(merged_packages, options.target_platform.clone());
        for meta in workspaces_meta {
            unified_lock.add_workspace(meta);
        }

        if let Ok(Some(prov)) = crate::vcs::GitProvider::get_provenance(workspace_dir) {
            unified_lock.vcs = Some(prov);
        }

        info!(
            closure_sig = %unified_lock.closure_signature,
            packages = unified_lock.packages.len(),
            workspaces = unified_lock.workspaces.len(),
            "Successfully resolved multi-workspace project into unified lockfile"
        );

        Ok(unified_lock)
    }

    /// Resolves multi-workspace projects by resolving each declared member and merging into a unified lockfile
    pub fn resolve_multi_workspace_with_target(
        workspace_dir: &Path,
        manifest: &crate::workspace::WorkspaceProjectConfig,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, ResolverError> {
        Self::resolve_multi_workspace(workspace_dir, manifest, None, target_platform)
    }

    /// Scans a single workspace directory, auto-resolves dependencies, and tags the target platform
    pub fn resolve_single_workspace_with_target(
        workspace_dir: &Path,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, ResolverError> {
        Self::resolve_single_workspace_with_options(
            workspace_dir,
            &ResolveOptions {
                target_platform,
                include_dev: false,
            },
        )
    }

    /// Scans a single workspace directory with full resolution options.
    pub fn resolve_single_workspace_with_options(
        workspace_dir: &Path,
        options: &ResolveOptions,
    ) -> Result<DmzLockfile, ResolverError> {
        info!(
            path = ?workspace_dir,
            include_dev = options.include_dev,
            target_platform = ?options.target_platform,
            "Scanning workspace and auto-resolving dependencies across ecosystems..."
        );

        let mut locked_packages = Vec::new();

        // Native Cargo Auto-Resolution (Root Cargo.toml or nested src-tauri/Cargo.toml)
        let cargo_toml_path = workspace_dir.join("Cargo.toml");
        let tauri_cargo_path = workspace_dir.join("src-tauri/Cargo.toml");
        let effective_cargo_dir = if cargo_toml_path.exists() {
            Some(workspace_dir.to_path_buf())
        } else if tauri_cargo_path.exists() {
            Some(workspace_dir.join("src-tauri"))
        } else {
            None
        };

        if let Some(ref cargo_dir) = effective_cargo_dir {
            locked_packages.extend(Self::resolve_cargo_graph(cargo_dir, options)?);
        }

        // Scan remaining non-Cargo ecosystems using options-aware ManifestScanner
        let manifests = crate::resolver::manifest::ManifestScanner::scan_dir_with_options(
            workspace_dir,
            options.target_platform.as_deref(),
            options.include_dev,
        )?;

        for manifest in manifests {
            // When `cargo metadata` already resolved the Cargo graph, do not re-inject
            // unpinned workspace/Cargo.toml ranges (which cause duplicate serde:1.0 entries)
            // or dev-dependencies excluded from the graph.
            if manifest.ecosystem == Ecosystem::Cargo && effective_cargo_dir.is_some() {
                continue;
            }
            info!(ecosystem = ?manifest.ecosystem, path = ?manifest.path, "Processing manifest");

            for dep in manifest.dependencies {
                if dep.is_dev && !options.include_dev {
                    continue;
                }
                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", dep.name, dep.version_req, dep.ecosystem).as_bytes(),
                );

                if !locked_packages.iter().any(|p: &LockedPackage| {
                    p.name == dep.name
                        && p.version == dep.version_req
                        && p.ecosystem == dep.ecosystem
                }) {
                    locked_packages.push(LockedPackage {
                        name: dep.name,
                        version: dep.version_req,
                        ecosystem: dep.ecosystem,
                        sha256: dummy_hash,
                        source_url: None,
                        dependencies: dep.dependencies,
                        member_id: None,
                    });
                }
            }
        }

        if locked_packages.is_empty() {
            return Err(ResolverError::EmptyResolution);
        }

        Self::append_tauri_cli_companion(&mut locked_packages, options.target_platform.as_deref());

        let mut lockfile =
            DmzLockfile::with_platform(locked_packages, options.target_platform.clone());
        if let Ok(Some(prov)) = crate::vcs::GitProvider::get_provenance(workspace_dir) {
            info!(
                commit = %prov.commit,
                branch = ?prov.branch,
                dirty = prov.dirty,
                "Attached VCS provenance to dmz.lock"
            );
            lockfile.vcs = Some(prov);
        }
        info!(closure_sig = %lockfile.closure_signature, count = lockfile.packages.len(), "Generated deterministic agnostic DMZ closure signature");

        Ok(lockfile)
    }

    fn resolve_cargo_graph(
        cargo_dir: &Path,
        options: &ResolveOptions,
    ) -> Result<Vec<LockedPackage>, ResolverError> {
        let target_triple =
            crate::workspace::resolve_rust_target_triple(options.target_platform.as_deref());
        info!(
            cargo_dir = ?cargo_dir,
            target_triple = ?target_triple,
            include_dev = options.include_dev,
            "Cargo.toml detected; invoking `cargo metadata` for graph resolution..."
        );

        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("metadata").arg("--format-version").arg("1");
        if let Some(ref triple) = target_triple {
            cmd.arg("--filter-platform").arg(triple);
        }
        let output = cmd.current_dir(cargo_dir).output().map_err(|e| {
            ResolverError::Command(format!("Failed to execute `cargo metadata`: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResolverError::Command(format!(
                "Cargo metadata resolution failed:\n{}",
                stderr
            )));
        }

        let json_meta: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
            ResolverError::Parse(format!("Failed to parse cargo metadata JSON: {}", e))
        })?;

        let workspace_member_ids: HashSet<String> = json_meta
            .get("workspace_members")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let reachable_ids = Self::collect_reachable_cargo_packages(
            &json_meta,
            &workspace_member_ids,
            options.include_dev,
        );

        let mut locked = Vec::new();
        if let Some(packages) = json_meta.get("packages").and_then(|p| p.as_array()) {
            for pkg in packages {
                let id = pkg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                if let Some(ref reachable) = reachable_ids {
                    if !reachable.contains(id) {
                        continue;
                    }
                }
                let name = pkg
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                let version = pkg
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let manifest_path = pkg
                    .get("manifest_path")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default();

                let is_workspace_member = workspace_member_ids.contains(id);
                let version_req = if is_workspace_member {
                    format!("path:{}", manifest_path)
                } else {
                    version
                };

                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", name, version_req, Ecosystem::Cargo).as_bytes(),
                );

                if !locked
                    .iter()
                    .any(|p: &LockedPackage| p.name == name && p.version == version_req)
                {
                    locked.push(LockedPackage {
                        name,
                        version: version_req,
                        ecosystem: Ecosystem::Cargo,
                        sha256: dummy_hash,
                        source_url: None,
                        dependencies: Vec::new(),
                        member_id: None,
                    });
                }
            }
        }
        Ok(locked)
    }

    fn collect_reachable_cargo_packages(
        json_meta: &serde_json::Value,
        workspace_member_ids: &HashSet<String>,
        include_dev: bool,
    ) -> Option<HashSet<String>> {
        let nodes = json_meta
            .get("resolve")
            .and_then(|r| r.get("nodes"))
            .and_then(|n| n.as_array())?;

        let mut node_map: HashMap<&str, &serde_json::Value> = HashMap::new();
        for node in nodes {
            if let Some(id) = node.get("id").and_then(|i| i.as_str()) {
                node_map.insert(id, node);
            }
        }

        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();
        for member_id in workspace_member_ids {
            if reachable.insert(member_id.clone()) {
                queue.push_back(member_id.clone());
            }
        }
        if queue.is_empty() {
            if let Some(root_id) = json_meta
                .get("resolve")
                .and_then(|r| r.get("root"))
                .and_then(|r| r.as_str())
            {
                if reachable.insert(root_id.to_string()) {
                    queue.push_back(root_id.to_string());
                }
            }
        }

        while let Some(curr_id) = queue.pop_front() {
            let Some(node) = node_map.get(curr_id.as_str()) else {
                continue;
            };
            let Some(deps) = node.get("deps").and_then(|d| d.as_array()) else {
                continue;
            };
            for dep in deps {
                let keep_edge = include_dev
                    || dep
                        .get("dep_kinds")
                        .and_then(|k| k.as_array())
                        .map(|kinds| {
                            kinds.is_empty()
                                || kinds
                                    .iter()
                                    .any(|k| k.get("kind").and_then(|v| v.as_str()) != Some("dev"))
                        })
                        .unwrap_or(true);
                if !keep_edge {
                    continue;
                }
                if let Some(dep_pkg) = dep.get("pkg").and_then(|p| p.as_str()) {
                    if reachable.insert(dep_pkg.to_string()) {
                        queue.push_back(dep_pkg.to_string());
                    }
                }
            }
        }

        Some(reachable)
    }

    fn append_tauri_cli_companion(
        locked_packages: &mut Vec<LockedPackage>,
        target_platform: Option<&str>,
    ) {
        let (target_os, target_arch) = crate::workspace::resolve_target_os_arch(target_platform);
        let platform_suffix = match (target_os, target_arch) {
            ("macos", "aarch64") => "darwin-arm64".to_string(),
            ("macos", "x86_64") => "darwin-x64".to_string(),
            ("linux", "x86_64") => "linux-x64-gnu".to_string(),
            ("linux", "aarch64") => "linux-arm64-gnu".to_string(),
            ("windows", "x86_64") => "win32-x64-msvc".to_string(),
            ("windows", "aarch64") => "win32-arm64-msvc".to_string(),
            (other_os, other_arch) => {
                let os_str = if other_os == "macos" {
                    "darwin"
                } else {
                    other_os
                };
                let arch_str = if other_arch == "aarch64" {
                    "arm64"
                } else if other_arch == "x86_64" {
                    "x64"
                } else {
                    other_arch
                };
                format!("{}-{}", os_str, arch_str)
            }
        };

        let mut companion_pkgs = Vec::new();
        for pkg in locked_packages.iter() {
            if pkg.name == "@tauri-apps/cli" {
                let companion_name = format!("@tauri-apps/cli-{}", platform_suffix);
                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", companion_name, pkg.version, Ecosystem::Npm).as_bytes(),
                );
                if !locked_packages.iter().any(|p| p.name == companion_name) {
                    companion_pkgs.push(LockedPackage {
                        name: companion_name,
                        version: pkg.version.clone(),
                        ecosystem: Ecosystem::Npm,
                        sha256: dummy_hash,
                        source_url: None,
                        dependencies: Vec::new(),
                        member_id: pkg.member_id.clone(),
                    });
                }
            }
        }
        locked_packages.extend(companion_pkgs);
    }

    /// Fetches locked dependencies from dmz.lock, downloads real package artifacts, verifies hashes, and populates the local store cache
    pub fn fetch_dependencies(
        lockfile_path: &Path,
        store_path: &Path,
    ) -> Result<usize, ResolverError> {
        fetch::DependencyFetcher::fetch_dependencies(lockfile_path, store_path)
    }

    /// Evaluates Python wheel ABI tags and selects the best matching artifact for target platform
    pub fn select_pypi_artifact(
        urls: &[serde_json::Value],
        target_platform: Option<&str>,
    ) -> Option<String> {
        fetch::DependencyFetcher::select_pypi_artifact(urls, target_platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_collect_reachable_cargo_packages_prunes_dev_closure() {
        let meta = serde_json::json!({
            "resolve": {
                "root": "app 0.1.0",
                "nodes": [
                    {
                        "id": "app 0.1.0",
                        "deps": [
                            { "pkg": "runtime-lib 1.0.0", "dep_kinds": [{ "kind": null }] },
                            { "pkg": "shared-lib 1.0.0", "dep_kinds": [{ "kind": null }, { "kind": "dev" }] },
                            { "pkg": "test-harness 1.0.0", "dep_kinds": [{ "kind": "dev" }] }
                        ]
                    },
                    {
                        "id": "runtime-lib 1.0.0",
                        "deps": []
                    },
                    {
                        "id": "shared-lib 1.0.0",
                        "deps": []
                    },
                    {
                        "id": "test-harness 1.0.0",
                        "deps": [
                            { "pkg": "dev-transitive 1.0.0", "dep_kinds": [{ "kind": null }] }
                        ]
                    },
                    {
                        "id": "dev-transitive 1.0.0",
                        "deps": []
                    }
                ]
            }
        });
        let members: HashSet<String> = ["app 0.1.0".to_string()].into_iter().collect();

        let prod = ResolverEngine::collect_reachable_cargo_packages(&meta, &members, false);
        assert!(prod.is_some());
        if let Some(prod_set) = prod {
            assert!(prod_set.contains("runtime-lib 1.0.0"));
            assert!(prod_set.contains("shared-lib 1.0.0"));
            assert!(!prod_set.contains("test-harness 1.0.0"));
            assert!(!prod_set.contains("dev-transitive 1.0.0"));
        }

        let with_dev = ResolverEngine::collect_reachable_cargo_packages(&meta, &members, true);
        assert!(with_dev.is_some());
        if let Some(dev_set) = with_dev {
            assert!(dev_set.contains("runtime-lib 1.0.0"));
            assert!(dev_set.contains("shared-lib 1.0.0"));
            assert!(dev_set.contains("test-harness 1.0.0"));
            assert!(dev_set.contains("dev-transitive 1.0.0"));
        }
    }

    #[test]
    fn test_cargo_filter_platform_and_dev_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let root = tmp.path();

        for crate_name in ["linux-only", "win-only", "dev-only"] {
            let dir = root.join(crate_name);
            fs::create_dir_all(dir.join("src"))?;
            fs::write(
                dir.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                    crate_name
                ),
            )?;
            fs::write(dir.join("src/lib.rs"), "")?;
        }

        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            r#"
[package]
name = "platform-app"
version = "0.1.0"
edition = "2021"

[target.'cfg(target_os = "linux")'.dependencies]
linux-only = { path = "linux-only" }

[target.'cfg(target_os = "windows")'.dependencies]
win-only = { path = "win-only" }

[dev-dependencies]
dev-only = { path = "dev-only" }
"#,
        )?;
        fs::write(root.join("src/lib.rs"), "")?;

        let linux_prod = ResolverEngine::resolve_single_workspace_with_options(
            root,
            &ResolveOptions {
                target_platform: Some("linux-x64".to_string()),
                include_dev: false,
            },
        )?;
        let linux_names: Vec<&str> = linux_prod
            .packages
            .values()
            .map(|p| p.name.as_str())
            .collect();
        assert!(linux_names.contains(&"linux-only"));
        assert!(!linux_names.contains(&"win-only"));
        assert!(!linux_names.contains(&"dev-only"));

        let win_with_dev = ResolverEngine::resolve_single_workspace_with_options(
            root,
            &ResolveOptions {
                target_platform: Some("windows-x64".to_string()),
                include_dev: true,
            },
        )?;
        let win_names: Vec<&str> = win_with_dev
            .packages
            .values()
            .map(|p| p.name.as_str())
            .collect();
        assert!(win_names.contains(&"win-only"));
        assert!(!win_names.contains(&"linux-only"));
        assert!(win_names.contains(&"dev-only"));

        Ok(())
    }
}
