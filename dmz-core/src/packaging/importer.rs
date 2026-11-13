use crate::resolver::lockfile::DmzLockfile;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use tracing::info;

pub const DEFAULT_MAX_FILES: usize = 250_000;
pub const DEFAULT_MAX_DECOMPRESSED_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GB

#[derive(Debug, thiserror::Error)]
pub enum PackagingImportError {
    #[error("Import I/O Error: {0}")]
    IoError(String),
    #[error("Cryptographic Verification Failed: {0}")]
    VerificationFailed(String),
    #[error("Archive Extraction Failed: {0}")]
    ExtractionError(String),
    #[error("Security Violation: {0}")]
    SecurityViolation(String),
    #[error("Lockfile Verification Failed: {0}")]
    LockfileError(String),
    #[error("Resource Quota Exceeded: {0}")]
    QuotaExceeded(String),
}

impl From<PackagingImportError> for String {
    fn from(err: PackagingImportError) -> Self {
        err.to_string()
    }
}

impl From<std::io::Error> for PackagingImportError {
    fn from(err: std::io::Error) -> Self {
        PackagingImportError::IoError(err.to_string())
    }
}

pub struct AirGapImporter;

impl AirGapImporter {
    pub const DEFAULT_MAX_FILES: usize = DEFAULT_MAX_FILES;
    pub const DEFAULT_MAX_DECOMPRESSED_BYTES: u64 = DEFAULT_MAX_DECOMPRESSED_BYTES;

    /// Validates whether a relative path is safely contained within its extraction directory
    pub fn is_safe_relative_path(path: &Path) -> bool {
        if path.is_absolute() {
            return false;
        }
        let mut depth: usize = 0;
        for c in path.components() {
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

    /// Imports and verifies a Target C .tar.zst closure archive offline.
    pub fn import_and_verify(
        archive_path: &Path,
        expected_sha256: &str,
        extract_to: &Path,
    ) -> Result<PathBuf, PackagingImportError> {
        info!(
            archive = ?archive_path,
            destination = ?extract_to,
            "Starting air-gap archive import and verification pipeline"
        );

        let mut file = File::open(archive_path).map_err(|e| {
            PackagingImportError::IoError(format!(
                "Failed to open archive at {:?}: {}",
                archive_path, e
            ))
        })?;

        // Cryptographic SHA-256 verification on the open file handle
        let computed_hash = crate::archive::exporter::hashing::calculate_reader_sha256(&mut file)
            .map_err(|e| {
            PackagingImportError::IoError(format!(
                "Failed to read archive for SHA-256 computation: {}",
                e
            ))
        })?;

        if computed_hash != expected_sha256 {
            return Err(PackagingImportError::VerificationFailed(format!(
                "Air-gap checksum verification failed! Expected: {}, Got: {}",
                expected_sha256, computed_hash
            )));
        }

        // Rewind verified file handle for decompression (avoids re-opening from disk)
        file.seek(SeekFrom::Start(0)).map_err(|e| {
            PackagingImportError::IoError(format!("Failed to rewind archive stream: {}", e))
        })?;

        // Ensure target extraction directory exists (idempotent, atomic)
        fs::create_dir_all(extract_to).map_err(|e| {
            PackagingImportError::IoError(format!(
                "Failed to create extraction dir {:?}: {}",
                extract_to, e
            ))
        })?;

        let decoder = zstd::stream::Decoder::new(file).map_err(|e| {
            PackagingImportError::ExtractionError(format!(
                "Zstd decoder initialization failed: {}",
                e
            ))
        })?;
        let mut archive = tar::Archive::new(decoder);

        let mut file_count: usize = 0;
        let mut total_bytes: u64 = 0;

        let entries = archive.entries().map_err(|e| {
            PackagingImportError::ExtractionError(format!(
                "Failed to read tar archive entries: {}",
                e
            ))
        })?;

        for entry_res in entries {
            let mut entry = entry_res.map_err(|e| {
                PackagingImportError::ExtractionError(format!("Corrupt tar entry header: {}", e))
            })?;

            file_count = file_count.saturating_add(1);
            if file_count > Self::DEFAULT_MAX_FILES {
                return Err(PackagingImportError::QuotaExceeded(format!(
                    "Archive exceeds maximum allowed file count ({})",
                    Self::DEFAULT_MAX_FILES
                )));
            }

            if total_bytes.saturating_add(entry.size()) > Self::DEFAULT_MAX_DECOMPRESSED_BYTES {
                return Err(PackagingImportError::QuotaExceeded(format!(
                    "Archive exceeds maximum allowed decompressed size ({} bytes)",
                    Self::DEFAULT_MAX_DECOMPRESSED_BYTES
                )));
            }
            let remaining_quota = Self::DEFAULT_MAX_DECOMPRESSED_BYTES.saturating_sub(total_bytes);

            let entry_path = entry
                .path()
                .map_err(|e| {
                    PackagingImportError::ExtractionError(format!(
                        "Invalid path in archive entry: {}",
                        e
                    ))
                })?
                .into_owned();

            if !Self::is_safe_relative_path(&entry_path) {
                return Err(PackagingImportError::SecurityViolation(format!(
                    "Archive entry contains unsafe path traversal component: {:?}",
                    entry_path
                )));
            }

            // Propagate link name errors explicitly rather than swallowing with if let Ok
            if let Some(link_target) = entry.link_name().map_err(|e| {
                PackagingImportError::ExtractionError(format!(
                    "Failed to read link target for entry {:?}: {}",
                    entry_path, e
                ))
            })? {
                if !Self::is_safe_relative_path(&link_target) {
                    return Err(PackagingImportError::SecurityViolation(format!(
                        "Archive link target escapes extraction directory: {:?}",
                        link_target
                    )));
                }
            }

            if entry.header().entry_type().is_file() {
                let mut current = extract_to.to_path_buf();
                if let Some(parent) = entry_path.parent() {
                    for comp in parent.components() {
                        if let Component::Normal(part) = comp {
                            current.push(part);
                            if let Ok(meta) = fs::symlink_metadata(&current) {
                                if meta.file_type().is_symlink() {
                                    return Err(PackagingImportError::SecurityViolation(format!(
                                        "Symlink traversal detected through intermediate directory {:?}",
                                        current
                                    )));
                                }
                            } else {
                                fs::create_dir_all(&current)?;
                            }
                        }
                    }
                }
                let outpath = extract_to.join(&entry_path);
                if let Ok(meta) = fs::symlink_metadata(&outpath) {
                    if meta.file_type().is_symlink() {
                        fs::remove_file(&outpath)?;
                    }
                }
                let mut outfile = File::create(&outpath)?;
                let mut limited = (&mut entry).take(remaining_quota.saturating_add(1));
                let copied = std::io::copy(&mut limited, &mut outfile)?;
                if copied > remaining_quota {
                    let _ = fs::remove_file(&outpath);
                    return Err(PackagingImportError::QuotaExceeded(format!(
                        "Archive exceeds maximum allowed decompressed size ({} bytes)",
                        Self::DEFAULT_MAX_DECOMPRESSED_BYTES
                    )));
                }
                total_bytes = total_bytes.saturating_add(copied);

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
                    let _ = fs::set_permissions(&outpath, fs::Permissions::from_mode(mode));
                }
                continue;
            }

            // Boundary-checked extraction via unpack_in
            let unpacked = entry.unpack_in(extract_to).map_err(|e| {
                PackagingImportError::ExtractionError(format!(
                    "Failed to unpack entry {:?}: {}",
                    entry_path, e
                ))
            })?;

            if !unpacked {
                return Err(PackagingImportError::SecurityViolation(format!(
                    "Archive entry refused by unpack_in boundary check: {:?}",
                    entry_path
                )));
            }
        }

        // Verify dmz.lock closure signature if embedded in archive
        let lockfile_path = extract_to.join("dmz.lock");
        if lockfile_path.is_file() {
            DmzLockfile::load_and_verify(&lockfile_path).map_err(|e| {
                PackagingImportError::LockfileError(format!("Lockfile verification failed: {}", e))
            })?;
        }

        info!(
            extracted_files = file_count,
            total_bytes = total_bytes,
            "Air-gap archive import and verification completed successfully"
        );

        Ok(extract_to.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_safe_relative_path() {
        assert!(AirGapImporter::is_safe_relative_path(Path::new("foo/bar")));
        assert!(AirGapImporter::is_safe_relative_path(Path::new(
            "./app/src/main.rs"
        )));
        assert!(AirGapImporter::is_safe_relative_path(Path::new(
            "sub/../sub/file"
        )));
        assert!(!AirGapImporter::is_safe_relative_path(Path::new(
            "/etc/shadow"
        )));
        assert!(!AirGapImporter::is_safe_relative_path(Path::new(
            "../escape"
        )));
        assert!(!AirGapImporter::is_safe_relative_path(Path::new(
            "foo/../../escape"
        )));
        assert!(!AirGapImporter::is_safe_relative_path(Path::new("/")));
    }

    #[test]
    fn test_checksum_mismatch_fails_fast() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("dummy.tar.zst");
        fs::write(&archive_path, b"dummy archive content").unwrap();

        let extract_path = dir.path().join("out");
        let result =
            AirGapImporter::import_and_verify(&archive_path, "wrong_sha256_hash", &extract_path);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, PackagingImportError::VerificationFailed(_)));
    }

    #[test]
    fn test_error_display_and_string_conversion() {
        let err = PackagingImportError::SecurityViolation("test traversal".to_string());
        assert_eq!(err.to_string(), "Security Violation: test traversal");

        let err_str: String = err.into();
        assert_eq!(err_str, "Security Violation: test traversal");
    }
}
