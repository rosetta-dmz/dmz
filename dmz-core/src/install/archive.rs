use flate2::read::GzDecoder;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Component, Path};
use tar::Archive as TarArchive;
use zip::ZipArchive;

use super::InstallError;

/// Format of the package archive detected via magic bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    Tar,
}

/// Polyglot archive extractor with zero-trust protection
/// against ZipSlip path traversal and decompression resource exhaustion.
pub struct SafeArchiveExtractor;

impl SafeArchiveExtractor {
    pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB cap per package
    pub const DEFAULT_MAX_FILES: usize = 50_000;

    /// Sniffs magic bytes to determine the archive format reliably regardless of file extension
    pub fn detect_format(archive_path: &Path) -> std::io::Result<ArchiveFormat> {
        let mut file = fs::File::open(archive_path)?;
        let mut magic = [0u8; 4];
        let bytes_read = file.read(&mut magic)?;

        if bytes_read >= 4 && &magic[0..4] == b"PK\x03\x04" {
            Ok(ArchiveFormat::Zip)
        } else if bytes_read >= 2 && magic[0] == 0x1F && magic[1] == 0x8B {
            Ok(ArchiveFormat::TarGz)
        } else if bytes_read >= 2 && &magic[0..2] == b"PK" {
            // Other valid zip headers (e.g. empty zip PK\x05\x06)
            Ok(ArchiveFormat::Zip)
        } else {
            Ok(ArchiveFormat::Tar)
        }
    }

    /// Automatically sniffs format and safely extracts the archive to target_dir
    pub fn extract(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let format = Self::detect_format(archive_path).map_err(|e| {
            InstallError::IoError(format!("Failed to inspect archive format: {}", e))
        })?;

        match format {
            ArchiveFormat::Zip => Self::extract_zip(archive_path, target_dir),
            ArchiveFormat::TarGz => Self::extract_tar_gz(archive_path, target_dir),
            ArchiveFormat::Tar => Self::extract_tar(archive_path, target_dir),
        }
    }

    /// Hardened ZIP extraction with ZipSlip path sanitation, permission normalization,
    /// and decompression bomb protection.
    pub fn extract_zip(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let file = fs::File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open zip archive: {}", e)))?;
        let buf_reader = BufReader::with_capacity(64 * 1024, file);

        let mut archive = ZipArchive::new(buf_reader).map_err(|e| {
            InstallError::MapperError(format!("Failed to parse zip archive: {}", e))
        })?;

        if archive.len() > Self::DEFAULT_MAX_FILES {
            return Err(InstallError::MapperError(format!(
                "Archive exceeds maximum file count limit ({})",
                Self::DEFAULT_MAX_FILES
            )));
        }

        fs::create_dir_all(target_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create destination directory: {}", e))
        })?;
        let canonical_target =
            fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());

        let mut total_bytes: u64 = 0;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| {
                InstallError::MapperError(format!("Corrupt zip entry at index {}: {}", i, e))
            })?;

            // ZipSlip defense: enclosed_name returns None if path traverses outside target
            let enclosed = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => {
                    return Err(InstallError::MapperError(format!(
                        "Path traversal attempt in archive entry: {:?}",
                        file.name()
                    )));
                }
            };

            let outpath = canonical_target.join(&enclosed);

            total_bytes = total_bytes.saturating_add(file.size());
            if total_bytes > Self::DEFAULT_MAX_BYTES {
                return Err(InstallError::MapperError(
                    "Archive decompression quota exceeded (zip bomb protection)".into(),
                ));
            }

            if file.is_dir() {
                fs::create_dir_all(&outpath).map_err(|e| {
                    InstallError::IoError(format!(
                        "Failed to create directory {:?}: {}",
                        outpath, e
                    ))
                })?;
            } else {
                if let Some(parent) = outpath.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).map_err(|e| {
                            InstallError::IoError(format!(
                                "Failed to create parent {:?}: {}",
                                parent, e
                            ))
                        })?;
                    }
                }

                let mut outfile = fs::File::create(&outpath).map_err(|e| {
                    InstallError::IoError(format!("Failed to create file {:?}: {}", outpath, e))
                })?;

                std::io::copy(&mut file, &mut outfile).map_err(|e| {
                    InstallError::IoError(format!("Failed to extract file {:?}: {}", outpath, e))
                })?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = file.unix_mode().unwrap_or(0o644) & 0o777;
                    let _ = fs::set_permissions(&outpath, fs::Permissions::from_mode(mode));
                }
            }
        }

        Ok(())
    }

    /// Hardened Tar.Gz extraction with ZipSlip defense and decompression quotas
    pub fn extract_tar_gz(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let file = fs::File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open tar.gz archive: {}", e)))?;
        let buf_reader = BufReader::with_capacity(64 * 1024, file);
        let decompressed = GzDecoder::new(buf_reader);
        let archive = TarArchive::new(decompressed);
        Self::extract_tar_stream(archive, target_dir)
    }

    /// Hardened Tar extraction with ZipSlip defense and decompression quotas
    pub fn extract_tar(archive_path: &Path, target_dir: &Path) -> Result<(), InstallError> {
        let file = fs::File::open(archive_path)
            .map_err(|e| InstallError::IoError(format!("Failed to open tar archive: {}", e)))?;
        let buf_reader = BufReader::with_capacity(64 * 1024, file);
        let archive = TarArchive::new(buf_reader);
        Self::extract_tar_stream(archive, target_dir)
    }

    fn extract_tar_stream<R: Read>(
        mut archive: TarArchive<R>,
        target_dir: &Path,
    ) -> Result<(), InstallError> {
        fs::create_dir_all(target_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create destination directory: {}", e))
        })?;

        let mut file_count: usize = 0;
        let mut total_bytes: u64 = 0;

        let entries = archive.entries().map_err(|e| {
            InstallError::MapperError(format!("Failed to read tar archive entries: {}", e))
        })?;

        for entry_res in entries {
            let mut entry = entry_res.map_err(|e| {
                InstallError::MapperError(format!("Corrupt tar archive entry: {}", e))
            })?;

            file_count += 1;
            if file_count > Self::DEFAULT_MAX_FILES {
                return Err(InstallError::MapperError(format!(
                    "Archive exceeds maximum file count limit ({})",
                    Self::DEFAULT_MAX_FILES
                )));
            }

            total_bytes = total_bytes.saturating_add(entry.size());
            if total_bytes > Self::DEFAULT_MAX_BYTES {
                return Err(InstallError::MapperError(
                    "Archive decompression quota exceeded (tar bomb protection)".into(),
                ));
            }

            let entry_path = entry
                .path()
                .map_err(|e| InstallError::MapperError(format!("Invalid tar entry path: {}", e)))?
                .into_owned();

            if entry_path.is_absolute()
                || entry_path.components().any(|c| {
                    matches!(
                        c,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                return Err(InstallError::MapperError(format!(
                    "Path traversal attempt in archive entry: {:?}",
                    entry_path
                )));
            }

            if let Some(link_target) = entry
                .link_name()
                .map_err(|e| InstallError::MapperError(format!("Invalid tar link target: {}", e)))?
            {
                if link_target.is_absolute()
                    || link_target.components().any(|c| {
                        matches!(
                            c,
                            Component::ParentDir | Component::RootDir | Component::Prefix(_)
                        )
                    })
                {
                    return Err(InstallError::MapperError(format!(
                        "Path traversal attempt in archive link target: {:?}",
                        link_target
                    )));
                }
            }

            let unpacked = entry.unpack_in(target_dir).map_err(|e| {
                InstallError::MapperError(format!(
                    "Failed to unpack tar entry {:?}: {}",
                    entry_path, e
                ))
            })?;

            if !unpacked {
                return Err(InstallError::MapperError(format!(
                    "Path traversal attempt in archive entry: {:?}",
                    entry_path
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    #[test]
    fn test_detect_format_zip_and_tar_gz() {
        let temp = tempdir().unwrap();
        let zip_path = temp.path().join("test.zip");
        let targz_path = temp.path().join("test.tar.gz");

        // Write small zip
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut writer = ZipWriter::new(file);
            writer
                .start_file("hello.txt", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"world").unwrap();
            writer.finish().unwrap();
        }

        // Write small tar.gz
        {
            let file = fs::File::create(&targz_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar_builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_cksum();
            tar_builder
                .append_data(&mut header, "hello.txt", &b"world"[..])
                .unwrap();
            tar_builder.into_inner().unwrap().finish().unwrap();
        }

        assert_eq!(
            SafeArchiveExtractor::detect_format(&zip_path).unwrap(),
            ArchiveFormat::Zip
        );
        assert_eq!(
            SafeArchiveExtractor::detect_format(&targz_path).unwrap(),
            ArchiveFormat::TarGz
        );
    }

    #[test]
    fn test_extract_zip_success() {
        let temp = tempdir().unwrap();
        let zip_path = temp.path().join("test.zip");
        let extract_dir = temp.path().join("extracted");

        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut writer = ZipWriter::new(file);
            writer
                .start_file("pkg/mod.py", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"print('hello')").unwrap();
            writer.finish().unwrap();
        }

        SafeArchiveExtractor::extract(&zip_path, &extract_dir).unwrap();
        assert!(extract_dir.join("pkg/mod.py").is_file());
        assert_eq!(
            fs::read_to_string(extract_dir.join("pkg/mod.py")).unwrap(),
            "print('hello')"
        );
    }

    #[test]
    fn test_zipslip_rejection() {
        let temp = tempdir().unwrap();
        let evil_zip = temp.path().join("evil.zip");
        let extract_dir = temp.path().join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();

        {
            let file = fs::File::create(&evil_zip).unwrap();
            let mut writer = ZipWriter::new(file);
            // Intentionally create path traversal entry
            writer
                .start_file("../outside.txt", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"malicious").unwrap();
            writer.finish().unwrap();
        }

        let result = SafeArchiveExtractor::extract(&evil_zip, &extract_dir);
        assert!(result.is_err(), "ZipSlip attempt must fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Path traversal attempt"),
            "Error must mention path traversal: {}",
            err_msg
        );
        assert!(
            !temp.path().join("outside.txt").exists(),
            "File outside target directory must not be created"
        );
    }

    #[test]
    fn test_tar_zipslip_rejection() {
        let temp = tempdir().unwrap();
        let evil_tar_gz = temp.path().join("evil.tar.gz");
        let extract_dir = temp.path().join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();

        {
            let file = fs::File::create(&evil_tar_gz).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar_builder = tar::Builder::new(encoder);
            let payload = b"malicious";
            let mut header = tar::Header::new_gnu();
            let raw_path = b"../outside_tar.txt";
            header.as_gnu_mut().unwrap().name[..raw_path.len()].copy_from_slice(raw_path);
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar_builder.append(&header, &payload[..]).unwrap();
            tar_builder.into_inner().unwrap().finish().unwrap();
        }

        let result = SafeArchiveExtractor::extract(&evil_tar_gz, &extract_dir);
        assert!(result.is_err(), "Tar ZipSlip attempt must fail");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Path traversal attempt"),
            "Error must mention path traversal: {}",
            err_msg
        );
        assert!(
            !temp.path().join("outside_tar.txt").exists(),
            "File outside target directory must not be created"
        );
    }
}
