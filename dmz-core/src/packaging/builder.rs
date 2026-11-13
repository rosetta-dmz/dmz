use crate::archive::exporter::HashingWriter;
use crate::resolver::lockfile::DmzLockfile;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::{info, warn};
use zstd::stream::write::Encoder as ZstdEncoder;

/// Deterministic build timestamp fallback
pub const DEFAULT_SOURCE_EPOCH: u64 = 315_532_800;

/// Defense-in-depth resource limits against decompression bombs and recursive directory explosion (CWE-400).
pub const DEFAULT_MAX_ARCHIVE_ENTRIES: usize = 250_000;
pub const DEFAULT_MAX_UNCOMPRESSED_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GB
pub const DEFAULT_MAX_RECURSION_DEPTH: usize = 64;

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

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Packaging IO Error: {0}")]
    IoError(String),
    #[error("Lockfile Verification Error: {0}")]
    LockfileError(String),
    #[error("Archive Creation Error: {0}")]
    ArchiveError(String),
    #[error("Resource Quota Exceeded: {0}")]
    QuotaExceeded(String),
}

impl From<std::io::Error> for BuildError {
    fn from(err: std::io::Error) -> Self {
        BuildError::IoError(err.to_string())
    }
}

/// Tracks packed resource counts to defend against decompression bombs and unbounded growth (CWE-400).
struct PackStats {
    total_entries: usize,
    total_bytes: u64,
}

impl PackStats {
    fn record_entry(&mut self, bytes: u64) -> Result<(), BuildError> {
        self.total_entries = self.total_entries.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(bytes);

        if self.total_entries > DEFAULT_MAX_ARCHIVE_ENTRIES {
            return Err(BuildError::QuotaExceeded(format!(
                "Archive entry count exceeded quota limit of {}",
                DEFAULT_MAX_ARCHIVE_ENTRIES
            )));
        }

        if self.total_bytes > DEFAULT_MAX_UNCOMPRESSED_BYTES {
            return Err(BuildError::QuotaExceeded(format!(
                "Archive uncompressed byte size exceeded quota limit of {} bytes",
                DEFAULT_MAX_UNCOMPRESSED_BYTES
            )));
        }

        Ok(())
    }
}

pub struct PackageBuilder;

impl PackageBuilder {
    /// Builds and exports target artifact closures (Target A, B, or C) as compressed .tar.zst packages
    pub fn build(config: &BuildConfig) -> Result<BuildResult, BuildError> {
        let workspace_root = if config.workspace_dir.is_relative() {
            std::fs::canonicalize(&config.workspace_dir)
                .unwrap_or_else(|_| config.workspace_dir.clone())
        } else {
            config.workspace_dir.clone()
        };

        let effective_output_dir = if config.output_dir.is_relative() {
            workspace_root.join(&config.output_dir)
        } else {
            config.output_dir.clone()
        };

        let effective_lockfile = if config.lockfile_path.is_relative() {
            workspace_root.join(&config.lockfile_path)
        } else {
            config.lockfile_path.clone()
        };

        info!(
            target = ?config.target,
            workspace = ?workspace_root,
            lockfile = ?effective_lockfile,
            output_dir = ?effective_output_dir,
            "Starting DMZ package build pipeline"
        );

        // Ensure destination directory exists (idempotent, atomic; avoids TOCTOU gap)
        fs::create_dir_all(&effective_output_dir)
            .map_err(|e| BuildError::IoError(format!("Failed to create output dir: {}", e)))?;

        // Load and cryptographically verify dmz.lock
        let lockfile = DmzLockfile::load_and_verify(&effective_lockfile)
            .map_err(|e| BuildError::LockfileError(e.to_string()))?;

        // Prepare target output file stream (.tar.zst) using temporary staging file to prevent partial corrupt artifacts
        let artifact_filename = format!("{}.tar.zst", config.archive_name);
        let artifact_path = effective_output_dir.join(&artifact_filename);
        let temp_artifact_path =
            effective_output_dir.join(format!("{}.tmp.{}", artifact_filename, std::process::id()));

        let output_file = File::create(&temp_artifact_path).map_err(|e| {
            BuildError::IoError(format!("Failed to create staging output file: {}", e))
        })?;
        let hashing_writer = HashingWriter::new(BufWriter::with_capacity(64 * 1024, output_file));

        let zstd_encoder = ZstdEncoder::new(hashing_writer, config.zstd_level).map_err(|e| {
            BuildError::ArchiveError(format!("Zstd encoder initialization failed: {}", e))
        })?;

        let mut tar_builder = TarBuilder::new(zstd_encoder);

        let epoch = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_SOURCE_EPOCH);

        let mut stats = PackStats {
            total_entries: 0,
            total_bytes: 0,
        };

        // Assemble package layers based on selected Build Target
        match config.target {
            BuildTarget::DependenciesOnly => {
                Self::bundle_dependencies(
                    &mut tar_builder,
                    &lockfile,
                    &workspace_root,
                    epoch,
                    &effective_output_dir,
                    &mut stats,
                )?;
            }
            BuildTarget::AppOnly => {
                Self::bundle_app_code(
                    &mut tar_builder,
                    &workspace_root,
                    epoch,
                    &effective_output_dir,
                    &mut stats,
                )?;
            }
            BuildTarget::Unified => {
                Self::bundle_dependencies(
                    &mut tar_builder,
                    &lockfile,
                    &workspace_root,
                    epoch,
                    &effective_output_dir,
                    &mut stats,
                )?;
                Self::bundle_app_code(
                    &mut tar_builder,
                    &workspace_root,
                    epoch,
                    &effective_output_dir,
                    &mut stats,
                )?;
            }
        }

        // Embed root dmz.lock directly inside the archive header with normalized reproducible headers
        if effective_lockfile.exists() {
            let lockfile_bytes = fs::read(&effective_lockfile).map_err(|e| {
                BuildError::IoError(format!(
                    "Failed to read lockfile for archive embedding: {}",
                    e
                ))
            })?;
            let mut lock_header =
                Self::create_normalized_header(lockfile_bytes.len() as u64, 0o644, epoch)?;
            tar_builder
                .append_data(&mut lock_header, "dmz.lock", &lockfile_bytes[..])
                .map_err(|e| {
                    BuildError::ArchiveError(format!("Failed to embed lockfile: {}", e))
                })?;
            stats.record_entry(lockfile_bytes.len() as u64)?;
        }

        // Finalize archive, flush zstd encoder, and obtain single-pass SHA-256 and byte length
        let zstd_encoder = tar_builder.into_inner().map_err(|e| {
            BuildError::ArchiveError(format!("Failed to finalize tar stream: {}", e))
        })?;

        let hashing_writer = zstd_encoder.finish().map_err(|e| {
            BuildError::ArchiveError(format!("Zstd compression finish failed: {}", e))
        })?;

        let (sha256, size_bytes) = hashing_writer.finalize().map_err(|e| {
            BuildError::IoError(format!("Failed to flush and finalize output hash: {}", e))
        })?;

        // Transactional commit to target artifact path
        fs::rename(&temp_artifact_path, &artifact_path).map_err(|e| {
            BuildError::IoError(format!("Failed to commit artifact to destination: {}", e))
        })?;

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

    /// Creates a deterministic, normalized GNU tar header with zeroed UID/GID and fixed timestamp
    fn create_normalized_header(
        size: u64,
        mode: u32,
        epoch: u64,
    ) -> Result<tar::Header, BuildError> {
        let mut header = tar::Header::new_gnu();
        header.set_size(size);
        header.set_mode(mode);
        header.set_mtime(epoch);
        header.set_uid(0);
        header.set_gid(0);
        header
            .set_username("")
            .map_err(|e| BuildError::ArchiveError(format!("Failed to set tar username: {}", e)))?;
        header
            .set_groupname("")
            .map_err(|e| BuildError::ArchiveError(format!("Failed to set tar groupname: {}", e)))?;
        header.set_cksum();
        Ok(header)
    }

    /// Validates whether a relative symlink target stays strictly within its extraction root
    fn is_safe_relative_symlink(target: &Path) -> bool {
        if target.is_absolute() {
            return false;
        }
        let mut depth: usize = 0;
        for c in target.components() {
            match c {
                Component::Normal(_) => {
                    depth = depth.saturating_add(1);
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                Component::RootDir | Component::Prefix(_) => {
                    return false;
                }
            }
        }
        true
    }

    /// Bundles locked dependency closure metadata and cached binary packages (Target A)
    fn bundle_dependencies<W: Write>(
        tar: &mut TarBuilder<W>,
        lockfile: &DmzLockfile,
        workspace_root: &Path,
        epoch: u64,
        output_dir: &Path,
        stats: &mut PackStats,
    ) -> Result<(), BuildError> {
        info!(
            pkg_count = lockfile.packages.len(),
            "Bundling dependency closure layer (Target A/C)"
        );

        let closure_json = serde_json::to_string_pretty(lockfile)
            .map_err(|e| BuildError::LockfileError(e.to_string()))?;

        let mut header = Self::create_normalized_header(closure_json.len() as u64, 0o644, epoch)?;

        tar.append_data(&mut header, ".dmz/closure.json", closure_json.as_bytes())
            .map_err(|e| {
                BuildError::ArchiveError(format!("Failed to write closure metadata: {}", e))
            })?;

        stats.record_entry(closure_json.len() as u64)?;

        let store_root = workspace_root.join(".dmz").join("store");
        for (key, pkg) in &lockfile.packages {
            info!(package = %key, sha256 = %pkg.sha256, "Archiving locked package entry");
            let safe_pkg_name = pkg.name.replace('/', "__").replace('\\', "_");
            let pkg_store_dir = store_root.join(&safe_pkg_name);
            if pkg_store_dir.is_dir() {
                Self::append_dir_filtered(
                    tar,
                    &pkg_store_dir,
                    &Path::new("store").join(&safe_pkg_name),
                    epoch,
                    output_dir,
                    0,
                    stats,
                )?;
            } else {
                warn!(
                    package = %key,
                    expected_dir = ?pkg_store_dir,
                    "Locked package store directory not found in local workspace store"
                );
            }
        }

        Ok(())
    }

    /// Bundles workspace source files excluding build artifacts and VCS noise (Target B)
    fn bundle_app_code<W: Write>(
        tar: &mut TarBuilder<W>,
        workspace_dir: &Path,
        epoch: u64,
        output_dir: &Path,
        stats: &mut PackStats,
    ) -> Result<(), BuildError> {
        info!(dir = ?workspace_dir, "Bundling application code layer (Target B/C)");
        Self::append_dir_filtered(
            tar,
            workspace_dir,
            Path::new("app"),
            epoch,
            output_dir,
            0,
            stats,
        )
    }

    /// Recursively walks directory tree ignoring common non-reproducible artifacts, with sorted entries and normalized headers
    fn append_dir_filtered<W: Write>(
        tar: &mut TarBuilder<W>,
        src_dir: &Path,
        archive_prefix: &Path,
        epoch: u64,
        output_dir: &Path,
        depth: usize,
        stats: &mut PackStats,
    ) -> Result<(), BuildError> {
        if depth > DEFAULT_MAX_RECURSION_DEPTH {
            return Err(BuildError::QuotaExceeded(format!(
                "Directory recursion limit ({}) exceeded at {:?}",
                DEFAULT_MAX_RECURSION_DEPTH, src_dir
            )));
        }

        let entries = fs::read_dir(src_dir).map_err(|e| {
            BuildError::IoError(format!("Read directory failed on {:?}: {}", src_dir, e))
        })?;

        let mut sorted_entries = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| BuildError::IoError(e.to_string()))?;
            sorted_entries.push(entry);
        }
        sorted_entries.sort_by_key(|e| e.file_name());

        for entry in sorted_entries {
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            // Ignore VCS directories, heavy build caches, store dir, active output dir, and dist
            if name_str == ".git"
                || name_str == ".dmz"
                || name_str == "node_modules"
                || name_str == "target"
                || name_str == ".dmz_cache"
                || name_str == "dist"
                || path == output_dir
            {
                continue;
            }

            // CRITICAL DEFENSE: Use symlink_metadata to prevent following external symlinks or recursive loops
            let sym_meta = fs::symlink_metadata(&path).map_err(|e| {
                BuildError::IoError(format!("Failed to read symlink metadata {:?}: {}", path, e))
            })?;

            let rel_path = archive_prefix.join(&file_name);

            if sym_meta.file_type().is_symlink() {
                // Safely validate symlink target to prevent path traversal (CWE-22)
                match fs::read_link(&path) {
                    Ok(target) => {
                        if Self::is_safe_relative_symlink(&target) {
                            let mut header = Self::create_normalized_header(0, 0o777, epoch)?;
                            header.set_entry_type(tar::EntryType::Symlink);
                            let normalized_path = rel_path.to_string_lossy().replace('\\', "/");
                            tar.append_link(&mut header, &normalized_path, &target)
                                .map_err(|e| {
                                    BuildError::ArchiveError(format!(
                                        "Failed to pack symlink {:?}: {}",
                                        path, e
                                    ))
                                })?;
                            stats.record_entry(0)?;
                        } else {
                            warn!(
                                symlink = ?path,
                                target = ?target,
                                "Skipping symlink pointing outside extraction boundary (CWE-22 defense)"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(symlink = ?path, error = %e, "Skipping unreadable symlink");
                    }
                }
            } else if sym_meta.is_dir() {
                Self::append_dir_filtered(
                    tar,
                    &path,
                    &rel_path,
                    epoch,
                    output_dir,
                    depth + 1,
                    stats,
                )?;
            } else if sym_meta.is_file() {
                stats.record_entry(sym_meta.len())?;

                let mut file = File::open(&path).map_err(|e| {
                    BuildError::IoError(format!("Failed to open {:?}: {}", path, e))
                })?;

                let is_executable = {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        sym_meta.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        let p_str = rel_path.to_string_lossy();
                        p_str.ends_with(".exe")
                            || p_str.ends_with(".bat")
                            || p_str.ends_with(".cmd")
                            || p_str.ends_with(".sh")
                    }
                };

                let mode = if is_executable { 0o755 } else { 0o644 };
                let mut header = Self::create_normalized_header(sym_meta.len(), mode, epoch)?;
                let normalized_path = rel_path.to_string_lossy().replace('\\', "/");

                tar.append_data(&mut header, &normalized_path, &mut file)
                    .map_err(|e| {
                        BuildError::ArchiveError(format!("Failed to pack file {:?}: {}", path, e))
                    })?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_build_config() {
        let config = BuildConfig::default();
        assert_eq!(config.workspace_dir, PathBuf::from("."));
        assert_eq!(config.lockfile_path, PathBuf::from("dmz.lock"));
        assert_eq!(config.output_dir, PathBuf::from("dist"));
        assert_eq!(config.target, BuildTarget::Unified);
        assert_eq!(config.archive_name, "workspace_closure");
        assert_eq!(config.zstd_level, 3);
    }

    #[test]
    fn test_is_safe_relative_symlink() {
        assert!(PackageBuilder::is_safe_relative_symlink(Path::new(
            "foo/bar"
        )));
        assert!(PackageBuilder::is_safe_relative_symlink(Path::new(
            "./foo/bar"
        )));
        assert!(PackageBuilder::is_safe_relative_symlink(Path::new(
            "foo/../bar"
        )));
        assert!(!PackageBuilder::is_safe_relative_symlink(Path::new(
            "/etc/shadow"
        )));
        assert!(!PackageBuilder::is_safe_relative_symlink(Path::new(
            "../escape"
        )));
        assert!(!PackageBuilder::is_safe_relative_symlink(Path::new(
            "foo/../../escape"
        )));
    }

    #[test]
    fn test_create_normalized_header() {
        let header = PackageBuilder::create_normalized_header(1024, 0o644, 315_532_800).unwrap();
        assert_eq!(header.size().unwrap(), 1024);
        assert_eq!(header.mode().unwrap(), 0o644);
        assert_eq!(header.mtime().unwrap(), 315_532_800);
        assert_eq!(header.uid().unwrap(), 0);
        assert_eq!(header.gid().unwrap(), 0);
    }

    #[test]
    fn test_quota_stats_tripwires() {
        let mut stats = PackStats {
            total_entries: DEFAULT_MAX_ARCHIVE_ENTRIES,
            total_bytes: 0,
        };
        assert!(stats.record_entry(10).is_err());

        let mut stats_bytes = PackStats {
            total_entries: 0,
            total_bytes: DEFAULT_MAX_UNCOMPRESSED_BYTES,
        };
        assert!(stats_bytes.record_entry(1).is_err());
    }
}
