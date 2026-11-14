use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::super::InstallError;

/// Safely normalizes and validates a relative symlink target per Guide Section 4.1.
pub(super) fn is_safe_relative_target(raw_target: &Path) -> bool {
    if raw_target.is_absolute() {
        return false;
    }
    let mut depth: usize = 0;
    for component in raw_target.components() {
        match component {
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

#[cfg(unix)]
pub(super) fn ensure_writable_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::symlink_metadata(dir) {
        let mode = meta.permissions().mode();
        if (mode & 0o700) != 0o700 {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode((mode & 0o777) | 0o700));
        }
    }
}

#[cfg(not(unix))]
pub(super) fn ensure_writable_dir(_dir: &Path) {}

/// Walks `rel_dir` component-by-component under `base_dir`, rejecting any pre-existing
/// symlink directory component (CWE-22) and guaranteeing owner `rwx` (`| 0o700`) so subsequent
/// extractions into read-only directories remain idempotent.
pub(super) fn ensure_safe_dir_components(
    base_dir: &Path,
    rel_dir: &Path,
) -> Result<PathBuf, InstallError> {
    let mut current = base_dir.to_path_buf();
    for comp in rel_dir.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(part) => {
                current.push(part);
                if let Ok(meta) = fs::symlink_metadata(&current) {
                    if meta.file_type().is_symlink() {
                        return Err(InstallError::MapperError(format!(
                            "Symlink traversal detected through intermediate directory {:?}",
                            current
                        )));
                    }
                    if !meta.is_dir() {
                        fs::remove_file(&current).map_err(|e| {
                            InstallError::IoError(format!(
                                "Failed to replace non-directory path {:?}: {}",
                                current, e
                            ))
                        })?;
                        fs::create_dir(&current).map_err(|e| {
                            InstallError::IoError(format!(
                                "Failed to create directory {:?}: {}",
                                current, e
                            ))
                        })?;
                    }
                    ensure_writable_dir(&current);
                } else {
                    fs::create_dir(&current).map_err(|e| {
                        InstallError::IoError(format!(
                            "Failed to create directory {:?}: {}",
                            current, e
                        ))
                    })?;
                }
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(InstallError::MapperError(format!(
                    "Path traversal attempt in archive entry: {:?}",
                    rel_dir
                )));
            }
        }
    }
    Ok(current)
}

/// Removes any existing symlink, directory, or file (including read-only files from prior
/// extractions) before creating a new extracted file inode, avoiding `EACCES`/`EISDIR` and
/// protecting any hardlinked store objects from in-place truncation.
pub(super) fn prepare_overwrite_target(outpath: &Path) -> Result<(), InstallError> {
    let Ok(meta) = fs::symlink_metadata(outpath) else {
        return Ok(());
    };

    if meta.file_type().is_symlink() {
        fs::remove_file(outpath).map_err(|e| {
            InstallError::IoError(format!("Failed to remove symlink {:?}: {}", outpath, e))
        })
    } else if meta.is_dir() {
        ensure_writable_dir(outpath);
        fs::remove_dir_all(outpath).map_err(|e| {
            InstallError::IoError(format!("Failed to replace directory {:?}: {}", outpath, e))
        })
    } else {
        match fs::remove_file(outpath) {
            Ok(()) => Ok(()),
            Err(first_err) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = (meta.permissions().mode() & 0o777) | 0o600;
                    let _ = fs::set_permissions(outpath, fs::Permissions::from_mode(mode));
                }
                #[cfg(not(unix))]
                {
                    let mut perms = meta.permissions();
                    if perms.readonly() {
                        perms.set_readonly(false);
                        let _ = fs::set_permissions(outpath, perms);
                    }
                }
                fs::remove_file(outpath).map_err(|e| {
                    InstallError::IoError(format!(
                        "Failed to remove existing file {:?} ({}; retry: {})",
                        outpath, first_err, e
                    ))
                })
            }
        }
    }
}

/// Normalizes extracted file permissions: falls back to `0o644` when an archive stores mode `0`
/// (e.g., Windows-built wheels) and preserves owner `rw` (`| 0o600`) for idempotent rebuilds.
pub(super) fn apply_extracted_file_permissions(outpath: &Path, raw_mode: Option<u32>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm_bits = raw_mode
            .map(|m| m & 0o777)
            .filter(|&m| m != 0)
            .unwrap_or(0o644);
        let sanitized = perm_bits | 0o600;
        let _ = fs::set_permissions(outpath, fs::Permissions::from_mode(sanitized));
    }
    #[cfg(not(unix))]
    let _ = (outpath, raw_mode);
}

/// Validates a relative symlink target and materializes it after clearing any existing target.
pub(super) fn create_validated_symlink(target: &str, outpath: &Path) -> Result<(), InstallError> {
    if !is_safe_relative_target(Path::new(target)) {
        return Err(InstallError::MapperError(format!(
            "Path traversal attempt in archive link target: {:?}",
            target
        )));
    }
    prepare_overwrite_target(outpath)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, outpath).map_err(|e| {
        InstallError::IoError(format!("Failed to create symlink {:?}: {}", outpath, e))
    })?;
    #[cfg(not(unix))]
    let _ = target;
    Ok(())
}

/// Streams up to `remaining_quota` bytes into a freshly created `outpath`, enforcing the
/// decompression quota and normalizing extracted permissions.
pub(super) fn stream_bounded_to_file<R: Read>(
    reader: R,
    outpath: &Path,
    remaining_quota: u64,
    total_bytes: &mut u64,
    raw_mode: Option<u32>,
    bomb_label: &str,
) -> Result<(), InstallError> {
    prepare_overwrite_target(outpath)?;

    let mut outfile = File::create(outpath).map_err(|e| {
        InstallError::IoError(format!("Failed to create file {:?}: {}", outpath, e))
    })?;

    let mut limited_reader = reader.take(remaining_quota.saturating_add(1));
    let copied = std::io::copy(&mut limited_reader, &mut outfile).map_err(|e| {
        InstallError::IoError(format!("Failed to extract file {:?}: {}", outpath, e))
    })?;

    if copied > remaining_quota {
        let _ = fs::remove_file(outpath);
        return Err(InstallError::MapperError(format!(
            "Archive decompression quota exceeded ({} bomb protection)",
            bomb_label
        )));
    }
    *total_bytes = total_bytes.saturating_add(copied);

    apply_extracted_file_permissions(outpath, raw_mode);
    Ok(())
}
