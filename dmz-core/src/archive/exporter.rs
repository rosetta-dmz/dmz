use crate::ecosystem::EcosystemRegistry;
use crate::resolver::lockfile::DmzLockfile;
use crate::vcs::{GitProvider, VcsProvenance};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::{info, warn};
use zstd::stream::write::Encoder as ZstdEncoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    Unified,  // Workspace source code + dependency store archives
    CodeOnly, // Workspace source code only (no dependencies)
    DepsOnly, // Dependency store archives only (no source code)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaBaseInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_archive_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_closure_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirGapManifest {
    pub dmz_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_base: Option<DeltaBaseInfo>,
    #[serde(default)]
    pub has_git_history: bool,
    pub closure_signature: String,
    pub package_count: usize,
    pub created_at_utc: u64,
    pub archive_sha256: String,
}

#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub workspace_path: PathBuf,
    pub lockfile_path: PathBuf,
    pub store_dir: PathBuf,
    pub output_path: PathBuf,
    pub zstd_level: i32,
    pub mode: ExportMode,
    pub target_platform: Option<String>,
    pub with_git_history: bool,
    pub since: Option<String>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            lockfile_path: PathBuf::from("dmz.lock"),
            store_dir: PathBuf::from(".dmz/store"),
            output_path: PathBuf::from("dist/airgap_bundle.tar.zst"),
            zstd_level: 3,
            mode: ExportMode::Unified,
            target_platform: None,
            with_git_history: false,
            since: None,
        }
    }
}

#[derive(Debug)]
pub enum ExportError {
    IoError(String),
    LockfileError(String),
    ArchiveError(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Export IO Error: {}", msg),
            Self::LockfileError(msg) => write!(f, "Export Lockfile Error: {}", msg),
            Self::ArchiveError(msg) => write!(f, "Export Archive Error: {}", msg),
        }
    }
}

impl std::error::Error for ExportError {}

pub struct AirGapExporter;

impl AirGapExporter {
    /// Bundles store packages, dmz.lock, and discovered ecosystem caches into a cryptographically verifiable air-gap archive
    pub fn export(config: &ExportConfig) -> Result<AirGapManifest, ExportError> {
        let workspace_root = if config.workspace_path.is_relative() {
            std::fs::canonicalize(&config.workspace_path)
                .unwrap_or_else(|_| config.workspace_path.clone())
        } else {
            config.workspace_path.clone()
        };

        let effective_lockfile = if config.lockfile_path.is_relative() {
            if workspace_root.join(&config.lockfile_path).exists() {
                workspace_root.join(&config.lockfile_path)
            } else if config.lockfile_path.exists() {
                std::fs::canonicalize(&config.lockfile_path)
                    .unwrap_or_else(|_| config.lockfile_path.clone())
            } else {
                workspace_root.join(&config.lockfile_path)
            }
        } else {
            config.lockfile_path.clone()
        };

        let effective_store = if config.store_dir.is_relative() {
            if workspace_root.join(&config.store_dir).exists() {
                workspace_root.join(&config.store_dir)
            } else if config.store_dir.exists() {
                std::fs::canonicalize(&config.store_dir)
                    .unwrap_or_else(|_| config.store_dir.clone())
            } else {
                workspace_root.join(&config.store_dir)
            }
        } else {
            config.store_dir.clone()
        };

        let effective_output = if config.output_path.is_relative() {
            if config.output_path.exists() {
                std::fs::canonicalize(&config.output_path)
                    .unwrap_or_else(|_| workspace_root.join(&config.output_path))
            } else {
                workspace_root.join(&config.output_path)
            }
        } else {
            config.output_path.clone()
        };

        info!(
            workspace = ?workspace_root,
            lockfile = ?effective_lockfile,
            store = ?effective_store,
            output = ?effective_output,
            with_git = config.with_git_history,
            since = ?config.since,
            "Initiating air-gap package export"
        );

        // Detect VCS Provenance
        let vcs_provenance = GitProvider::get_provenance(&workspace_root).ok().flatten();
        if let Some(ref prov) = vcs_provenance {
            info!(commit = %prov.commit, branch = ?prov.branch, dirty = prov.dirty, "Detected workspace VCS provenance");
        }

        // Parse Delta Baseline if `--since` was requested
        let mut delta_base = None;
        let mut changed_files_filter: Option<HashSet<PathBuf>> = None;

        if let Some(since_spec) = &config.since {
            let since_path = Path::new(since_spec);
            if since_path.exists() {
                if since_spec.ends_with(".zst") || since_spec.ends_with(".tar.zst") {
                    if let Ok(prev_manifest) = Self::read_manifest_from_archive(since_path) {
                        let base_commit = prev_manifest.vcs.as_ref().map(|v| v.commit.clone());
                        delta_base = Some(DeltaBaseInfo {
                            base_commit: base_commit.clone(),
                            base_archive_sha256: Some(prev_manifest.archive_sha256),
                            base_closure_signature: Some(prev_manifest.closure_signature),
                        });
                        if let Some(ref commit) = base_commit {
                            if let Ok(files) =
                                GitProvider::get_changed_files_since(&workspace_root, commit)
                            {
                                changed_files_filter = Some(files.into_iter().collect());
                            }
                        }
                    }
                } else if since_path
                    .file_name()
                    .map(|n| n == "airgap_manifest.json")
                    .unwrap_or(false)
                {
                    if let Ok(content) = fs::read_to_string(since_path) {
                        if let Ok(prev_manifest) = serde_json::from_str::<AirGapManifest>(&content)
                        {
                            let base_commit = prev_manifest.vcs.as_ref().map(|v| v.commit.clone());
                            delta_base = Some(DeltaBaseInfo {
                                base_commit: base_commit.clone(),
                                base_archive_sha256: Some(prev_manifest.archive_sha256),
                                base_closure_signature: Some(prev_manifest.closure_signature),
                            });
                            if let Some(ref commit) = base_commit {
                                if let Ok(files) =
                                    GitProvider::get_changed_files_since(&workspace_root, commit)
                                {
                                    changed_files_filter = Some(files.into_iter().collect());
                                }
                            }
                        }
                    }
                }
            }

            if delta_base.is_none() {
                // Assume since_spec is a git commit hash or ref
                let base_commit = since_spec.trim().to_string();
                if let Ok(files) =
                    GitProvider::get_changed_files_since(&workspace_root, &base_commit)
                {
                    info!(base = %base_commit, count = files.len(), "Computed changed files since git ref");
                    changed_files_filter = Some(files.into_iter().collect());
                }
                delta_base = Some(DeltaBaseInfo {
                    base_commit: Some(base_commit),
                    base_archive_sha256: None,
                    base_closure_signature: None,
                });
            }
        }

        // Automatically discover active workspace ecosystems & gather generalized caches
        let registry = EcosystemRegistry::new();

        let detected_stacks = registry
            .process_workspace(&workspace_root)
            .map_err(|e| ExportError::ArchiveError(format!("Ecosystem discovery failed: {}", e)))?;

        info!(stacks = ?detected_stacks, "Discovered active workspace ecosystems");

        let cache_paths = registry.gather_all_caches(&workspace_root);
        info!(caches = ?cache_paths, "Gathered generalized ecosystem cache paths for archiving");

        // Verify dmz.lock integrity before exporting
        let mut lockfile = DmzLockfile::load_and_verify(&effective_lockfile)
            .map_err(|e| ExportError::LockfileError(e.to_string()))?;

        // Synchronize lockfile VCS provenance if not already set
        if lockfile.vcs.is_none() && vcs_provenance.is_some() {
            lockfile.vcs = vcs_provenance.clone();
            let _ = lockfile.save(&effective_lockfile);
        }

        // Ensure parent output directory exists
        if let Some(parent) = effective_output.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    ExportError::IoError(format!("Failed to create parent dir: {}", e))
                })?;
            }
        }

        // Prepare archive output stream
        let file = File::create(&effective_output)
            .map_err(|e| ExportError::IoError(format!("Failed to create export archive: {}", e)))?;

        let zstd_encoder = ZstdEncoder::new(file, config.zstd_level)
            .map_err(|e| ExportError::ArchiveError(format!("Failed to initialize zstd: {}", e)))?;

        let mut tar_builder = TarBuilder::new(zstd_encoder);

        // Append lockfile
        tar_builder
            .append_path_with_name(&effective_lockfile, "dmz.lock")
            .map_err(|e| ExportError::ArchiveError(format!("Failed to pack dmz.lock: {}", e)))?;

        // Append store items (if mode is Unified or DepsOnly)
        let mut packed_count = 0;
        if config.mode == ExportMode::Unified || config.mode == ExportMode::DepsOnly {
            for pkg_info in lockfile.packages.values() {
                // Skip local path dependencies/workspace members since they are in the workspace source
                if pkg_info.version.starts_with("path:") || pkg_info.version.contains("path") {
                    continue;
                }

                let safe_pkg_name = pkg_info.name.replace('/', "__");

                let pkg_store_path = effective_store.join(&safe_pkg_name);
                if pkg_store_path.exists() {
                    let archive_dest = Path::new("store").join(&safe_pkg_name);
                    if pkg_store_path.is_dir() {
                        Self::append_dir_all(&mut tar_builder, &pkg_store_path, &archive_dest)?;
                    } else {
                        tar_builder
                            .append_path_with_name(&pkg_store_path, &archive_dest)
                            .map_err(|e| {
                                ExportError::ArchiveError(format!(
                                    "Failed to pack package {}: {}",
                                    pkg_info.name, e
                                ))
                            })?;
                    }
                    packed_count += 1;
                } else {
                    warn!(package = %pkg_info.name, "Store item missing during export package");
                }
            }
        }

        // Append workspace source code (if mode is Unified or CodeOnly)
        if config.mode == ExportMode::Unified || config.mode == ExportMode::CodeOnly {
            info!(path = ?workspace_root, "Appending workspace source code to archive");
            if workspace_root.is_dir() {
                if let Some(ref changed_files) = changed_files_filter {
                    info!(
                        changed_count = changed_files.len(),
                        "Packaging incremental delta workspace files"
                    );
                    Self::append_filtered_files(&mut tar_builder, &workspace_root, changed_files)?;
                } else {
                    Self::append_dir_all(&mut tar_builder, &workspace_root, Path::new(""))?;
                }
            }
        }

        // Append Git Bundle if requested (`--with-git-history`)
        let mut has_git_history = false;
        if config.with_git_history {
            let temp_dir = tempfile::tempdir().map_err(|e| ExportError::IoError(e.to_string()))?;
            let temp_bundle = temp_dir.path().join("workspace.bundle");
            let since_commit_for_bundle =
                delta_base.as_ref().and_then(|d| d.base_commit.as_deref());

            match GitProvider::create_bundle(&workspace_root, &temp_bundle, since_commit_for_bundle)
            {
                Ok(binfo) => {
                    info!(
                        objects = binfo.object_count,
                        head = %binfo.head_commit,
                        "Adding deterministic workspace.bundle to air-gap archive"
                    );
                    tar_builder
                        .append_path_with_name(&temp_bundle, "workspace.bundle")
                        .map_err(|e| {
                            ExportError::ArchiveError(format!(
                                "Failed to pack workspace.bundle: {}",
                                e
                            ))
                        })?;
                    has_git_history = true;
                }
                Err(e) => {
                    warn!("Could not create git bundle for workspace: {}", e);
                }
            }
        }

        let target_platform = config
            .target_platform
            .clone()
            .or_else(|| lockfile.target_platform.clone());

        // Write air-gap manifest header
        let manifest = AirGapManifest {
            dmz_version: env!("CARGO_PKG_VERSION").to_string(),
            target_platform,
            vcs: vcs_provenance,
            delta_base,
            has_git_history,
            closure_signature: lockfile.closure_signature.clone(),
            package_count: packed_count,
            created_at_utc: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            archive_sha256: String::new(), // Populated post-compression
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| ExportError::ArchiveError(e.to_string()))?;

        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_json.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        tar_builder
            .append_data(
                &mut header,
                "airgap_manifest.json",
                manifest_json.as_bytes(),
            )
            .map_err(|e| {
                ExportError::ArchiveError(format!("Failed to embed airgap manifest: {}", e))
            })?;

        // Finish streams and ensure file handle is fully closed
        {
            let zstd_encoder = tar_builder.into_inner().map_err(|e| {
                ExportError::ArchiveError(format!("Failed to finalize tar stream: {}", e))
            })?;

            let mut inner_file = zstd_encoder
                .finish()
                .map_err(|e| format!("Failed to finish zstd compression: {}", e))
                .map_err(ExportError::ArchiveError)?;

            inner_file
                .flush()
                .map_err(|e| ExportError::IoError(e.to_string()))?;

            inner_file
        };

        // Compute final archive checksum
        let archive_sha256 = Self::calculate_file_sha256(&effective_output)?;
        let mut final_manifest = manifest;
        final_manifest.archive_sha256 = archive_sha256;

        info!(
            checksum = %final_manifest.archive_sha256,
            packages = final_manifest.package_count,
            has_git = final_manifest.has_git_history,
            delta = final_manifest.delta_base.is_some(),
            "Air-gap export completed successfully"
        );

        Ok(final_manifest)
    }

    fn append_dir_all<W: Write>(
        tar: &mut TarBuilder<W>,
        src_dir: &Path,
        dest_prefix: &Path,
    ) -> Result<(), ExportError> {
        let entries = fs::read_dir(src_dir).map_err(|e| {
            ExportError::IoError(format!("Directory read failed {:?}: {}", src_dir, e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| ExportError::IoError(e.to_string()))?;
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // --- SKIP HEAVY BUILD ARTIFACTS & CACHES ---
            if path.is_dir()
                && (name_str == "target"
                    || name_str == "node_modules"
                    || name_str == ".git"
                    || name_str == "dist"
                    || name_str == ".dmz")
            {
                continue;
            }

            let rel_dest = dest_prefix.join(name);

            if path.is_dir() {
                Self::append_dir_all(tar, &path, &rel_dest)?;
            } else {
                tar.append_path_with_name(&path, &rel_dest).map_err(|e| {
                    ExportError::ArchiveError(format!("Failed to add file {:?}: {}", path, e))
                })?;
            }
        }

        Ok(())
    }

    fn append_filtered_files<W: Write>(
        tar: &mut TarBuilder<W>,
        workspace_root: &Path,
        files: &HashSet<PathBuf>,
    ) -> Result<(), ExportError> {
        for rel_path in files {
            // Skip .git, target, node_modules, dist, .dmz
            let starts_with_ignored = rel_path
                .components()
                .next()
                .map(|c| {
                    let s = c.as_os_str().to_string_lossy();
                    s == "target"
                        || s == "node_modules"
                        || s == ".git"
                        || s == "dist"
                        || s == ".dmz"
                })
                .unwrap_or(false);

            if starts_with_ignored {
                continue;
            }

            let full_path = workspace_root.join(rel_path);
            if full_path.is_file() {
                tar.append_path_with_name(&full_path, rel_path)
                    .map_err(|e| {
                        ExportError::ArchiveError(format!(
                            "Failed to add delta file {:?}: {}",
                            rel_path, e
                        ))
                    })?;
            }
        }
        Ok(())
    }

    fn read_manifest_from_archive(path: &Path) -> Result<AirGapManifest, String> {
        let file = File::open(path).map_err(|e| format!("Failed to open since archive: {}", e))?;
        let decoder =
            zstd::stream::read::Decoder::new(file).map_err(|e| format!("Zstd error: {}", e))?;
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().map_err(|e| format!("Tar error: {}", e))? {
            let mut entry = entry.map_err(|e| format!("Tar entry error: {}", e))?;
            let entry_path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
            if entry_path == Path::new("airgap_manifest.json") {
                let mut content = String::new();
                entry
                    .read_to_string(&mut content)
                    .map_err(|e| format!("Read error: {}", e))?;
                let manifest: AirGapManifest = serde_json::from_str(&content)
                    .map_err(|e| format!("JSON parse error: {}", e))?;
                return Ok(manifest);
            }
        }
        Err("airgap_manifest.json not found in archive".to_string())
    }

    fn calculate_file_sha256(path: &Path) -> Result<String, ExportError> {
        let mut file = File::open(path)
            .map_err(|e| ExportError::IoError(format!("Failed to open file for hash: {}", e)))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];

        loop {
            let count = file.read(&mut buffer).map_err(|e| {
                ExportError::IoError(format!("File read error during hashing: {}", e))
            })?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }

        Ok(format!("{:x}", hasher.finalize()))
    }
}
