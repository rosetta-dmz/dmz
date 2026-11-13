pub mod fetch;
pub mod lockfile;
pub mod manifest;

use crate::resolver::manifest::Ecosystem;
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

pub struct ResolverEngine;

impl ResolverEngine {
    /// Scans project workspace, auto-resolves dependencies across ecosystems, and produces a cryptographic lockfile
    pub fn resolve_workspace(workspace_dir: &Path) -> Result<DmzLockfile, ResolverError> {
        Self::resolve_workspace_with_target(workspace_dir, None)
    }

    /// Scans project workspace, auto-resolves dependencies, and tags the target platform
    pub fn resolve_workspace_with_target(
        workspace_dir: &Path,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, ResolverError> {
        let manifest_path = workspace_dir.join("dmz-workspace.toml");
        if manifest_path.is_file() {
            if let Ok(manifest) =
                crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
            {
                if !manifest.members.is_empty() {
                    return Self::resolve_multi_workspace_with_target(
                        workspace_dir,
                        &manifest,
                        target_platform,
                    );
                }
            }
        }

        Self::resolve_single_workspace_with_target(workspace_dir, target_platform)
    }

    /// Resolves dependencies across all members of a multi-repository `WorkspaceProjectConfig`,
    /// tagging each package with its canonical `member_id` and recording workspace metadata.
    pub fn resolve_multi_workspace(
        workspace_dir: &Path,
        project: &crate::workspace::WorkspaceProjectConfig,
        manifest_path: Option<&Path>,
        target_platform: Option<String>,
    ) -> Result<DmzLockfile, ResolverError> {
        let layouts = project.resolve_member_layouts(workspace_dir, manifest_path);
        if layouts.is_empty() {
            return Self::resolve_single_workspace_with_target(workspace_dir, target_platform);
        }

        info!(
            project = %project.name,
            members = layouts.len(),
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
            match Self::resolve_single_workspace_with_target(
                &layout.host_path,
                target_platform.clone(),
            ) {
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

        let mut unified_lock = DmzLockfile::with_platform(merged_packages, target_platform);
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
        info!(path = ?workspace_dir, "Scanning workspace and auto-resolving dependencies across ecosystems...");

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
            info!(
                cargo_dir = ?cargo_dir,
                "Cargo.toml detected; invoking `cargo metadata` for full graph auto-resolution..."
            );
            let output = std::process::Command::new("cargo")
                .arg("metadata")
                .arg("--format-version")
                .arg("1")
                .current_dir(cargo_dir)
                .output()
                .map_err(|e| {
                    ResolverError::Command(format!("Failed to execute `cargo metadata`: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ResolverError::Command(format!(
                    "Cargo metadata resolution failed:\n{}",
                    stderr
                )));
            }

            let json_meta: serde_json::Value =
                serde_json::from_slice(&output.stdout).map_err(|e| {
                    ResolverError::Parse(format!("Failed to parse cargo metadata JSON: {}", e))
                })?;

            // Collect workspace member IDs for accurate local path identification
            let workspace_member_ids: Vec<String> = json_meta
                .get("workspace_members")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            if let Some(packages) = json_meta.get("packages").and_then(|p| p.as_array()) {
                for pkg in packages {
                    let id = pkg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
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

                    // Check if this package is a workspace member
                    let is_workspace_member = workspace_member_ids.iter().any(|m_id| m_id == id);

                    let version_req = if is_workspace_member {
                        format!("path:{}", manifest_path)
                    } else {
                        version
                    };

                    let dummy_hash = DmzLockfile::hash_bytes(
                        format!("{}:{}:{:?}", name, version_req, Ecosystem::Cargo).as_bytes(),
                    );

                    if !locked_packages
                        .iter()
                        .any(|p: &LockedPackage| p.name == name && p.version == version_req)
                    {
                        locked_packages.push(LockedPackage {
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
        }

        // Scan remaining non-Cargo ecosystems using the standard ManifestScanner
        let manifests = crate::resolver::manifest::ManifestScanner::scan_dir(workspace_dir)?;

        for manifest in manifests {
            info!(ecosystem = ?manifest.ecosystem, path = ?manifest.path, "Processing manifest");

            for dep in manifest.dependencies {
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

        // Auto-resolve companion native binary package for @tauri-apps/cli
        let mut companion_pkgs = Vec::new();
        for pkg in &locked_packages {
            if pkg.name == "@tauri-apps/cli" {
                let platform_suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
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

        let mut lockfile = DmzLockfile::with_platform(locked_packages, target_platform);
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
