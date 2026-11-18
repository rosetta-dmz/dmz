mod fs_safety;
mod tar;
mod zip;

#[cfg(test)]
mod tests;

use ::tar::Archive as TarArchive;
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::InstallError;

/// Format of the package archive detected via magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    Tar,
}

/// Polyglot archive extractor with ZipSlip defense, permission normalization, and quota guards.
pub struct SafeArchiveExtractor;

impl SafeArchiveExtractor {
    pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB cap per package
    pub const DEFAULT_MAX_FILES: usize = 50_000;

    /// Sniffs magic bytes from an open readable stream and rewinds to the beginning.
    pub fn detect_format_from_reader<R: Read + Seek>(
        reader: &mut R,
    ) -> std::io::Result<ArchiveFormat> {
        let mut magic = [0u8; 4];
        let bytes_read = reader.read(&mut magic)?;
        reader.seek(SeekFrom::Start(0))?;

        match (bytes_read, magic.get(0..4), magic.get(0..2)) {
            (4.., Some(b"PK\x03\x04"), _) => Ok(ArchiveFormat::Zip),
            (2.., _, Some([0x1F, 0x8B])) => Ok(ArchiveFormat::TarGz),
            (2.., _, Some(b"PK")) => Ok(ArchiveFormat::Zip),
            _ => Ok(ArchiveFormat::Tar),
        }
    }

    /// Sniffs magic bytes to determine the archive format reliably regardless of file extension.
    pub fn detect_format(archive_path: &Path) -> std::io::Result<ArchiveFormat> {
        let mut file = File::open(archive_path)?;
        Self::detect_format_from_reader(&mut file)
    }

    /// Safely normalizes and validates a relative symlink target per Guide Section 4.1.
    pub fn is_safe_relative_target(raw_target: &Path) -> bool {
        fs_safety::is_safe_relative_target(raw_target)
    }

    /// Automatically sniffs format and safely extracts the archive to `target_dir` using a single file handle.
    pub fn extract(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let mut file = File::open(archive_path).map_err(|e| {
            InstallError::IoError(format!(
                "Failed to open archive at {:?}: {}",
                archive_path, e
            ))
        })?;

        let format = Self::detect_format_from_reader(&mut file).map_err(|e| {
            InstallError::IoError(format!("Failed to inspect archive format: {}", e))
        })?;

        match format {
            ArchiveFormat::Zip => zip::extract_zip_from_reader(file, target_dir),
            ArchiveFormat::TarGz => {
                let buf_reader = BufReader::with_capacity(64 * 1024, file);
                let decompressed = GzDecoder::new(buf_reader);
                let archive = TarArchive::new(decompressed);
                tar::extract_tar_stream(archive, target_dir)
            }
            ArchiveFormat::Tar => {
                let buf_reader = BufReader::with_capacity(64 * 1024, file);
                let archive = TarArchive::new(buf_reader);
                tar::extract_tar_stream(archive, target_dir)
            }
        }
    }

    /// ZIP extraction with ZipSlip path sanitation, permission normalization,
    /// and decompression bomb protection.
    pub fn extract_zip(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let file = File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open zip archive: {}", e)))?;
        zip::extract_zip_from_reader(file, target_dir)
    }

    /// Tar.Gz extraction with ZipSlip defense and decompression quotas.
    pub fn extract_tar_gz(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let file = File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open tar.gz archive: {}", e)))?;
        let buf_reader = BufReader::with_capacity(64 * 1024, file);
        let decompressed = GzDecoder::new(buf_reader);
        let archive = TarArchive::new(decompressed);
        tar::extract_tar_stream(archive, target_dir)
    }

    /// Tar extraction with ZipSlip defense and decompression quotas.
    pub fn extract_tar(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let file = File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open tar archive: {}", e)))?;
        let buf_reader = BufReader::with_capacity(64 * 1024, file);
        let archive = TarArchive::new(buf_reader);
        tar::extract_tar_stream(archive, target_dir)
    }
}
