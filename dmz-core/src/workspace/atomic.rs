// Atomic filesystem persistence utilities.
// Provides transactional, crash-consistent file writing using temporary files,
// synchronous data flushing, optional POSIX permission configuration,
// and atomic renames.

use std::fs;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Default file permissions for standard workspace files (read/write for owner, read for group/others).
pub const DEFAULT_FILE_MODE: u32 = 0o644;

pub fn atomic_write_file(path: &Path, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    atomic_write_file_with_mode(path, content, DEFAULT_FILE_MODE)
}

pub fn atomic_write_file_with_mode(
    path: &Path,
    content: impl AsRef<[u8]>,
    #[allow(unused_variables)] mode: u32,
) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent)?;

    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;

    #[cfg(unix)]
    {
        let permissions = fs::Permissions::from_mode(mode);
        temp_file.as_file().set_permissions(permissions)?;
    }

    temp_file.write_all(content.as_ref())?;
    temp_file.flush()?;
    temp_file.as_file().sync_all()?;
    temp_file.persist(path).map_err(|e| e.error)?;

    // On POSIX filesystems, sync the parent directory to persist the directory entry update.
    // If the directory fsync fails (e.g. virtual or network filesystem), trace rather than silently discarding.
    #[cfg(unix)]
    match fs::File::open(parent) {
        Ok(dir) => {
            if let Err(err) = dir.sync_all() {
                tracing::trace!(parent = ?parent, error = %err, "Parent directory sync skipped or unsupported");
            }
        }
        Err(err) => {
            tracing::trace!(parent = ?parent, error = %err, "Parent directory open for sync skipped or unsupported");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use tempfile::tempdir;

    #[test]
    fn test_atomic_write_file_basic_and_overwrite() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        let target = dir.path().join("atomic_test.txt");

        atomic_write_file(&target, b"first content")?;
        assert_eq!(fs::read_to_string(&target)?, "first content");

        atomic_write_file(&target, b"second content")?;
        assert_eq!(fs::read_to_string(&target)?, "second content");
        Ok(())
    }

    #[test]
    fn test_atomic_write_file_creates_nested_parent_dirs() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        let target = dir.path().join("a").join("b").join("c").join("nested.txt");

        atomic_write_file(&target, b"deep nested payload")?;
        assert_eq!(fs::read_to_string(&target)?, "deep nested payload");
        Ok(())
    }

    #[test]
    fn test_atomic_write_file_empty_content() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        let target = dir.path().join("empty.txt");

        atomic_write_file(&target, b"")?;
        assert_eq!(fs::read_to_string(&target)?, "");
        Ok(())
    }

    #[test]
    fn test_atomic_write_file_bare_path() -> Result<(), Box<dyn Error>> {
        struct FileGuard<'a>(&'a Path);
        impl<'a> Drop for FileGuard<'a> {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0);
            }
        }

        let target = Path::new("test_atomic_write_file_bare_target_tmp.txt");
        let _guard = FileGuard(target);

        atomic_write_file(target, b"bare target content")?;
        assert_eq!(fs::read_to_string(target)?, "bare target content");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_atomic_write_file_with_custom_mode() -> Result<(), Box<dyn Error>> {
        let dir = tempdir()?;
        let target = dir.path().join("script.sh");

        atomic_write_file_with_mode(&target, b"#!/bin/sh\necho hello\n", 0o755)?;
        let metadata = fs::metadata(&target)?;
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        Ok(())
    }
}
