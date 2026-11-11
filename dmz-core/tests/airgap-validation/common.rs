use std::fs;
use std::path::{Path, PathBuf};

/// Helper to create mock binary executable files.
pub fn create_mock_bin(dir: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let bin_path = dir.join(name);
    fs::write(&bin_path, "#!/bin/sh\necho mock\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755));
    }
    bin_path
}

/// Helper to link or copy directory recursively across Unix and Windows platforms.
pub fn link_or_copy_dir(src: &Path, dst: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let _ = symlink(src, dst);
    }
    #[cfg(windows)]
    {
        let res = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(dst)
            .arg(src)
            .output();
        if res.is_err() || !res.unwrap().status.success() {
            if std::os::windows::fs::symlink_dir(src, dst).is_err() {
                fn copy_rec(s: &Path, d: &Path) {
                    let _ = fs::create_dir_all(d);
                    if let Ok(entries) = fs::read_dir(s) {
                        for e in entries.flatten() {
                            let p = e.path();
                            let target = d.join(e.file_name());
                            if p.is_dir() {
                                copy_rec(&p, &target);
                            } else {
                                let _ = fs::copy(&p, &target);
                            }
                        }
                    }
                }
                copy_rec(src, dst);
            }
        }
    }
}
