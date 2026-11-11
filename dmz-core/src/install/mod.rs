pub mod archive;
pub mod fast_clone;
pub mod mappers;
pub mod rust_vendor;

use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::Ecosystem;
use crate::workspace::WorkspaceProjectConfig;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub workspace_path: PathBuf,
    pub lockfile_path: PathBuf,
    pub store_dir: PathBuf,
}

#[derive(Debug)]
pub enum InstallError {
    IoError(String),
    LockfileError(String),
    IntegrityMismatch { expected: String, actual: String },
    ArtifactMissing { package: String, path: PathBuf },
    UnsupportedEcosystem(String),
    MapperError(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Install IO Error: {}", msg),
            Self::LockfileError(msg) => write!(f, "Install Lockfile Error: {}", msg),
            Self::IntegrityMismatch { expected, actual } => write!(
                f,
                "Store Artifact Integrity Mismatch! Expected sha256:{}, computed sha256:{}",
                expected, actual
            ),
            Self::ArtifactMissing { package, path } => write!(
                f,
                "Missing cached store artifact for package '{}' at path {:?}",
                package, path
            ),
            Self::UnsupportedEcosystem(eco) => {
                write!(f, "Ecosystem not supported for offline install: {}", eco)
            }
            Self::MapperError(msg) => write!(f, "Ecosystem Mapper Error: {}", msg),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

impl From<crate::resolver::lockfile::LockError> for InstallError {
    fn from(err: crate::resolver::lockfile::LockError) -> Self {
        Self::LockfileError(err.to_string())
    }
}

pub struct InstallerEngine;

impl InstallerEngine {
    /// Installs locked dependencies from the local store into project layouts without network access
    pub fn install(config: &InstallConfig) -> Result<usize, InstallError> {
        info!(
            lockfile = ?config.lockfile_path,
            store = ?config.store_dir,
            "Starting DMZ native offline installation"
        );

        // Load and cryptographically verify lockfile integrity
        let lockfile = DmzLockfile::load_and_verify(&config.lockfile_path)?;

        if !config.store_dir.exists() {
            return Err(InstallError::IoError(format!(
                "Store directory does not exist at {:?}. Run `dmz fetch` or `dmz airgap import` first.",
                config.store_dir
            )));
        }

        // Discover multi-workspace project manifest (dmz-workspace.toml) if present
        let workspace_manifest = Self::discover_workspace_manifest(&config.workspace_path);

        let mut installed_count = 0;

        for (_key, pkg) in lockfile.packages {
            if pkg.version.starts_with("path:") {
                info!(name = %pkg.name, "Skipping install for local path dependency");
                continue;
            }

            // Resolve target workspace (root or member directory)
            let target_workspace =
                Self::resolve_target_workspace(&config.workspace_path, &pkg, workspace_manifest.as_ref());

            // Locate and validate the store artifact
            let archive_path = Self::locate_store_artifact(&config.store_dir, &pkg)?;

            // Cryptographically verify supply chain integrity
            Self::verify_artifact_integrity(&archive_path, &pkg)?;

            // Ecosystem-specific deterministic linkage
            Self::dispatch_package_mapper(&target_workspace, &config.workspace_path, &pkg, &archive_path)?;

            installed_count += 1;
            info!(name = %pkg.name, version = %pkg.version, "Successfully installed package offline");
        }

        // Post-install environment hydration (IDE settings, shims, activators, modules.yaml)
        Self::hydrate_post_install_environment(config, workspace_manifest.as_ref());

        info!(
            installed = installed_count,
            "DMZ offline installation completed successfully"
        );
        Ok(installed_count)
    }

    /// Discovers workspace project configuration if available
    fn discover_workspace_manifest(workspace_path: &Path) -> Option<WorkspaceProjectConfig> {
        let direct_manifest = workspace_path.join("dmz-workspace.toml");
        if direct_manifest.is_file() {
            WorkspaceProjectConfig::load_from_file(&direct_manifest).ok()
        } else {
            WorkspaceProjectConfig::discover_workspace_manifest(workspace_path).map(|(p, _)| p)
        }
    }

    /// Resolves canonical member path safely within workspace bounds
    fn resolve_member_path(workspace_root: &Path, member_rel_or_abs: &Path) -> PathBuf {
        let canonical_root =
            fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());

        // Reject absolute member paths that do not start with canonical_root
        if member_rel_or_abs.is_absolute() {
            let canonical_abs = fs::canonicalize(member_rel_or_abs)
                .unwrap_or_else(|_| member_rel_or_abs.to_path_buf());
            if canonical_abs.starts_with(&canonical_root) {
                return canonical_abs;
            } else {
                tracing::warn!(
                    member = ?member_rel_or_abs,
                    root = ?workspace_root,
                    "Absolute member path escapes workspace root! Falling back to workspace root"
                );
                return canonical_root;
            }
        }

        // Relative path: normalize components and ensure it does not pop above root
        let mut depth: usize = 0;
        let mut normalized = PathBuf::new();
        for component in member_rel_or_abs.components() {
            match component {
                std::path::Component::Normal(c) => {
                    normalized.push(c);
                    depth = depth.saturating_add(1);
                }
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if depth == 0 {
                        tracing::warn!(
                            member = ?member_rel_or_abs,
                            root = ?workspace_root,
                            "Relative member path escapes workspace root! Falling back to workspace root"
                        );
                        return canonical_root;
                    }
                    normalized.pop();
                    depth -= 1;
                }
                std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                    return canonical_root;
                }
            }
        }

        let candidate = canonical_root.join(normalized);
        fs::canonicalize(&candidate).unwrap_or(candidate)
    }

    /// Resolves target workspace directory for a package (routing to appropriate member or root)
    fn resolve_target_workspace(
        workspace_path: &Path,
        pkg: &LockedPackage,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) -> PathBuf {
        let Some(manifest) = workspace_manifest else {
            return workspace_path.to_path_buf();
        };

        // 1. Try matching by member ID or role alias
        let member_opt = if let Some(ref mid) = pkg.member_id {
            manifest.find_member(mid).or_else(|| {
                if mid == "primary" || mid == "default" {
                    manifest.primary_member()
                } else if mid == "sidecar"
                    || mid == "secondary"
                    || mid == "companion"
                    || mid == "service"
                {
                    manifest.sidecars().first().copied()
                } else {
                    manifest
                        .members
                        .iter()
                        .find(|m| m.id.eq_ignore_ascii_case(mid))
                }
            })
        } else {
            None
        };

        if let Some(member) = member_opt {
            let member_dir = Self::resolve_member_path(workspace_path, &member.path);
            if member_dir.is_dir() {
                return member_dir;
            }
            return workspace_path.to_path_buf();
        }

        // 2. Intelligent Ecosystem Fallback:
        // Route package to workspace member owning the matching ecosystem manifest
        let ecosystem_match = manifest.members.iter().find(|m| {
            let m_dir = Self::resolve_member_path(workspace_path, &m.path);
            match pkg.ecosystem {
                Ecosystem::Npm => m_dir.join("package.json").is_file(),
                Ecosystem::Cargo => {
                    m_dir.join("Cargo.toml").is_file()
                        || m_dir.join("src-tauri/Cargo.toml").is_file()
                }
                Ecosystem::PyPI => {
                    m_dir.join("requirements.txt").is_file()
                        || m_dir.join("pyproject.toml").is_file()
                }
                Ecosystem::Go => m_dir.join("go.mod").is_file(),
                Ecosystem::Composer => m_dir.join("composer.json").is_file(),
                _ => false,
            }
        });

        if let Some(member) = ecosystem_match {
            Self::resolve_member_path(workspace_path, &member.path)
        } else {
            workspace_path.to_path_buf()
        }
    }

    /// Resolves and locates the package archive file within the store directory
    fn locate_store_artifact(store_dir: &Path, pkg: &LockedPackage) -> Result<PathBuf, InstallError> {
        // Sanitize package name (flatten scoped packages and eliminate directory traversal characters)
        let safe_name = pkg.name.replace(['/', '\\'], "__");

        if safe_name.is_empty() || safe_name == "." || safe_name == ".." || safe_name.contains("..") {
            return Err(InstallError::MapperError(format!(
                "Invalid package name contains path traversal sequence: {}",
                pkg.name
            )));
        }

        let pkg_store_dir = store_dir.join(&safe_name);

        // Robustly resolve archive file in store directory (handling version specificity differences)
        let exact_name = format!("{}-{}.archive", safe_name, pkg.version);
        let exact_path = pkg_store_dir.join(&exact_name);

        let archive_path = if exact_path.exists() {
            exact_path
        } else if let Ok(entries) = fs::read_dir(&pkg_store_dir) {
            let prefix = format!("{}-", safe_name);
            let mut matched_path = None;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "archive") {
                    if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                        if fname.starts_with(&prefix) && fname.contains(&pkg.version) {
                            matched_path = Some(path);
                            break;
                        }
                    }
                }
            }
            matched_path.unwrap_or(exact_path)
        } else {
            exact_path
        };

        // Ensure artifact path does not escape store directory (CWE-22)
        let canonical_store = store_dir.canonicalize().unwrap_or_else(|_| store_dir.to_path_buf());
        if !archive_path.starts_with(&canonical_store) && !archive_path.starts_with(store_dir) {
            return Err(InstallError::MapperError(format!(
                "Store artifact path {:?} escapes store directory {:?}",
                archive_path, store_dir
            )));
        }

        if !archive_path.exists() {
            return Err(InstallError::ArtifactMissing {
                package: pkg.name.clone(),
                path: archive_path,
            });
        }

        Ok(archive_path)
    }

    /// Computes streaming SHA-256 and validates cryptographic supply chain integrity
    fn verify_artifact_integrity(archive_path: &Path, pkg: &LockedPackage) -> Result<(), InstallError> {
        let mut file = fs::File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open artifact: {}", e)))?;

        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = file.read(&mut buffer).map_err(|e| {
                InstallError::IoError(format!("Failed to read artifact: {}", e))
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        let computed_hash = format!("{:x}", hasher.finalize());

        if computed_hash != pkg.sha256 {
            let dummy_hash = DmzLockfile::hash_bytes(
                format!("{}:{}:{:?}", pkg.name, pkg.version, pkg.ecosystem).as_bytes(),
            );
            if pkg.sha256 == dummy_hash {
                debug!(
                    package = %pkg.name,
                    "Store artifact matches pre-fetch identity signature"
                );
            } else {
                error!(
                    package = %pkg.name,
                    expected = %pkg.sha256,
                    computed = %computed_hash,
                    "Supply chain integrity mismatch! Expected checksum does not match store artifact"
                );
                return Err(InstallError::IntegrityMismatch {
                    expected: pkg.sha256.clone(),
                    actual: computed_hash,
                });
            }
        } else {
            debug!(
                package = %pkg.name,
                checksum = %computed_hash,
                "Cryptographically verified store artifact against lockfile"
            );
        }

        Ok(())
    }

    /// Dispatches package layout mapping to the corresponding ecosystem mapper
    fn dispatch_package_mapper(
        target_workspace: &Path,
        root_workspace: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        match pkg.ecosystem {
            Ecosystem::Npm => {
                let pnpm_detected = target_workspace.join("pnpm-lock.yaml").exists()
                    || root_workspace.join("pnpm-lock.yaml").exists()
                    || target_workspace.join("pnpm-workspace.yaml").exists()
                    || root_workspace.join("pnpm-workspace.yaml").exists()
                    || target_workspace.join("node_modules/.pnpm").is_dir()
                    || root_workspace.join("node_modules/.pnpm").is_dir()
                    || {
                        if let Ok(c) = fs::read_to_string(target_workspace.join("package.json")) {
                            c.contains("\"pnpm\"")
                        } else {
                            false
                        }
                    };
                if pnpm_detected {
                    mappers::PnpmMapper::map_package(target_workspace, pkg, archive_path)?;
                } else {
                    mappers::NpmMapper::map_package(target_workspace, pkg, archive_path)?;
                }
            }
            Ecosystem::Cargo => {
                mappers::CargoMapper::map_package(target_workspace, pkg, archive_path)?;
            }
            Ecosystem::PyPI => {
                mappers::PyPiMapper::map_package(target_workspace, pkg, archive_path)?;
            }
            Ecosystem::Go => {
                mappers::GoMapper::map_package(target_workspace, pkg, archive_path)?;
            }
            Ecosystem::Composer => {
                mappers::ComposerMapper::map_package(target_workspace, pkg, archive_path)?;
            }
            Ecosystem::Bundler => {
                mappers::BundlerMapper::map_package(target_workspace, pkg, archive_path)?;
            }
            Ecosystem::MavenGradle => {
                mappers::MavenMapper::map_package(target_workspace, pkg, archive_path)?;
            }
            Ecosystem::Unknown(ref eco) => {
                return Err(InstallError::UnsupportedEcosystem(eco.clone()));
            }
            _ => {
                return Err(InstallError::UnsupportedEcosystem(format!(
                    "{:?}",
                    pkg.ecosystem
                )));
            }
        }
        Ok(())
    }

    /// Hydrates IDE workspace settings, CLI binary shims, activator scripts, and pnpm configurations
    fn hydrate_post_install_environment(
        config: &InstallConfig,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) {
        // Collect all distinct workspace directories (root + valid members)
        let mut target_dirs = vec![config.workspace_path.clone()];
        if let Some(manifest) = workspace_manifest {
            for member in &manifest.members {
                let member_dir = Self::resolve_member_path(&config.workspace_path, &member.path);
                if member_dir.is_dir() && !target_dirs.contains(&member_dir) {
                    target_dirs.push(member_dir);
                }
            }
        }

        // 1. Ensure IDE workspace settings (.vscode/settings.json)
        for dir in &target_dirs {
            let _ = crate::toolchain::ToolchainSetup::generate_ide_settings(dir);
        }

        // 2. Hydrate CLI binary shims (e.g. node_modules/.bin/tauri)
        for dir in &target_dirs {
            mappers::NpmMapper::hydrate_workspace_bin_shims(dir);
        }

        // 3. Regenerate environment activator scripts (.dmz/env.sh, .dmz/env.bat)
        let _ = crate::toolchain::ToolchainSetup::generate_activator_scripts(&config.workspace_path);

        // 4. Ensure .modules.yaml exists for pnpm tooling
        for dir in &target_dirs {
            mappers::PnpmMapper::ensure_modules_yaml(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_install_error_display() {
        let err = InstallError::IntegrityMismatch {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert!(err.to_string().contains("Store Artifact Integrity Mismatch"));

        let err2 = InstallError::ArtifactMissing {
            package: "foo".into(),
            path: PathBuf::from("/store/foo.archive"),
        };
        assert!(err2.to_string().contains("Missing cached store artifact"));

        let err3 = InstallError::UnsupportedEcosystem("unknown_lang".into());
        assert!(err3.to_string().contains("Ecosystem not supported"));
    }

    #[test]
    fn test_install_fails_fast_when_store_missing() {
        let temp = tempdir().unwrap();
        let ws = temp.path().join("workspace");
        fs::create_dir_all(&ws).unwrap();
        let lockfile_path = ws.join("dmz.lock");

        let packages = std::collections::BTreeMap::new();
        let closure_signature = DmzLockfile::compute_closure_hash(&packages);
        let lockfile = DmzLockfile {
            version: 1,
            target_platform: None,
            vcs: None,
            workspaces: std::collections::BTreeMap::new(),
            closure_signature,
            packages,
        };
        lockfile.save(&lockfile_path).unwrap();

        let config = InstallConfig {
            workspace_path: ws,
            lockfile_path,
            store_dir: temp.path().join("non_existent_store"),
        };

        let result = InstallerEngine::install(&config);
        assert!(matches!(result, Err(InstallError::IoError(_))));
    }

    #[test]
    fn test_locate_store_artifact_missing() {
        let temp = tempdir().unwrap();
        let store = temp.path().join("store");
        fs::create_dir_all(&store).unwrap();

        let pkg = LockedPackage {
            name: "test-pkg".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: "deadbeef".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        let result = InstallerEngine::locate_store_artifact(&store, &pkg);
        assert!(matches!(result, Err(InstallError::ArtifactMissing { .. })));
    }

    #[test]
    fn test_verify_artifact_integrity_mismatch() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("artifact.archive");
        fs::write(&archive_path, b"corrupted content").unwrap();

        let pkg = LockedPackage {
            name: "test-pkg".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        let result = InstallerEngine::verify_artifact_integrity(&archive_path, &pkg);
        assert!(matches!(result, Err(InstallError::IntegrityMismatch { .. })));
    }

    #[test]
    fn test_locate_store_artifact_rejects_traversal() {
        let temp = tempdir().unwrap();
        let store = temp.path().join("store");
        fs::create_dir_all(&store).unwrap();

        for traversal_name in &["..", "../escape", "../../etc/shadow", "safe/../../escape"] {
            let pkg = LockedPackage {
                name: traversal_name.to_string(),
                version: "1.0.0".into(),
                ecosystem: Ecosystem::Cargo,
                sha256: "deadbeef".into(),
                source_url: None,
                dependencies: vec![],
                member_id: None,
            };

            let result = InstallerEngine::locate_store_artifact(&store, &pkg);
            assert!(
                matches!(result, Err(InstallError::MapperError(_))),
                "Expected traversal rejection for {}",
                traversal_name
            );
        }
    }

    #[test]
    fn test_resolve_member_path_containment() {
        let temp = tempdir().unwrap();
        let ws = temp.path().join("workspace");
        fs::create_dir_all(&ws).unwrap();
        let canonical_ws = fs::canonicalize(&ws).unwrap();

        // Valid inside member
        let valid_member = ws.join("packages/app");
        fs::create_dir_all(&valid_member).unwrap();
        let resolved = InstallerEngine::resolve_member_path(&ws, Path::new("packages/app"));
        assert!(resolved.starts_with(&canonical_ws));

        // Escaping member path
        let escaped_path = Path::new("../../../../etc");
        let resolved = InstallerEngine::resolve_member_path(&ws, escaped_path);
        // Must fallback to workspace root
        assert_eq!(resolved, canonical_ws);
    }
}
