use std::fs;
use std::io;
use std::path::Path;

const MAX_RECURSION_DEPTH: usize = 64;

/// File cloner providing Copy-on-Write (CoW) reflinks
/// (APFS clonefile on macOS, FICLONE on Linux) with hardlink and copy fallbacks.
pub struct FastClone;

impl FastClone {
    /// Safely prepares a destination path by removing any pre-existing file or symlink.
    /// On Windows, clears read-only attributes before unlinking to eliminate `ERROR_ACCESS_DENIED`.
    pub fn prepare_destination(dst: &Path) -> io::Result<()> {
        if dst.exists() || fs::symlink_metadata(dst).is_ok() {
            #[cfg(windows)]
            {
                if let Ok(meta) = fs::metadata(dst) {
                    if meta.permissions().readonly() {
                        let mut perms = meta.permissions();
                        perms.set_readonly(false);
                        let _ = fs::set_permissions(dst, perms);
                    }
                }
            }
            let _ = fs::remove_file(dst);
        }
        Ok(())
    }

    /// Fast-clones a file from `src` to `dst`.
    /// Fallback hierarchy:
    /// OS-native Copy-on-Write reflink (APFS `clonefile` on macOS / Linux `ioctl(FICLONE)`).
    /// `std::fs::hard_link` (instant, zero duplicate disk storage).
    /// `std::fs::copy` (fallback when cross-device, cross-filesystem, or unsupported).
    pub fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
        Self::prepare_destination(dst)?;

        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            if let (Ok(s), Ok(d)) = (
                CString::new(src.as_os_str().as_bytes()),
                CString::new(dst.as_os_str().as_bytes()),
            ) {
                // SAFETY: `s` and `d` are validated non-null, null-terminated CStrings with valid
                // storage lifetimes. `libc::clonefile` creates an atomic APFS copy-on-write clone
                // at the destination path.
                let ret = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), libc::FSOPT_NOFOLLOW) };
                if ret == 0 {
                    return Ok(());
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            if let Ok(src_file) = fs::File::open(src) {
                if let Ok(dst_file) = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(dst)
                {
                    const FICLONE: u64 = 0x40049409;
                    // SAFETY: `dst_file` and `src_file` are valid open file descriptors owned by
                    // live Rust `File` instances. The `ioctl(FICLONE)` request instructs the underlying
                    // filesystem (Btrfs, XFS) to clone extents in kernel space without userspace buffers.
                    let ret = unsafe {
                        libc::ioctl(dst_file.as_raw_fd(), FICLONE as _, src_file.as_raw_fd())
                    };

                    if ret == 0 {
                        return Ok(());
                    }

                    // Should FICLONE fail (e.g. ext4 or tmpfs), drop and delete
                    // the empty 0-byte file so subsequent `hard_link` fallback doesn't fail
                    drop(dst_file);
                    let _ = fs::remove_file(dst);
                }
            }
        }

        // Fallback 1: Hard link (instant, zero duplicate disk storage)
        if fs::hard_link(src, dst).is_ok() {
            let is_store_path = src.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s == "store" || s == ".store" || s == ".dmz"
            });
            if is_store_path {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = fs::metadata(src) {
                        let mut perms = meta.permissions();
                        let mode = perms.mode();
                        perms.set_mode(mode & !0o222);
                        let _ = fs::set_permissions(src, perms);
                    }
                }
                #[cfg(windows)]
                {
                    if let Ok(meta) = fs::metadata(src) {
                        let mut perms = meta.permissions();
                        perms.set_readonly(true);
                        let _ = fs::set_permissions(src, perms);
                    }
                }
            }
            return Ok(());
        }

        // Fallback 2: Standard copy
        fs::copy(src, dst).map(|_| ())
    }

    /// Recursively fast-clones an entire directory tree using `clone_file` for each file.
    pub fn clone_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
        Self::clone_dir_recursive_bounded(src, dst, 0)
    }

    fn clone_dir_recursive_bounded(src: &Path, dst: &Path, depth: usize) -> io::Result<()> {
        if depth > MAX_RECURSION_DEPTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Maximum recursion depth ({}) exceeded during clone",
                    MAX_RECURSION_DEPTH
                ),
            ));
        }

        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)?.flatten() {
            let entry_path = entry.path();
            let file_name = match entry_path.file_name() {
                Some(n) => n,
                None => continue,
            };
            let target_path = dst.join(file_name);
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                Self::clone_dir_recursive_bounded(&entry_path, &target_path, depth + 1)?;
            } else if file_type.is_symlink() {
                #[cfg(unix)]
                {
                    if let Ok(link_target) = fs::read_link(&entry_path) {
                        Self::prepare_destination(&target_path)?;
                        let _ = std::os::unix::fs::symlink(&link_target, &target_path);
                    }
                }
                #[cfg(windows)]
                {
                    if let Ok(link_target) = fs::read_link(&entry_path) {
                        Self::prepare_destination(&target_path)?;
                        let is_dir = entry_path.is_dir();
                        let result = if is_dir {
                            std::os::windows::fs::symlink_dir(&link_target, &target_path)
                        } else {
                            std::os::windows::fs::symlink_file(&link_target, &target_path)
                        };
                        if result.is_err() {
                            let _ = fs::copy(&entry_path, &target_path);
                        }
                    }
                }
            } else {
                Self::clone_file(&entry_path, &target_path)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_clone_file_basic_and_overwrite() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("source.txt");
        let dst = temp.path().join("target.txt");

        fs::write(&src, "initial payload").unwrap();
        FastClone::clone_file(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "initial payload");

        // Test overwrite with modified source
        fs::write(&src, "updated payload").unwrap();
        FastClone::clone_file(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "updated payload");
    }

    #[test]
    fn test_clone_dir_recursive_preserves_tree() {
        let temp = tempdir().unwrap();
        let src_dir = temp.path().join("src_pkg");
        let dst_dir = temp.path().join("dst_pkg");

        fs::create_dir_all(src_dir.join("nested/folder")).unwrap();
        fs::write(src_dir.join("root.json"), "{\"status\": \"ok\"}").unwrap();
        fs::write(src_dir.join("nested/folder/lib.rs"), "pub fn run() {}").unwrap();

        FastClone::clone_dir_recursive(&src_dir, &dst_dir).unwrap();

        assert_eq!(
            fs::read_to_string(dst_dir.join("root.json")).unwrap(),
            "{\"status\": \"ok\"}"
        );
        assert_eq!(
            fs::read_to_string(dst_dir.join("nested/folder/lib.rs")).unwrap(),
            "pub fn run() {}"
        );
    }

    #[test]
    fn test_store_path_immutability_enforced() {
        let temp = tempdir().unwrap();
        let store_dir = temp.path().join(".dmz/store/pkg");
        fs::create_dir_all(&store_dir).unwrap();

        let store_file = store_dir.join("module.txt");
        fs::write(&store_file, "store content").unwrap();

        let dst = temp.path().join("installed/module.txt");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();

        FastClone::clone_file(&store_file, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "store content");
    }
}
