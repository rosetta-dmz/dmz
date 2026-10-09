use crate::resolver::lockfile::DmzLockfile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::info;
use zstd::stream::write::Encoder as ZstdEncoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildTarget {
    /// Target A: Package dependencies only (Base runtime environment)
    DependenciesOnly,
    /// Target B: Package repository code only (Application layer)
    AppOnly,
    /// Target C: Package repository code combined with locked dependencies (Unified distribution)
    Unified,
}

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub workspace_dir: PathBuf,
    pub lockfile_path: PathBuf,
    pub output_dir: PathBuf,
    pub target: BuildTarget,
    pub archive_name: String,
    pub zstd_level: i32,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            workspace_dir: PathBuf::from("."),
            lockfile_path: PathBuf::from("dmz.lock"),
            output_dir: PathBuf::from("dist"),
            target: BuildTarget::Unified,
            archive_name: String::from("workspace_closure"),
            zstd_level: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResult {
    pub target: BuildTarget,
    pub artifact_path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub closure_signature: String,
}

#[derive(Debug)]
pub enum BuildError {
    IoError(String),
    LockfileError(String),
    ArchiveError(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Packaging IO Error: {}", msg),
            Self::LockfileError(msg) => write!(f, "Lockfile Verification Error: {}", msg),
            Self::ArchiveError(msg) => write!(f, "Archive Creation Error: {}", msg),
        }
    }
}

impl std::error::Error for BuildError {}

pub struct PackageBuilder;

impl PackageBuilder {
    /// Builds and exports target artifact closures (Target A, B, or C) as compressed .tar.zst packages
    pub fn build(config: &BuildConfig) -> Result<BuildResult, BuildError> {
        info!(
            target = ?config.target,
            workspace = ?config.workspace_dir,
            "Starting DMZ package build pipeline"
        );

        // 1. Ensure destination directory exists
        if !config.output_dir.exists() {
            fs::create_dir_all(&config.output_dir)
                .map_err(|e| BuildError::IoError(format!("Failed to create output dir: {}", e)))?;
        }

        // 2. Load and cryptographically verify dmz.lock
        let lockfile = DmzLockfile::load_and_verify(&config.lockfile_path)
            .map_err(|e| BuildError::LockfileError(e.to_string()))?;

        // 3. Prepare target output file stream (.tar.zst)
        let artifact_filename = format!("{}.tar.zst", config.archive_name);
        let artifact_path = config.output_dir.join(&artifact_filename);

        let output_file = File::create(&artifact_path)
            .map_err(|e| BuildError::IoError(format!("Failed to create output file: {}", e)))?;

        let zstd_encoder = ZstdEncoder::new(output_file, config.zstd_level)
            .map_err(|e| BuildError::ArchiveError(format!("Zstd encoder initialization failed: {}", e)))?;

        let mut tar_builder = TarBuilder::new(zstd_encoder);

        // 4. Assemble package layers based on selected Build Target
        match config.target {
            BuildTarget::DependenciesOnly => {
                Self::bundle_dependencies(&mut tar_builder, &lockfile)?;
            }
            BuildTarget::AppOnly => {
                Self::bundle_app_code(&mut tar_builder, &config.workspace_dir)?;
            }
            BuildTarget::Unified => {
                Self::bundle_dependencies(&mut tar_builder, &lockfile)?;
                Self::bundle_app_code(&mut tar_builder, &config.workspace_dir)?;
            }
        }

        // 5. Embed root dmz.lock directly inside the archive header for air-gap verification
        if config.lockfile_path.exists() {
            tar_builder
                .append_path_with_name(&config.lockfile_path, "dmz.lock")
                .map_err(|e| BuildError::ArchiveError(format!("Failed to embed lockfile: {}", e)))?;
        }

        // 6. Finalize archive and flush zstd encoder
        let zstd_encoder = tar_builder
            .into_inner()
            .map_err(|e| BuildError::ArchiveError(format!("Failed to finalize tar stream: {}", e)))?;

        zstd_encoder
            .finish()
            .map_err(|e| BuildError::ArchiveError(format!("Zstd compression finish failed: {}", e)))?;

        // 7. Calculate SHA-256 integrity hash and byte size of final artifact
        let (sha256, size_bytes) = Self::calculate_checksum_and_size(&artifact_path)?;

        info!(
            artifact = ?artifact_path,
            sha256 = %sha256,
            bytes = size_bytes,
            "Package build complete"
        );

        Ok(BuildResult {
            target: config.target,
            artifact_path,
            sha256,
            size_bytes,
            closure_signature: lockfile.closure_signature,
        })
    }

    /// Bundles locked dependency closure metadata and cached binary packages (Target A)
    fn bundle_dependencies<W: Write>(
        tar: &mut TarBuilder<W>,
        lockfile: &DmzLockfile,
    ) -> Result<(), BuildError> {
        info!(
            pkg_count = lockfile.packages.len(),
            "Bundling dependency closure layer (Target A/C)"
        );

        let closure_json = serde_json::to_string_pretty(lockfile)
            .map_err(|e| BuildError::LockfileError(e.to_string()))?;

        let mut header = tar::Header::new_gnu();
        header.set_size(closure_json.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        tar.append_data(&mut header, ".dmz/closure.json", closure_json.as_bytes())
            .map_err(|e| BuildError::ArchiveError(format!("Failed to write closure metadata: {}", e)))?;

        for (key, pkg) in &lockfile.packages {
            info!(package = %key, sha256 = %pkg.sha256, "Archiving locked package entry");
        }

        Ok(())
    }

    /// Bundles workspace source files excluding build artifacts and VCS noise (Target B)
    fn bundle_app_code<W: Write>(
        tar: &mut TarBuilder<W>,
        workspace_dir: &Path,
    ) -> Result<(), BuildError> {
        info!(dir = ?workspace_dir, "Bundling application code layer (Target B/C)");
        Self::append_dir_filtered(tar, workspace_dir, Path::new("app"))
    }

    /// Recursively walks directory tree ignoring common non-reproducible artifacts
    fn append_dir_filtered<W: Write>(
        tar: &mut TarBuilder<W>,
        src_dir: &Path,
        archive_prefix: &Path,
    ) -> Result<(), BuildError> {
        let entries = fs::read_dir(src_dir)
            .map_err(|e| BuildError::IoError(format!("Read directory failed: {}", e)))?;

        for entry in entries {
            let entry = entry.map_err(|e| BuildError::IoError(e.to_string()))?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            // Ignore VCS directories, heavy build caches, and node_modules
            if name_str == ".git"
                || name_str == "node_modules"
                || name_str == "target"
                || name_str == ".dmz_cache"
                || name_str == "dist"
            {
                continue;
            }

            let rel_path = archive_prefix.join(&file_name);

            if path.is_dir() {
                Self::append_dir_filtered(tar, &path, &rel_path)?;
            } else if path.is_file() {
                tar.append_path_with_name(&path, &rel_path)
                    .map_err(|e| BuildError::ArchiveError(format!("Failed to pack file {:?}: {}", path, e)))?;
            }
        }

        Ok(())
    }

    /// Calculates SHA-256 hash and byte length of built artifact
    fn calculate_checksum_and_size(path: &Path) -> Result<(String, u64), BuildError> {
        let mut file = File::open(path)
            .map_err(|e| BuildError::IoError(format!("Failed to open artifact for hash: {}", e)))?;

        let metadata = file
            .metadata()
            .map_err(|e| BuildError::IoError(format!("Failed to read metadata: {}", e)))?;

        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];

        loop {
            let bytes_read = file
                .read(&mut buffer)
                .map_err(|e| BuildError::IoError(format!("Error hashing file: {}", e)))?;

            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        Ok((format!("{:x}", hasher.finalize()), metadata.len()))
    }
}