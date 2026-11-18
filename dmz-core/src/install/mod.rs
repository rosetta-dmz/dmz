pub mod archive;
pub mod fast_clone;
pub mod mappers;
pub mod rust_vendor;

use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::Ecosystem;
use crate::workspace::WorkspaceProjectConfig;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct InstallConfig {
    pub workspace_path: PathBuf,
    pub lockfile_path: PathBuf,
    pub store_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Install IO Error: {0}")]
    IoError(String),
    #[error("Install Lockfile Error: {0}")]
    LockfileError(String),
    #[error(
        "Store Artifact Integrity Mismatch! Expected sha256:{expected}, computed sha256:{actual}"
    )]
    IntegrityMismatch { expected: String, actual: String },
    #[error("Missing cached store artifact for package '{package}' at path {path:?}")]
    ArtifactMissing { package: String, path: PathBuf },
    #[error("Ecosystem not supported for offline install: {0}")]
    UnsupportedEcosystem(String),
    #[error("Ecosystem Mapper Error: {0}")]
    MapperError(String),
}

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

        for pkg in lockfile.packages.values() {
            if pkg.is_local_path() {
                info!(name = %pkg.name, "Skipping install for local path dependency");
                continue;
            }

            // Resolve target workspace(s) (root, member directory, or nested frontend root)
            let target_workspaces = Self::resolve_target_workspaces(
                &config.workspace_path,
                pkg,
                workspace_manifest.as_ref(),
            );

            // Locate and validate the store artifact
            let archive_path = Self::locate_store_artifact(&config.store_dir, pkg)?;

            // Cryptographically verify supply chain integrity
            Self::verify_artifact_integrity(&archive_path, pkg)?;

            // Ecosystem-specific deterministic linkage
            for target_workspace in &target_workspaces {
                let excluded = Self::excluded_sibling_dirs_for(
                    &config.workspace_path,
                    target_workspace,
                    workspace_manifest.as_ref(),
                );
                Self::dispatch_package_mapper(
                    target_workspace,
                    &config.workspace_path,
                    pkg,
                    &archive_path,
                    &excluded,
                )?;
            }

            installed_count += 1;
            info!(name = %pkg.name, version = %pkg.version, "Successfully installed package offline");
        }

        // Post-install Cargo vendor completeness verification across every discovered Cargo target
        Self::verify_cargo_vendor_completeness(
            &config.workspace_path,
            &lockfile,
            workspace_manifest.as_ref(),
        )?;

        // Post-install environment hydration (IDE settings, shims, activators, modules.yaml, pnpm links)
        Self::hydrate_post_install_environment(config, &lockfile, workspace_manifest.as_ref())?;

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
            WorkspaceProjectConfig::discover_workspace_manifest_scoped(workspace_path)
                .map(|(p, _)| p)
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

    fn expand_ecosystem_target_dirs(dir: &Path, ecosystem: &Ecosystem) -> Vec<PathBuf> {
        if *ecosystem == Ecosystem::Npm {
            let roots = crate::workspace::discover_node_project_roots(dir);
            if !roots.is_empty() {
                return roots;
            }
        }
        vec![dir.to_path_buf()]
    }

    fn member_matches_ecosystem(m_dir: &Path, ecosystem: &Ecosystem) -> bool {
        match ecosystem {
            Ecosystem::Npm => !crate::workspace::discover_node_project_roots(m_dir).is_empty(),
            Ecosystem::Cargo => !crate::workspace::discover_tauri_cargo_dirs(m_dir).is_empty(),
            Ecosystem::PyPI => {
                m_dir.join("requirements.txt").is_file() || m_dir.join("pyproject.toml").is_file()
            }
            Ecosystem::Go => m_dir.join("go.mod").is_file(),
            Ecosystem::Composer => m_dir.join("composer.json").is_file(),
            Ecosystem::Bundler => {
                m_dir.join("Gemfile").is_file() || m_dir.join("Gemfile.lock").is_file()
            }
            Ecosystem::MavenGradle => {
                m_dir.join("pom.xml").is_file()
                    || m_dir.join("build.gradle").is_file()
                    || m_dir.join("build.gradle.kts").is_file()
            }
            _ => false,
        }
    }

    /// Resolves all target directories for a package (member root, nested Node root, or all matching members)
    fn resolve_target_workspaces(
        workspace_path: &Path,
        pkg: &LockedPackage,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) -> Vec<PathBuf> {
        let Some(manifest) = workspace_manifest else {
            return Self::expand_ecosystem_target_dirs(workspace_path, &pkg.ecosystem);
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
            let other_member_owns_eco = manifest.members.iter().any(|m| {
                m.id != member.id
                    && Self::member_matches_ecosystem(
                        &Self::resolve_member_path(workspace_path, &m.path),
                        &pkg.ecosystem,
                    )
            });
            if member_dir.is_dir()
                && (Self::member_matches_ecosystem(&member_dir, &pkg.ecosystem)
                    || !other_member_owns_eco)
            {
                return Self::expand_ecosystem_target_dirs(&member_dir, &pkg.ecosystem);
            }
            if !other_member_owns_eco {
                return Self::expand_ecosystem_target_dirs(workspace_path, &pkg.ecosystem);
            }
        }

        // 2. Intelligent Ecosystem Fallback:
        // Route package to all workspace members owning a matching ecosystem manifest
        let mut targets = Vec::new();
        for member in &manifest.members {
            let m_dir = Self::resolve_member_path(workspace_path, &member.path);
            if Self::member_matches_ecosystem(&m_dir, &pkg.ecosystem) {
                for dir in Self::expand_ecosystem_target_dirs(&m_dir, &pkg.ecosystem) {
                    if !targets.contains(&dir) {
                        targets.push(dir);
                    }
                }
            }
        }

        if targets.is_empty() {
            Self::expand_ecosystem_target_dirs(workspace_path, &pkg.ecosystem)
        } else {
            targets
        }
    }

    /// Resolves primary target workspace directory for a package (routing to appropriate member or root)
    #[cfg_attr(not(test), allow(dead_code))]
    fn resolve_target_workspace(
        workspace_path: &Path,
        pkg: &LockedPackage,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) -> PathBuf {
        Self::resolve_target_workspaces(workspace_path, pkg, workspace_manifest)
            .into_iter()
            .next()
            .unwrap_or_else(|| workspace_path.to_path_buf())
    }

    /// Resolves and locates the package archive file within the store directory
    fn locate_store_artifact(
        store_dir: &Path,
        pkg: &LockedPackage,
    ) -> Result<PathBuf, InstallError> {
        locate_locked_store_artifact(store_dir, pkg)
    }

    /// Computes streaming SHA-256 and validates cryptographic supply chain integrity
    fn verify_artifact_integrity(
        archive_path: &Path,
        pkg: &LockedPackage,
    ) -> Result<(), InstallError> {
        let computed_hash = crate::archive::exporter::hashing::calculate_file_sha256(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to read artifact: {}", e)))?;

        if computed_hash != pkg.sha256 {
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

        debug!(
            package = %pkg.name,
            checksum = %computed_hash,
            "Cryptographically verified store artifact against lockfile"
        );

        Ok(())
    }

    fn excluded_sibling_dirs_for(
        workspace_path: &Path,
        target_workspace: &Path,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) -> Vec<PathBuf> {
        let Some(manifest) = workspace_manifest else {
            return Vec::new();
        };
        let target_canon =
            fs::canonicalize(target_workspace).unwrap_or_else(|_| target_workspace.to_path_buf());
        manifest
            .members
            .iter()
            .filter_map(|m| {
                let resolved = Self::resolve_member_path(workspace_path, &m.path);
                let canon = fs::canonicalize(&resolved).unwrap_or(resolved);
                (canon != target_canon).then_some(canon)
            })
            .collect()
    }

    /// Dispatches package layout mapping to the corresponding ecosystem mapper
    fn dispatch_package_mapper(
        target_workspace: &Path,
        root_workspace: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
        excluded_dirs: &[PathBuf],
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
                mappers::CargoMapper::map_package_excluding(
                    target_workspace,
                    pkg,
                    archive_path,
                    excluded_dirs,
                )?;
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
        }
        Ok(())
    }

    fn resolve_effective_cargo_vendor_dir(cargo_dir: &Path, stop_root: &Path) -> Option<PathBuf> {
        let stop_canon = fs::canonicalize(stop_root).unwrap_or_else(|_| stop_root.to_path_buf());
        let mut cursor = Some(cargo_dir);
        while let Some(dir) = cursor {
            let cfg_path = dir.join(".cargo/config.toml");
            if cfg_path.is_file() {
                if let Some(raw_dir) =
                    mappers::CargoMapper::read_vendored_source_directory(&cfg_path)
                {
                    let raw = Path::new(&raw_dir);
                    let resolved = if raw.is_absolute() {
                        raw.to_path_buf()
                    } else {
                        dir.join(raw)
                    };
                    return Some(resolved);
                }
            }
            let canon_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            if canon_dir == stop_canon {
                break;
            }
            cursor = dir.parent();
        }
        None
    }

    fn vendor_dir_contains_crate_named(vendor_dir: &Path, crate_name: &str) -> bool {
        let prefix = format!("{}-", crate_name);
        let Ok(entries) = fs::read_dir(vendor_dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let p = entry.path();
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(&prefix))
                && p.join("Cargo.toml").is_file()
        })
    }

    /// Verifies that every non-local Cargo package in `dmz.lock`, every direct Cargo.toml dependency,
    /// and (for target-unfiltered closures) every Cargo.lock crates.io package is materialized inside
    /// the resolved `[source.vendored-sources].directory` used by each discovered `Cargo.toml`.
    fn verify_cargo_vendor_completeness(
        workspace_path: &Path,
        lockfile: &DmzLockfile,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) -> Result<(), InstallError> {
        let cargo_pkgs: Vec<&LockedPackage> = lockfile
            .packages
            .values()
            .filter(|p| p.ecosystem == Ecosystem::Cargo && !p.is_local_path())
            .collect();

        for pkg in &cargo_pkgs {
            for target_ws in
                Self::resolve_target_workspaces(workspace_path, pkg, workspace_manifest)
            {
                let excluded =
                    Self::excluded_sibling_dirs_for(workspace_path, &target_ws, workspace_manifest);
                let cargo_dirs =
                    crate::workspace::discover_tauri_cargo_dirs_excluding(&target_ws, &excluded);
                for cargo_dir in cargo_dirs {
                    let Some(vendor_dir) =
                        Self::resolve_effective_cargo_vendor_dir(&cargo_dir, workspace_path)
                    else {
                        return Err(InstallError::MapperError(format!(
                            "Missing `.cargo/config.toml` vendored-sources configuration for Cargo project at {:?}",
                            cargo_dir
                        )));
                    };
                    let crate_manifest = vendor_dir
                        .join(format!("{}-{}", pkg.name, pkg.version))
                        .join("Cargo.toml");
                    if !crate_manifest.is_file() {
                        return Err(InstallError::MapperError(format!(
                            "Vendored Cargo crate '{}@{}' required by {:?} was not materialized in {:?}",
                            pkg.name, pkg.version, cargo_dir, vendor_dir
                        )));
                    }
                }
            }
        }

        let include_dev = lockfile.include_dev.unwrap_or(true);
        let include_target_tables = lockfile.target_platform.is_none();
        let member_targets: Vec<PathBuf> = if let Some(manifest) = workspace_manifest {
            manifest
                .members
                .iter()
                .map(|m| Self::resolve_member_path(workspace_path, &m.path))
                .filter(|d| d.is_dir())
                .collect()
        } else {
            vec![workspace_path.to_path_buf()]
        };

        for target_ws in member_targets {
            let excluded =
                Self::excluded_sibling_dirs_for(workspace_path, &target_ws, workspace_manifest);
            for cargo_dir in
                crate::workspace::discover_tauri_cargo_dirs_excluding(&target_ws, &excluded)
            {
                let direct_crates = crate::resolver::manifest::cargo::CargoManifestScanner::collect_direct_crate_names_with_targets(
                    &cargo_dir,
                    include_dev,
                    include_target_tables,
                );
                let lock_pkgs = if include_target_tables && include_dev {
                    crate::resolver::manifest::cargo::CargoManifestScanner::collect_lockfile_registry_packages(&cargo_dir)
                } else {
                    Vec::new()
                };
                if direct_crates.is_empty() && lock_pkgs.is_empty() {
                    continue;
                }
                let Some(vendor_dir) =
                    Self::resolve_effective_cargo_vendor_dir(&cargo_dir, workspace_path)
                else {
                    return Err(InstallError::MapperError(format!(
                        "Missing `.cargo/config.toml` vendored-sources configuration for Cargo project at {:?}",
                        cargo_dir
                    )));
                };
                for crate_name in direct_crates {
                    if !Self::vendor_dir_contains_crate_named(&vendor_dir, &crate_name) {
                        return Err(InstallError::MapperError(format!(
                            "Direct Cargo.toml dependency '{}' required by {:?} is missing from vendored directory {:?} (re-run `dmz bundle` or `Scan & Resolve`)",
                            crate_name, cargo_dir, vendor_dir
                        )));
                    }
                }
                for entry in lock_pkgs {
                    let crate_manifest = vendor_dir
                        .join(format!("{}-{}", entry.name, entry.version))
                        .join("Cargo.toml");
                    if !crate_manifest.is_file() {
                        return Err(InstallError::MapperError(format!(
                            "Cargo.lock package '{}@{}' required by {:?} was not materialized in {:?} (re-run `dmz bundle` or `Scan & Resolve`)",
                            entry.name, entry.version, cargo_dir, vendor_dir
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Hydrates IDE workspace settings, CLI binary shims, activator scripts, and pnpm configurations
    fn hydrate_post_install_environment(
        config: &InstallConfig,
        lockfile: &DmzLockfile,
        workspace_manifest: Option<&WorkspaceProjectConfig>,
    ) -> Result<(), InstallError> {
        // Collect all distinct workspace directories (root + valid members + nested Node roots)
        let mut target_dirs = vec![config.workspace_path.clone()];
        if let Some(manifest) = workspace_manifest {
            for member in &manifest.members {
                let member_dir = Self::resolve_member_path(&config.workspace_path, &member.path);
                if member_dir.is_dir() && !target_dirs.contains(&member_dir) {
                    target_dirs.push(member_dir);
                }
            }
        }

        let mut node_target_dirs = target_dirs.clone();
        for dir in &target_dirs {
            for node_root in crate::workspace::discover_node_project_roots(dir) {
                if node_root.is_dir() && !node_target_dirs.contains(&node_root) {
                    node_target_dirs.push(node_root);
                }
            }
        }

        // 1. Ensure IDE workspace settings (.vscode/settings.json)
        for dir in &target_dirs {
            if let Err(e) = crate::toolchain::ToolchainSetup::generate_ide_settings(dir) {
                warn!(dir = ?dir, error = %e, "Failed to update IDE settings during post-install");
            }
        }

        let include_dev = lockfile.include_dev.unwrap_or(true);

        // 2. Hydrate CLI binary shims (e.g. node_modules/.bin/tauri)
        for dir in &node_target_dirs {
            mappers::NpmMapper::hydrate_workspace_bin_shims_with_options(dir, include_dev);
        }

        // 3. Regenerate environment activator scripts (.dmz/env.sh, .dmz/env.bat)
        if let Err(e) =
            crate::toolchain::ToolchainSetup::generate_activator_scripts(&config.workspace_path)
        {
            warn!(error = %e, "Failed to generate environment activator scripts during post-install");
        }

        // 4. Wire transitive pnpm dependency symlinks inside `.pnpm/` virtual store,
        //    hydrate workspace sub-package `node_modules` + `.bin/*` shims, and ensure .modules.yaml
        for dir in &node_target_dirs {
            mappers::PnpmMapper::wire_pnpm_dependency_links(dir, lockfile)?;
            mappers::NpmMapper::hydrate_npm_workspace_members(dir, include_dev);
            mappers::PnpmMapper::ensure_modules_yaml_with_options(dir, include_dev);
        }

        Ok(())
    }
}

fn collect_matching_store_archives(
    dir: &Path,
    raw_name: &str,
    version: &str,
    is_pypi: bool,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    let prefix = format!("{}-", raw_name);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "archive") && path.is_file() {
            if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                let matched = (fname.starts_with(&prefix) && fname.contains(version))
                    || (is_pypi
                        && crate::resolver::fetch::cache::pypi_archive_matches_name_and_version(
                            fname, raw_name, version,
                        ));
                if matched {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// Resolves and validates a locked package's immutable `.archive` artifact inside `store_dir`.
///
/// # Errors
/// Returns `InstallError` if the package name attempts traversal, escapes `store_dir`,
/// or no matching artifact exists on disk.
pub fn locate_locked_store_artifact(
    store_dir: &Path,
    pkg: &LockedPackage,
) -> Result<PathBuf, InstallError> {
    let safe_name = pkg.name.replace(['/', '\\'], "__");

    if safe_name.is_empty() || safe_name == "." || safe_name == ".." || safe_name.contains("..") {
        return Err(InstallError::MapperError(format!(
            "Invalid package name contains path traversal sequence: {}",
            pkg.name
        )));
    }

    let is_pypi = pkg.ecosystem == crate::resolver::manifest::Ecosystem::PyPI;
    let mut store_dir_names = Vec::new();
    if is_pypi {
        let lower_name = safe_name.to_ascii_lowercase();
        if !lower_name.is_empty() {
            store_dir_names.push(lower_name);
        }
        let norm_name = crate::resolver::manifest::ManifestScanner::normalize_pypi_name(&pkg.name);
        if !norm_name.is_empty() && !store_dir_names.contains(&norm_name) {
            store_dir_names.push(norm_name);
        }
    }
    if !store_dir_names.contains(&safe_name) {
        store_dir_names.push(safe_name.clone());
    }

    let primary_pkg_dir = store_dir.join(&safe_name);
    let exact_name = format!("{}-{}.archive", safe_name, pkg.version);
    let exact_path = primary_pkg_dir.join(&exact_name);

    let mut candidates = Vec::new();
    for dir_name in &store_dir_names {
        let pkg_store_dir = store_dir.join(dir_name);
        let v2_dir = pkg_store_dir.join("v2");
        for path in collect_matching_store_archives(&v2_dir, &safe_name, &pkg.version, is_pypi) {
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }
    if exact_path.is_file() {
        candidates.push(exact_path.clone());
    }
    for dir_name in &store_dir_names {
        let pkg_store_dir = store_dir.join(dir_name);
        for path in
            collect_matching_store_archives(&pkg_store_dir, &safe_name, &pkg.version, is_pypi)
        {
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    }

    let has_valid_sha256 =
        pkg.sha256.len() == 64 && pkg.sha256.bytes().all(|b| b.is_ascii_hexdigit());

    let archive_path = if has_valid_sha256 {
        candidates
            .iter()
            .find(|p| crate::resolver::fetch::cache::verify_file_sha256(p, &pkg.sha256))
            .cloned()
            // Fallback lets downstream `verify_artifact_integrity` surface `IntegrityMismatch`
            // if a single cached file was tampered on disk instead of masking it as `ArtifactMissing`.
            .unwrap_or_else(|| candidates.into_iter().next().unwrap_or(exact_path))
    } else if exact_path.exists() {
        exact_path
    } else {
        candidates.into_iter().next().unwrap_or(exact_path)
    };

    let canonical_store = store_dir
        .canonicalize()
        .unwrap_or_else(|_| store_dir.to_path_buf());
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

#[cfg(test)]
mod tests;
