use ::tar::{Archive as TarArchive, Entry};
use std::fs;
use std::io::Read;
use std::path::{Component, Path};

use super::super::InstallError;
use super::fs_safety;
use super::SafeArchiveExtractor;

/// Extracts entries from an open tar stream with ZipSlip defense, quota limits, and idempotent overwrites.
pub(super) fn extract_tar_stream<R: Read>(
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
        let mut entry = entry_res
            .map_err(|e| InstallError::MapperError(format!("Corrupt tar archive entry: {}", e)))?;

        file_count = file_count.saturating_add(1);
        if file_count > SafeArchiveExtractor::DEFAULT_MAX_FILES {
            return Err(InstallError::MapperError(format!(
                "Archive exceeds maximum file count limit ({})",
                SafeArchiveExtractor::DEFAULT_MAX_FILES
            )));
        }

        extract_tar_entry(&mut entry, target_dir, &mut total_bytes)?;
    }

    Ok(())
}

fn extract_tar_entry<R: Read>(
    entry: &mut Entry<'_, R>,
    target_dir: &Path,
    total_bytes: &mut u64,
) -> Result<(), InstallError> {
    if total_bytes.saturating_add(entry.size()) > SafeArchiveExtractor::DEFAULT_MAX_BYTES {
        return Err(InstallError::MapperError(
            "Archive decompression quota exceeded (tar bomb protection)".into(),
        ));
    }
    let remaining_quota = SafeArchiveExtractor::DEFAULT_MAX_BYTES.saturating_sub(*total_bytes);

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
        if !fs_safety::is_safe_relative_target(&link_target) {
            return Err(InstallError::MapperError(format!(
                "Path traversal attempt in archive link target: {:?}",
                link_target
            )));
        }
    }

    if entry.header().entry_type().is_file() {
        let safe_parent = if let Some(parent) = entry_path.parent() {
            fs_safety::ensure_safe_dir_components(target_dir, parent)?
        } else {
            target_dir.to_path_buf()
        };
        let Some(file_name) = entry_path.file_name() else {
            return Err(InstallError::MapperError(format!(
                "Invalid empty file name in tar entry: {:?}",
                entry_path
            )));
        };
        let outpath = safe_parent.join(file_name);
        let raw_mode = entry.header().mode().ok();
        return fs_safety::stream_bounded_to_file(
            entry,
            &outpath,
            remaining_quota,
            total_bytes,
            raw_mode,
            "tar",
        );
    }

    if entry.header().entry_type().is_dir() {
        fs_safety::ensure_safe_dir_components(target_dir, &entry_path)?;
        return Ok(());
    }

    if let Some(parent) = entry_path.parent() {
        fs_safety::ensure_safe_dir_components(target_dir, parent)?;
    }
    fs_safety::prepare_overwrite_target(&target_dir.join(&entry_path))?;

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

    Ok(())
}
