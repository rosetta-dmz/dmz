// Binary RPATH relocation and Mach-O / ELF code signing

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

/// Relocate bundled binaries and shared libraries using relative runtime search paths (RPATH).
/// On macOS, uses `install_name_tool` to inject `@loader_path/../lib` and `@executable_path/../lib`,
/// followed by ad-hoc codesigning (`codesign --force --sign -`) to satisfy Apple Silicon Gatekeeper.
/// On Linux, uses `patchelf` to inject `$ORIGIN/../lib`.
pub fn relocate_bundled_binaries(workspace_root: &Path) -> Result<(), String> {
    let toolchains_dir = workspace_root.join(".dmz/toolchains");
    if !toolchains_dir.is_dir() {
        return Ok(());
    }

    fn walk_and_relocate(dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_and_relocate(&p);
            } else if p.is_file() {
                // Skip symlinks
                if fs::symlink_metadata(&p)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    continue;
                }

                // Check magic bytes to identify Mach-O / ELF binaries
                let mut magic = [0u8; 4];
                let is_binary = if let Ok(mut f) = File::open(&p) {
                    f.read_exact(&mut magic).is_ok()
                } else {
                    false
                };

                if !is_binary {
                    continue;
                }

                #[cfg(target_os = "macos")]
                {
                    let is_macho = matches!(
                        magic,
                        [0xfe, 0xed, 0xfa, 0xcf]
                            | [0xcf, 0xfa, 0xed, 0xfe]
                            | [0xfe, 0xed, 0xfa, 0xce]
                            | [0xce, 0xfa, 0xed, 0xfe]
                            | [0xca, 0xfe, 0xba, 0xbe]
                            | [0xbe, 0xba, 0xfe, 0xca]
                    );

                    if is_macho {
                        let _ = Command::new("install_name_tool")
                            .arg("-add_rpath")
                            .arg("@loader_path/../lib")
                            .arg(&p)
                            .output();
                        let _ = Command::new("install_name_tool")
                            .arg("-add_rpath")
                            .arg("@executable_path/../lib")
                            .arg(&p)
                            .output();

                        // Nix `fixDarwinDylibNames` pattern:
                        // 1. If this file is a shared library in `lib/`, set its install_name id to `@rpath/<name>`.
                        if p.parent()
                            .and_then(|parent| parent.file_name())
                            .is_some_and(|n| n == "lib")
                        {
                            if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
                                let _ = Command::new("install_name_tool")
                                    .arg("-id")
                                    .arg(format!("@rpath/{}", fname))
                                    .arg(&p)
                                    .output();
                            }
                        }

                        // 2. Inspect Mach-O load commands (`otool -L`) and rewrite absolute non-system dylibs
                        // that exist in the bundled toolchain's `lib/` directory to `@rpath/<lib_name>`.
                        if let Ok(otool_out) = Command::new("otool").arg("-L").arg(&p).output()
                        {
                            if otool_out.status.success() {
                                let stdout = String::from_utf8_lossy(&otool_out.stdout);
                                for line in stdout.lines().skip(1) {
                                    let trimmed = line.trim();
                                    let dep_path = trimmed
                                        .split(" (compatibility version")
                                        .next()
                                        .unwrap_or("")
                                        .trim();
                                    if dep_path.starts_with('/')
                                        && !dep_path.starts_with("/usr/lib/")
                                        && !dep_path.starts_with("/System/Library/")
                                    {
                                        if let Some(dep_name) = Path::new(dep_path)
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                        {
                                            let lib_dir_opt = if p
                                                .parent()
                                                .and_then(|parent| parent.file_name())
                                                .is_some_and(|n| n == "lib")
                                            {
                                                p.parent().map(Path::to_path_buf)
                                            } else {
                                                p.parent()
                                                    .and_then(|d| d.parent())
                                                    .map(|root| root.join("lib"))
                                            };
                                            // Perform an exact case-sensitive filename match inside `lib/`
                                            // so case-insensitive filesystems (APFS/NTFS) never confuse
                                            // `bin/python` with `lib/Python`.
                                            let has_bundled_dep = lib_dir_opt
                                                .as_ref()
                                                .and_then(|dir| fs::read_dir(dir).ok())
                                                .is_some_and(|entries| {
                                                    entries.flatten().any(|e| {
                                                        e.file_name().to_str() == Some(dep_name)
                                                            && e.path().is_file()
                                                    })
                                                });
                                            if has_bundled_dep {
                                                let _ = Command::new("install_name_tool")
                                                    .args([
                                                        "-change",
                                                        dep_path,
                                                        &format!("@rpath/{}", dep_name),
                                                    ])
                                                    .arg(&p)
                                                    .output();
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let _ = Command::new("codesign")
                            .args(["--force", "--sign", "-", p.to_str().unwrap_or_default()])
                            .output();
                    }
                }

                #[cfg(target_os = "linux")]
                {
                    let is_elf = magic == *b"\x7fELF";
                    if is_elf {
                        let _ = Command::new("patchelf")
                            .arg("--set-rpath")
                            .arg("$ORIGIN/../lib")
                            .arg(&p)
                            .output();
                    }
                }
            }
        }
    }

    walk_and_relocate(&toolchains_dir);
    Ok(())
}

