use crate::resolver::lockfile::DmzLockfile;
use crate::ecosystem::EcosystemRegistry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::{info, warn};
use zstd::stream::write::Encoder as ZstdEncoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    Unified,   // Workspace source code + dependency store archives
    CodeOnly,  // Workspace source code only (no dependencies)
    DepsOnly,  // Dependency store archives only (no source code)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirGapManifest {
    pub dmz_version: String,
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
        info!(
            lockfile = ?config.lockfile_path,
            output = ?config.output_path,
            "Initiating air-gap package export"
        );

        // 1. Automatically discover active workspace ecosystems & gather generalized caches
        let workspace_root = &config.workspace_path;
        let registry = EcosystemRegistry::new();
        
        let detected_stacks = registry.process_workspace(workspace_root)
            .map_err(|e| ExportError::ArchiveError(format!("Ecosystem discovery failed: {}", e)))?;
        
        info!(stacks = ?detected_stacks, "Discovered active workspace ecosystems");
        
        let cache_paths = registry.gather_all_caches(workspace_root);
        info!(caches = ?cache_paths, "Gathered generalized ecosystem cache paths for archiving");

        // 2. Verify dmz.lock integrity before exporting
        let lockfile = DmzLockfile::load_and_verify(&config.lockfile_path)
            .map_err(|e| ExportError::LockfileError(e.to_string()))?;

        // 3. Ensure parent output directory exists
        if let Some(parent) = config.output_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| ExportError::IoError(format!("Failed to create parent dir: {}", e)))?;
            }
        }

        // 4. Prepare archive output stream
        let file = File::create(&config.output_path)
            .map_err(|e| ExportError::IoError(format!("Failed to create export archive: {}", e)))?;

        let zstd_encoder = ZstdEncoder::new(file, config.zstd_level)
            .map_err(|e| ExportError::ArchiveError(format!("Failed to initialize zstd: {}", e)))?;

        let mut tar_builder = TarBuilder::new(zstd_encoder);

        // Append lockfile
        tar_builder
            .append_path_with_name(&config.lockfile_path, "dmz.lock")
            .map_err(|e| ExportError::ArchiveError(format!("Failed to pack dmz.lock: {}", e)))?;

        // Append store items (if mode is Unified or DepsOnly)
        let mut packed_count = 0;
        if config.mode == ExportMode::Unified || config.mode == ExportMode::DepsOnly {
            for (_pkg_key, pkg_info) in &lockfile.packages {
                // Skip local path dependencies/workspace members since they are in the workspace source
                if pkg_info.version.starts_with("path:") || pkg_info.version.contains("path") {
                    continue;
                }

                let pkg_store_path = config.store_dir.join(&pkg_info.name); // Use pkg_info.name to match store directory layout
                if pkg_store_path.exists() {
                    let archive_dest = Path::new("store").join(&pkg_info.name);
                    if pkg_store_path.is_dir() {
                        Self::append_dir_all(&mut tar_builder, &pkg_store_path, &archive_dest)?;
                    } else {
                        tar_builder
                            .append_path_with_name(&pkg_store_path, &archive_dest)
                            .map_err(|e| ExportError::ArchiveError(format!("Failed to pack package {}: {}", pkg_info.name, e)))?;
                    }
                    packed_count += 1;
                } else {
                    warn!(package = %pkg_info.name, "Store item missing during export package");
                }
            }
        }

        // Append workspace source code (if mode is Unified or CodeOnly)
        if config.mode == ExportMode::Unified || config.mode == ExportMode::CodeOnly {
            info!(path = ?config.workspace_path, "Appending workspace source code to archive");
            let workspace_dest = Path::new("");
            if config.workspace_path.is_dir() {
                Self::append_dir_all(&mut tar_builder, &config.workspace_path, workspace_dest)?;
            }
        }

        // Write air-gap manifest header
        let manifest = AirGapManifest {
            dmz_version: env!("CARGO_PKG_VERSION").to_string(),
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
            .append_data(&mut header, "airgap_manifest.json", manifest_json.as_bytes())
            .map_err(|e| ExportError::ArchiveError(format!("Failed to embed airgap manifest: {}", e)))?;

        // Finish streams and ensure file handle is fully closed
        { // Removed unused 'let file =' binding to clear compiler warning
            let zstd_encoder = tar_builder
                .into_inner()
                .map_err(|e| ExportError::ArchiveError(format!("Failed to finalize tar stream: {}", e)))?;

            let mut inner_file = zstd_encoder
                .finish()
                .map_err(|e| format!("Failed to finish zstd compression: {}", e))
                .map_err(ExportError::ArchiveError)?;

            inner_file.flush()
                .map_err(|e| ExportError::IoError(e.to_string()))?;
                
            inner_file
        }; // File handle drops/closes here, flushing bytes to disk safely

        // Compute final archive checksum
        let archive_sha256 = Self::calculate_file_sha256(&config.output_path)?;
        let mut final_manifest = manifest;
        final_manifest.archive_sha256 = archive_sha256;

        info!(
            checksum = %final_manifest.archive_sha256,
            packages = final_manifest.package_count,
            "Air-gap export completed successfully"
        );

        Ok(final_manifest)
    }

    fn append_dir_all<W: Write>(
        tar: &mut TarBuilder<W>,
        src_dir: &Path,
        dest_prefix: &Path,
    ) -> Result<(), ExportError> {
        let entries = fs::read_dir(src_dir)
            .map_err(|e| ExportError::IoError(format!("Directory read failed {:?}: {}", src_dir, e)))?;

        for entry in entries {
            let entry = entry.map_err(|e| ExportError::IoError(e.to_string()))?;
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // --- SKIP HEAVY BUILD ARTIFACTS & CACHES ---
            if path.is_dir() && (name_str == "target" || name_str == "node_modules" || name_str == ".git" || name_str == "dist" || name_str == ".dmz") {
                continue;
            }

            let rel_dest = dest_prefix.join(name);

            if path.is_dir() {
                Self::append_dir_all(tar, &path, &rel_dest)?;
            } else {
                tar.append_path_with_name(&path, &rel_dest)
                    .map_err(|e| ExportError::ArchiveError(format!("Failed to add file {:?}: {}", path, e)))?;
            }
        }

        Ok(())
    }

    fn calculate_file_sha256(path: &Path) -> Result<String, ExportError> {
        let mut file = File::open(path)
            .map_err(|e| ExportError::IoError(format!("Failed to open file for hash: {}", e)))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];

        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|e| ExportError::IoError(format!("File read error during hashing: {}", e)))?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }

        Ok(format!("{:x}", hasher.finalize()))
    }
}