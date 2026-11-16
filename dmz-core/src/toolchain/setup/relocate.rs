// Binary RPATH relocation and Mach-O / ELF code signing

use super::super::ToolchainError;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

#[cfg(target_os = "macos")]
fn resolve_non_stub_install_name_tool(toolchains_dir: &Path) -> Option<std::path::PathBuf> {
    if let Ok(entries) = fs::read_dir(toolchains_dir) {
        for entry in entries.flatten() {
            let bundled = entry.path().join("bin/install_name_tool");
            if bundled.is_file() {
                return Some(bundled);
            }
        }
    }
    crate::toolchain::preflight::resolve_real_apple_developer_tool("install_name_tool")
}

/// Relocate bundled binaries and shared libraries using relative runtime search paths (RPATH).
/// On macOS, inspects Mach-O headers in pure Rust (zero `otool` calls), only invoking a verified
/// non-stub `install_name_tool` + ad-hoc `codesign` when RPATH or `@rpath` rewrites are actually needed.
/// On Linux, uses `patchelf` to inject `$ORIGIN/../lib`.
pub fn relocate_bundled_binaries(workspace_root: &Path) -> Result<(), ToolchainError> {
    let toolchains_dir = workspace_root.join(".dmz/toolchains");
    if !toolchains_dir.is_dir() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    let install_name_tool_bin = resolve_non_stub_install_name_tool(&toolchains_dir);

    fn walk_and_relocate(
        dir: &Path,
        #[cfg(target_os = "macos")] install_name_tool_bin: Option<&Path>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };

        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_and_relocate(
                    &p,
                    #[cfg(target_os = "macos")]
                    install_name_tool_bin,
                );
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
                        let Some(tool_bin) = install_name_tool_bin else {
                            // No non-stub install_name_tool on host or bundle; skip relocation
                            // cleanly rather than triggering a macOS Apple CLT GUI prompt.
                            continue;
                        };
                        let Some(load_info) = super::macho::inspect_macho_load_info(&p) else {
                            continue;
                        };

                        let in_lib_dir = p
                            .parent()
                            .and_then(|parent| parent.file_name())
                            .is_some_and(|n| n == "lib");
                        let lib_dir_opt = if in_lib_dir {
                            p.parent().map(Path::to_path_buf)
                        } else {
                            p.parent()
                                .and_then(|d| d.parent())
                                .map(|root| root.join("lib"))
                        };

                        // Resolve which bundled `lib/` dylibs this Mach-O still links by absolute path
                        let mut rewrites: Vec<(&str, String)> = Vec::new();
                        for dep_path in &load_info.dylib_deps {
                            if dep_path.starts_with('/')
                                && !dep_path.starts_with("/usr/lib/")
                                && !dep_path.starts_with("/System/Library/")
                            {
                                if let Some(dep_name) =
                                    Path::new(dep_path).file_name().and_then(|n| n.to_str())
                                {
                                    // Exact case-sensitive filename match inside `lib/`
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
                                        rewrites.push((dep_path, format!("@rpath/{}", dep_name)));
                                    }
                                }
                            }
                        }

                        let has_rpath_dep = !rewrites.is_empty()
                            || load_info
                                .dylib_deps
                                .iter()
                                .any(|d| d.starts_with("@rpath/"));
                        let needs_loader_rpath = has_rpath_dep
                            && !load_info.rpaths.iter().any(|r| r == "@loader_path/../lib");
                        let needs_exec_rpath = !in_lib_dir
                            && has_rpath_dep
                            && !load_info
                                .rpaths
                                .iter()
                                .any(|r| r == "@executable_path/../lib");
                        let desired_id = in_lib_dir
                            .then(|| p.file_name().and_then(|n| n.to_str()))
                            .flatten()
                            .map(|fname| format!("@rpath/{}", fname));
                        let needs_id = load_info.is_dylib
                            && desired_id
                                .as_ref()
                                .is_some_and(|id| load_info.install_id.as_ref() != Some(id));

                        if !needs_loader_rpath
                            && !needs_exec_rpath
                            && !needs_id
                            && rewrites.is_empty()
                        {
                            continue;
                        }

                        if needs_loader_rpath {
                            let _ = Command::new(tool_bin)
                                .args(["-add_rpath", "@loader_path/../lib"])
                                .arg(&p)
                                .output();
                        }
                        if needs_exec_rpath {
                            let _ = Command::new(tool_bin)
                                .args(["-add_rpath", "@executable_path/../lib"])
                                .arg(&p)
                                .output();
                        }
                        if needs_id {
                            if let Some(id) = desired_id {
                                let _ = Command::new(tool_bin).args(["-id", &id]).arg(&p).output();
                            }
                        }
                        for (old_dep, new_dep) in &rewrites {
                            let _ = Command::new(tool_bin)
                                .args(["-change", old_dep, new_dep])
                                .arg(&p)
                                .output();
                        }

                        if Path::new("/usr/bin/codesign").is_file() {
                            let _ = Command::new("/usr/bin/codesign")
                                .args(["--force", "--sign", "-"])
                                .arg(&p)
                                .output();
                        }
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

    walk_and_relocate(
        &toolchains_dir,
        #[cfg(target_os = "macos")]
        install_name_tool_bin.as_deref(),
    );
    Ok(())
}
