use ::zip::read::ZipFile;
use ::zip::ZipArchive;
use std::fs;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use super::super::InstallError;
use super::fs_safety;
use super::SafeArchiveExtractor;

const MAX_SYMLINK_TARGET_BYTES: u64 = 4096;

/// ZIP extraction from an open reader with bounded streaming decompression and ZipSlip defense.
pub(super) fn extract_zip_from_reader<R: Read + Seek>(
    reader: R,
    target_dir: &Path,
) -> Result<(), InstallError> {
    let buf_reader = BufReader::with_capacity(64 * 1024, reader);
    let mut archive = ZipArchive::new(buf_reader)
        .map_err(|e| InstallError::MapperError(format!("Failed to parse zip archive: {}", e)))?;

    if archive.len() > SafeArchiveExtractor::DEFAULT_MAX_FILES {
        return Err(InstallError::MapperError(format!(
            "Archive exceeds maximum file count limit ({})",
            SafeArchiveExtractor::DEFAULT_MAX_FILES
        )));
    }

    fs::create_dir_all(target_dir).map_err(|e| {
        InstallError::IoError(format!("Failed to create destination directory: {}", e))
    })?;
    let canonical_target =
        fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());

    let mut total_bytes: u64 = 0;
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| {
            InstallError::MapperError(format!("Corrupt zip entry at index {}: {}", i, e))
        })?;
        extract_zip_entry(file, &canonical_target, &mut total_bytes)?;
    }

    Ok(())
}

fn extract_zip_entry<R: Read>(
    mut file: ZipFile<'_, R>,
    canonical_target: &Path,
    total_bytes: &mut u64,
) -> Result<(), InstallError> {
    // ZipSlip defense: enclosed_name returns None if path traverses outside target
    let Some(enclosed) = file.enclosed_name() else {
        return Err(InstallError::MapperError(format!(
            "Path traversal attempt in archive entry: {:?}",
            file.name()
        )));
    };

    if file.is_dir() {
        fs_safety::ensure_safe_dir_components(canonical_target, &enclosed)?;
        return Ok(());
    }

    let safe_parent = if let Some(parent) = enclosed.parent() {
        fs_safety::ensure_safe_dir_components(canonical_target, parent)?
    } else {
        canonical_target.to_path_buf()
    };
    let Some(file_name) = enclosed.file_name() else {
        return Err(InstallError::MapperError(format!(
            "Invalid empty file name in archive entry: {:?}",
            file.name()
        )));
    };
    let outpath = safe_parent.join(file_name);

    let raw_mode = file.unix_mode();
    let is_symlink = raw_mode.is_some_and(|m| (m & 0o170000) == 0o120000);
    if is_symlink {
        return extract_zip_symlink(&mut file, &enclosed, &outpath);
    }

    let remaining_quota = SafeArchiveExtractor::DEFAULT_MAX_BYTES.saturating_sub(*total_bytes);
    fs_safety::stream_bounded_to_file(
        &mut file,
        &outpath,
        remaining_quota,
        total_bytes,
        raw_mode,
        "zip",
    )
}

fn extract_zip_symlink<R: Read>(
    file: &mut ZipFile<'_, R>,
    enclosed: &Path,
    outpath: &Path,
) -> Result<(), InstallError> {
    let mut target_buf = Vec::new();
    file.take(MAX_SYMLINK_TARGET_BYTES.saturating_add(1))
        .read_to_end(&mut target_buf)
        .map_err(|e| {
            InstallError::IoError(format!(
                "Failed to read zip symlink target {:?}: {}",
                enclosed, e
            ))
        })?;
    if target_buf.len() > MAX_SYMLINK_TARGET_BYTES as usize {
        return Err(InstallError::MapperError(format!(
            "Zip symlink target exceeds maximum length: {:?}",
            enclosed
        )));
    }
    let target_str = String::from_utf8(target_buf).map_err(|_| {
        InstallError::MapperError(format!(
            "Non-UTF8 zip symlink target in entry: {:?}",
            enclosed
        ))
    })?;
    fs_safety::create_validated_symlink(&target_str, outpath)
}
