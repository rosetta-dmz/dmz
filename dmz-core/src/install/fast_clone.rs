use std::fs;
use std::io;
use std::path::Path;

/// High-performance file cloner providing Copy-on-Write (CoW) reflinks
/// (APFS clonefile on macOS, FICLONE on Linux) with hardlink and copy fallbacks.
pub struct FastClone;

impl FastClone {
    /// Fast-clones a file from `src` to `dst`.
    /// 1. Tries OS-native Copy-on-Write reflink (APFS `clonefile` / Linux `ioctl(FICLONE)`).
    /// 2. If unsupported, tries `std::fs::hard_link`.
    /// 3. If hard_link fails (cross-device/permissions), falls back to `std::fs::copy`.
    pub fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
        if dst.exists() || fs::symlink_metadata(dst).is_ok() {
            let _ = fs::remove_file(dst);
        }

        #[cfg(target_os = "macos")]
        {
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;

            if let (Ok(s), Ok(d)) = (
                CString::new(src.as_os_str().as_bytes()),
                CString::new(dst.as_os_str().as_bytes()),
            ) {
                let ret = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), libc::FSOPT_NOFOLLOW) };
                if ret == 0 {
                    return Ok(());
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            if let (Ok(src_file), Ok(dst_file)) = (
                fs::File::open(src),
                fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(dst),
            ) {
<<<<<<< Updated upstream
                const FICLONE: libc::c_ulong = 0x40049409;
                let ret =
                    unsafe { libc::ioctl(dst_file.as_raw_fd(), FICLONE, src_file.as_raw_fd()) };
=======
                // Linux ioctl request type varies across libc implementations:
                // `c_ulong` (u64) on glibc, `c_int` (i32) on musl/Android.
                const FICLONE: u64 = 0x40049409;
                let ret = unsafe {
                    libc::ioctl(dst_file.as_raw_fd(), FICLONE as _, src_file.as_raw_fd())
                };
>>>>>>> Stashed changes
                if ret == 0 {
                    return Ok(());
                }
            }
        }

        // Fallback 1: Hard link (instant, zero duplicate disk storage)
        if fs::hard_link(src, dst).is_ok() {
            return Ok(());
        }

        // Fallback 2: Standard copy
        fs::copy(src, dst).map(|_| ())
    }

    /// Recursively fast-clones an entire directory tree using `clone_file` for each file.
    pub fn clone_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
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
                Self::clone_dir_recursive(&entry_path, &target_path)?;
            } else if file_type.is_symlink() {
                #[cfg(unix)]
                {
                    if let Ok(link_target) = fs::read_link(&entry_path) {
                        if target_path.exists() || fs::symlink_metadata(&target_path).is_ok() {
                            let _ = fs::remove_file(&target_path);
                        }
                        let _ = std::os::unix::fs::symlink(&link_target, &target_path);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = fs::copy(&entry_path, &target_path);
                }
            } else {
                Self::clone_file(&entry_path, &target_path)?;
            }
        }
        Ok(())
    }
}
