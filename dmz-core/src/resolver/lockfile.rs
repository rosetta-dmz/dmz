use crate::resolver::manifest::Ecosystem;
use crate::vcs::VcsProvenance;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

pub const MAX_LOCKFILE_SIZE: u64 = 16 * 1024 * 1024;

pub trait LockfileReader {
    fn load_and_verify(path: &Path) -> Result<DmzLockfile, LockError>;
}

pub trait LockfileWriter {
    fn save(&self, path: &Path) -> Result<(), LockError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub ecosystem: Ecosystem,
    pub sha256: String,
    pub source_url: Option<String>,
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
}

impl LockedPackage {
    /// Returns `true` when this package references a local filesystem path rather than a registry artifact.
    pub fn is_local_path(&self) -> bool {
        self.version.starts_with("path:")
            || self
                .source_url
                .as_deref()
                .is_some_and(|u| u.starts_with("file:"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMetadata {
    pub id: String,
    pub path: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmzLockfile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_dev: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsProvenance>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workspaces: BTreeMap<String, WorkspaceMetadata>,
    pub closure_signature: String,
    pub packages: BTreeMap<String, LockedPackage>,
}

impl Default for DmzLockfile {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("Lockfile IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lockfile Serialization Error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Cryptographic Lock Mismatch! Expected sha256:{expected}, computed sha256:{actual}")]
    IntegrityMismatch { expected: String, actual: String },
    #[error(
        "Lockfile exceeds maximum allowable size of {max_bytes} bytes (found {actual_bytes} bytes)"
    )]
    LockfileTooLarge { max_bytes: u64, actual_bytes: u64 },
    #[error("Lockfile IO Error: {0}")]
    IoError(String),
    #[error("Lockfile Serialization Error: {0}")]
    SerializationError(String),
}

impl LockfileReader for DmzLockfile {
    fn load_and_verify(path: &Path) -> Result<Self, LockError> {
        Self::load_and_verify(path)
    }
}

impl LockfileWriter for DmzLockfile {
    fn save(&self, path: &Path) -> Result<(), LockError> {
        self.save(path)
    }
}

impl DmzLockfile {
    pub fn new(packages: Vec<LockedPackage>) -> Self {
        Self::with_platform(packages, None)
    }

    pub fn with_platform(packages: Vec<LockedPackage>, target_platform: Option<String>) -> Self {
        let mut package_map = BTreeMap::new();
        for pkg in packages {
            Self::insert_package(&mut package_map, pkg);
        }

        let closure_signature = Self::compute_closure_hash(&package_map);

        Self {
            version: 1,
            target_platform,
            include_dev: None,
            inputs_digest: None,
            vcs: None,
            workspaces: BTreeMap::new(),
            closure_signature,
            packages: package_map,
        }
    }

    /// Computes a deterministic SHA-256 content digest across all workspace and member
    /// manifests/lockfiles (`Cargo.toml`, `Cargo.lock`, `package.json`, `pnpm-lock.yaml`,
    /// `requirements.txt`, `pyproject.toml`, `go.mod`, etc.) plus resolution options.
    pub fn compute_workspace_inputs_digest(
        workspace_root: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"dmz-inputs-v1\0");
        hasher.update(target_platform.unwrap_or("").as_bytes());
        hasher.update(b"\0");
        hasher.update([u8::from(include_dev)]);
        hasher.update(b"\0");

        let ws_manifest = workspace_root.join("dmz-workspace.toml");
        if ws_manifest.is_file() {
            if let Ok(bytes) = fs::read(&ws_manifest) {
                hasher.update(b"dmz-workspace.toml\0");
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
        }

        let members = Self::resolve_member_dirs_with_exclusions(workspace_root, multi_workspace);
        for (member_id, member_dir, excluded) in members {
            hasher.update(b"member\0");
            hasher.update(member_id.as_bytes());
            hasher.update(b"\0");
            if !member_dir.is_dir() {
                hasher.update(b"missing\0");
                continue;
            }
            let manifest_files = Self::collect_member_manifest_files(&member_dir, &excluded);
            for (rel_key, abs_path) in manifest_files {
                if let Ok(bytes) = fs::read(&abs_path) {
                    hasher.update(rel_key.as_bytes());
                    hasher.update(b"\0");
                    hasher.update((bytes.len() as u64).to_le_bytes());
                    hasher.update(&bytes);
                }
            }
        }

        crate::archive::exporter::hashing::format_sha256_hex(&hasher.finalize())
    }

    fn resolve_member_dirs_with_exclusions(
        workspace_root: &Path,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> Vec<(String, std::path::PathBuf, Vec<std::path::PathBuf>)> {
        let Some(mws) = multi_workspace else {
            return vec![("root".to_string(), workspace_root.to_path_buf(), Vec::new())];
        };
        let resolved: Vec<(String, std::path::PathBuf)> = mws
            .members
            .iter()
            .map(|member| {
                let dir = if member.path.is_absolute() {
                    member.path.clone()
                } else {
                    workspace_root.join(&member.path)
                };
                let canon = fs::canonicalize(&dir).unwrap_or(dir);
                (member.id.clone(), canon)
            })
            .collect();

        let mut out: Vec<(String, std::path::PathBuf, Vec<std::path::PathBuf>)> = resolved
            .iter()
            .enumerate()
            .map(|(idx, (id, dir))| {
                let excluded = resolved
                    .iter()
                    .enumerate()
                    .filter(|(j, (_, other_dir))| *j != idx && other_dir != dir)
                    .map(|(_, (_, other_dir))| other_dir.clone())
                    .collect();
                (id.clone(), dir.clone(), excluded)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn collect_member_manifest_files(
        member_dir: &Path,
        excluded: &[std::path::PathBuf],
    ) -> BTreeMap<String, std::path::PathBuf> {
        let mut files = BTreeMap::new();
        let mut record_if_file = |path: std::path::PathBuf| {
            if path.is_file() {
                let rel = path
                    .strip_prefix(member_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(rel, path);
            }
        };

        for node_root in
            crate::workspace::discover_node_project_roots_excluding(member_dir, excluded)
        {
            for fname in [
                "package.json",
                "pnpm-workspace.yaml",
                "pnpm-lock.yaml",
                "package-lock.json",
                "yarn.lock",
            ] {
                record_if_file(node_root.join(fname));
            }
        }

        for cargo_dir in crate::workspace::discover_tauri_cargo_dirs_excluding(member_dir, excluded)
        {
            record_if_file(cargo_dir.join("Cargo.toml"));
            record_if_file(cargo_dir.join("Cargo.lock"));
            for ws_member in
                crate::workspace::discovery::discover_cargo_workspace_member_dirs(&cargo_dir)
            {
                record_if_file(ws_member.join("Cargo.toml"));
                record_if_file(ws_member.join("Cargo.lock"));
            }
        }

        for fname in [
            "requirements.txt",
            "pyproject.toml",
            "setup.py",
            "Pipfile",
            "uv.lock",
            "poetry.lock",
            "go.mod",
            "go.sum",
            "composer.json",
            "composer.lock",
            "Gemfile",
            "Gemfile.lock",
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
        ] {
            record_if_file(member_dir.join(fname));
        }

        files
    }

    /// Returns `true` when this lockfile was resolved with incompatible options or is missing
    /// declared workspace members, signaling that `bundle` / `Bundle (Fetch)` should re-resolve.
    pub fn is_stale_for_bundle(
        &self,
        target_platform: Option<&str>,
        include_dev: bool,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> bool {
        if self.target_platform.as_deref() != target_platform {
            return true;
        }
        // Only treat missing `include_dev` metadata as stale when `include_dev` is requested,
        // so legacy lockfiles without devDeps are refreshed once devDeps are enabled.
        if self.include_dev != Some(include_dev) && (self.include_dev.is_some() || include_dev) {
            return true;
        }
        if let Some(mws) = multi_workspace {
            for member in &mws.members {
                if !self.workspaces.contains_key(&member.id) {
                    return true;
                }
            }
        }
        false
    }

    /// Checks whether a lockfile on disk is missing, unreadable/corrupt, or stale for the requested
    /// bundle configuration.
    pub fn file_needs_refresh(
        lockfile_path: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> bool {
        if !lockfile_path.exists() {
            return true;
        }
        match Self::load_and_verify(lockfile_path) {
            Ok(existing) => {
                existing.is_stale_for_bundle(target_platform, include_dev, multi_workspace)
            }
            Err(_) => true,
        }
    }

    /// Returns `true` when this lockfile is stale for the requested options, manifest digest,
    /// or uncovered Cargo/Npm/PyPI direct dependencies in `workspace_root` / `multi_workspace`.
    pub fn is_stale_for_workspace(
        &self,
        workspace_root: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> bool {
        if self.is_stale_for_bundle(target_platform, include_dev, multi_workspace) {
            return true;
        }

        if let Some(ref locked_digest) = self.inputs_digest {
            let current_digest = Self::compute_workspace_inputs_digest(
                workspace_root,
                target_platform,
                include_dev,
                multi_workspace,
            );
            return locked_digest != &current_digest;
        }

        let targets: Vec<(Option<String>, std::path::PathBuf, Vec<std::path::PathBuf>)> =
            if multi_workspace.is_some() {
                Self::resolve_member_dirs_with_exclusions(workspace_root, multi_workspace)
                    .into_iter()
                    .map(|(id, dir, ex)| (Some(id), dir, ex))
                    .collect()
            } else {
                vec![(None, workspace_root.to_path_buf(), Vec::new())]
            };

        for (member_id, dir, excluded) in targets {
            if self.member_has_uncovered_manifests(
                &dir,
                member_id.as_deref(),
                target_platform,
                include_dev,
                &excluded,
            ) {
                return true;
            }
        }

        false
    }

    pub(crate) fn find_missing_cargo_requirement(
        &self,
        workspace_root: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> Option<String> {
        let targets: Vec<(Option<String>, std::path::PathBuf, Vec<std::path::PathBuf>)> =
            if multi_workspace.is_some() {
                Self::resolve_member_dirs_with_exclusions(workspace_root, multi_workspace)
                    .into_iter()
                    .map(|(id, dir, ex)| (Some(id), dir, ex))
                    .collect()
            } else {
                vec![(None, workspace_root.to_path_buf(), Vec::new())]
            };

        let include_target_tables = target_platform.is_none();
        for (member_id, dir, excluded) in targets {
            if !dir.is_dir() {
                continue;
            }
            for cargo_dir in crate::workspace::discover_tauri_cargo_dirs_excluding(&dir, &excluded)
            {
                let required_crates = crate::resolver::manifest::cargo::CargoManifestScanner::collect_direct_crate_names_with_targets(
                    &cargo_dir,
                    include_dev,
                    include_target_tables,
                );
                for crate_name in required_crates {
                    let covered = self.packages.values().any(|p| {
                        p.ecosystem == Ecosystem::Cargo
                            && p.name == crate_name
                            && Self::package_matches_member(p, member_id.as_deref())
                    });
                    if !covered {
                        return Some(format!(
                            "Cargo crate '{}' required by {:?} is missing from dmz.lock",
                            crate_name, cargo_dir
                        ));
                    }
                }
                if include_dev && target_platform.is_none() {
                    for entry in crate::resolver::manifest::cargo::CargoManifestScanner::collect_lockfile_registry_packages(&cargo_dir) {
                        let covered = self.packages.values().any(|p| {
                            p.ecosystem == Ecosystem::Cargo
                                && p.name == entry.name
                                && p.version == entry.version
                                && Self::package_matches_member(p, member_id.as_deref())
                        });
                        if !covered {
                            return Some(format!(
                                "Cargo.lock package '{}@{}' required by {:?} is missing from dmz.lock",
                                entry.name, entry.version, cargo_dir
                            ));
                        }
                    }
                }
            }
        }
        None
    }

    fn package_matches_member(pkg: &LockedPackage, member_id: Option<&str>) -> bool {
        match member_id {
            Some(mid) => pkg.member_id.as_deref() == Some(mid) || pkg.member_id.is_none(),
            None => true,
        }
    }

    fn member_has_uncovered_manifests(
        &self,
        dir: &Path,
        member_id: Option<&str>,
        target_platform: Option<&str>,
        include_dev: bool,
        excluded: &[std::path::PathBuf],
    ) -> bool {
        if !dir.is_dir() {
            return false;
        }

        // 1. Cargo coverage (checks every discovered Cargo.toml / Cargo.lock crate name)
        for cargo_dir in crate::workspace::discover_tauri_cargo_dirs_excluding(dir, excluded) {
            let required_crates =
                crate::resolver::manifest::cargo::CargoManifestScanner::collect_direct_crate_names(
                    &cargo_dir,
                    include_dev,
                );
            for crate_name in required_crates {
                let covered = self.packages.values().any(|p| {
                    p.ecosystem == Ecosystem::Cargo
                        && p.name == crate_name
                        && Self::package_matches_member(p, member_id)
                });
                if !covered {
                    return true;
                }
            }
            if include_dev && target_platform.is_none() {
                for lock_entry in crate::resolver::manifest::cargo::CargoManifestScanner::collect_lockfile_registry_packages(&cargo_dir) {
                    let covered = self.packages.values().any(|p| {
                        p.ecosystem == Ecosystem::Cargo
                            && p.name == lock_entry.name
                            && p.version == lock_entry.version
                            && Self::package_matches_member(p, member_id)
                    });
                    if !covered {
                        return true;
                    }
                }
            }
        }

        // 2. Npm coverage (legacy check plus direct dependency name check)
        let mut npm_manifests = Vec::new();
        let _ = crate::resolver::manifest::npm::NpmManifestScanner::scan_with_options_excluding(
            dir,
            target_platform,
            include_dev,
            excluded,
            &mut npm_manifests,
        );
        for dep in npm_manifests.iter().flat_map(|m| m.dependencies.iter()) {
            if dep.version_req.starts_with("path:")
                || dep.version_req.starts_with("file:")
                || dep.version_req.starts_with("workspace:")
                || dep.version_req.starts_with("link:")
            {
                continue;
            }
            let covered = self.packages.values().any(|p| {
                p.ecosystem == Ecosystem::Npm
                    && p.name == dep.name
                    && Self::package_matches_member(p, member_id)
            });
            if !covered {
                return true;
            }
        }

        // 3. Python coverage
        let mut py_manifests = Vec::new();
        let _ = crate::resolver::manifest::python::PythonManifestScanner::scan_with_options(
            dir,
            &mut py_manifests,
            target_platform,
            include_dev,
        );
        if py_manifests.iter().any(|m| !m.dependencies.is_empty())
            && !self.packages.values().any(|p| {
                p.ecosystem == Ecosystem::PyPI && Self::package_matches_member(p, member_id)
            })
        {
            return true;
        }

        false
    }

    /// Workspace-aware lockfile refresh check verifying both metadata options and uncovered manifests.
    pub fn file_needs_refresh_for_workspace(
        lockfile_path: &Path,
        workspace_root: &Path,
        target_platform: Option<&str>,
        include_dev: bool,
        multi_workspace: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) -> bool {
        if !lockfile_path.exists() {
            return true;
        }
        match Self::load_and_verify(lockfile_path) {
            Ok(existing) => existing.is_stale_for_workspace(
                workspace_root,
                target_platform,
                include_dev,
                multi_workspace,
            ),
            Err(_) => true,
        }
    }

    /// Inserts `pkg` into `packages` using the canonical `name:version` key
    /// (disambiguated when another workspace member or ecosystem holds the base key).
    pub fn insert_package(packages: &mut BTreeMap<String, LockedPackage>, pkg: LockedPackage) {
        let base_key = format!("{}:{}", pkg.name, pkg.version);
        let key = if let Some(existing) = packages.get(&base_key) {
            if existing.member_id != pkg.member_id {
                if let Some(ref mid) = pkg.member_id {
                    format!("{}@{}", mid, base_key)
                } else if existing.ecosystem != pkg.ecosystem {
                    format!("{:?}@{}", pkg.ecosystem, base_key)
                } else {
                    base_key
                }
            } else if existing.ecosystem != pkg.ecosystem {
                format!("{:?}@{}", pkg.ecosystem, base_key)
            } else {
                base_key
            }
        } else {
            base_key
        };
        packages.insert(key, pkg);
    }

    pub fn with_vcs(mut self, vcs: Option<VcsProvenance>) -> Self {
        self.vcs = vcs;
        self
    }

    pub fn with_workspaces(mut self, workspaces: BTreeMap<String, WorkspaceMetadata>) -> Self {
        self.workspaces = workspaces;
        self
    }

    pub fn add_workspace(&mut self, meta: WorkspaceMetadata) {
        self.workspaces.insert(meta.id.clone(), meta);
    }

    /// Computes a deterministic SHA-256 signature across the entire ordered package map
    pub fn compute_closure_hash(packages: &BTreeMap<String, LockedPackage>) -> String {
        let mut hasher = Sha256::new();

        for (key, pkg) in packages {
            hasher.update(key.as_bytes());
            hasher.update(pkg.name.as_bytes());
            hasher.update(pkg.version.as_bytes());
            hasher.update(pkg.sha256.as_bytes());
            if let Some(ref mid) = pkg.member_id {
                hasher.update(mid.as_bytes());
            }
        }

        crate::archive::exporter::hashing::format_sha256_hex(&hasher.finalize())
    }

    /// Saves the lockfile to `dmz.lock` atomically
    pub fn save(&self, path: &Path) -> Result<(), LockError> {
        let json_data = serde_json::to_string_pretty(self)?;
        crate::workspace::atomic_write_file(path, json_data)?;
        Ok(())
    }

    /// Loads and verifies cryptographic integrity of `dmz.lock`
    pub fn load_and_verify(path: &Path) -> Result<Self, LockError> {
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() {
            return Err(LockError::Io(std::io::Error::other(format!(
                "Path {:?} is a directory, expected lockfile",
                path
            ))));
        }
        if metadata.len() > MAX_LOCKFILE_SIZE {
            return Err(LockError::LockfileTooLarge {
                max_bytes: MAX_LOCKFILE_SIZE,
                actual_bytes: metadata.len(),
            });
        }

        let mut reader = file.take(MAX_LOCKFILE_SIZE + 1);
        let mut content = String::new();
        reader.read_to_string(&mut content)?;
        if content.len() as u64 > MAX_LOCKFILE_SIZE {
            return Err(LockError::LockfileTooLarge {
                max_bytes: MAX_LOCKFILE_SIZE,
                actual_bytes: content.len() as u64,
            });
        }

        let lockfile: Self = serde_json::from_str(&content)?;

        let computed_signature = Self::compute_closure_hash(&lockfile.packages);

        if computed_signature != lockfile.closure_signature {
            return Err(LockError::IntegrityMismatch {
                expected: lockfile.closure_signature,
                actual: computed_signature,
            });
        }

        Ok(lockfile)
    }

    /// Helper to compute SHA-256 digest of raw binary closure files
    pub fn hash_bytes(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        crate::archive::exporter::hashing::format_sha256_hex(&hasher.finalize())
    }
}

#[cfg(test)]
#[path = "lockfile_tests.rs"]
mod tests;
