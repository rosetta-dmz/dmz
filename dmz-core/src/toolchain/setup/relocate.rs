// Binary RPATH relocation and Mach-O / ELF code signing

use super::super::ToolchainError;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

#[cfg(target_os = "macos")]
fn resolve_non_stub_install_name_tool(toolchains_dir: &Path) -> Option<PathBuf> {
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

/// Computes the relative runtime prefix (e.g. `@loader_path` or `$ORIGIN`) from `binary_dir`
/// to `toolchain_lib_dir` (`bin/` -> `<token>/../lib`, `lib/` -> `<token>`, `lib/python3.12/lib-dynload/` -> `<token>/../..`).
fn relative_token_lib_prefix(
    binary_dir: &Path,
    toolchain_lib_dir: &Path,
    token: &str,
) -> Option<String> {
    let bin_comps: Vec<Component<'_>> = binary_dir.components().collect();
    let lib_comps: Vec<Component<'_>> = toolchain_lib_dir.components().collect();

    let common = bin_comps
        .iter()
        .zip(lib_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common == 0 && (binary_dir.is_absolute() || toolchain_lib_dir.is_absolute()) {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    for _ in bin_comps.iter().skip(common) {
        parts.push("..".to_string());
    }
    for comp in lib_comps.iter().skip(common) {
        parts.push(comp.as_os_str().to_string_lossy().into_owned());
    }

    if parts.is_empty() {
        Some(token.to_string())
    } else {
        Some(format!("{}/{}", token, parts.join("/")))
    }
}

/// Computes merged `$ORIGIN` RPATH tokens, filtering non-`$ORIGIN` host paths cleanly.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn compute_linux_rpath_tokens(
    binary_dir: &Path,
    toolchain_lib_dir: Option<&Path>,
    existing: &[String],
) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let origin_lib = toolchain_lib_dir
        .and_then(|lib| relative_token_lib_prefix(binary_dir, lib, "$ORIGIN"))
        .unwrap_or_else(|| "$ORIGIN/../lib".to_string());

    for candidate in [origin_lib, "$ORIGIN".to_string()] {
        if !candidate.is_empty() && !tokens.contains(&candidate) {
            tokens.push(candidate);
        }
    }
    for prev in existing {
        let is_origin_relative = prev.starts_with("$ORIGIN") || prev.starts_with("${ORIGIN}");
        if is_origin_relative && !tokens.contains(prev) {
            tokens.push(prev.clone());
        }
    }
    tokens
}

#[cfg(target_os = "macos")]
fn run_relocation_tool(tool_bin: &Path, args: &[&str], target: &Path, action: &str) -> bool {
    match Command::new(tool_bin).args(args).arg(target).output() {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            tracing::warn!(
                action,
                status = ?output.status.code(),
                stderr = %stderr,
                "Mach-O relocation tool returned non-zero status"
            );
            false
        }
        Err(err) => {
            tracing::warn!(action, error = %err, "Failed to spawn Mach-O relocation tool");
            false
        }
    }
}

/// Relocates bundled binaries and shared libraries using relative runtime search paths (`@loader_path` / `$ORIGIN`).
///
/// On macOS, inspects Mach-O headers in pure Rust (zero `otool` calls), prefers in-place `@loader_path`
/// rewrites, and falls back to `patch_macho_dylib_paths_in_place` + ad-hoc `codesign` when `install_name_tool` is absent.
/// On Linux, inspects ELF binaries in pure Rust and invokes `patchelf --force-rpath --set-rpath` with depth-relative `$ORIGIN` paths.
///
/// # Errors
/// Returns an error only if fatal relocation setup invariants fail.
pub fn relocate_bundled_binaries(workspace_root: &Path) -> Result<(), ToolchainError> {
    let toolchains_dir = workspace_root.join(".dmz/toolchains");
    if !toolchains_dir.is_dir() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    let install_name_tool_bin = resolve_non_stub_install_name_tool(&toolchains_dir);

    walk_and_relocate(
        &toolchains_dir,
        None,
        #[cfg(target_os = "macos")]
        install_name_tool_bin.as_deref(),
    );
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn resolve_lib_dir_opt(p: &Path, toolchain_lib_dir: Option<&Path>) -> (bool, Option<PathBuf>) {
    let in_lib_dir = p
        .parent()
        .and_then(|parent| parent.file_name())
        .is_some_and(|n| n == "lib");
    let lib_dir_opt = toolchain_lib_dir
        .filter(|d| d.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| {
            if in_lib_dir {
                p.parent().map(Path::to_path_buf)
            } else {
                p.parent()
                    .and_then(|d| d.parent())
                    .map(|root| root.join("lib"))
            }
        });
    (in_lib_dir, lib_dir_opt)
}

#[cfg(target_os = "macos")]
fn is_macho_magic(magic: [u8; 4]) -> bool {
    matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xcf | 0xce]
            | [0xcf | 0xce, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe | 0xbf]
            | [0xbe | 0xbf, 0xba, 0xfe, 0xca]
    )
}

#[cfg(target_os = "macos")]
struct MachORewrites<'a> {
    direct: Vec<(&'a str, String)>,
    rpath: Vec<(&'a str, String)>,
}

#[cfg(target_os = "macos")]
fn collect_macho_rewrites<'a>(
    load_info: &'a super::macho::MachOLoadInfo,
    lib_dir_opt: Option<&Path>,
    loader_lib_prefix: &str,
) -> MachORewrites<'a> {
    let mut direct = Vec::new();
    let mut rpath = Vec::new();
    for dep_path in &load_info.dylib_deps {
        if !dep_path.starts_with('/')
            || dep_path.starts_with("/usr/lib/")
            || dep_path.starts_with("/System/Library/")
        {
            continue;
        }
        let Some(dep_name) = Path::new(dep_path).file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let has_bundled_dep = lib_dir_opt
            .and_then(|dir| fs::read_dir(dir).ok())
            .is_some_and(|entries| {
                entries.flatten().any(|e| {
                    e.file_name().to_str() == Some(dep_name)
                        && fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_file())
                })
            });
        if !has_bundled_dep {
            continue;
        }
        let direct_target = format!("{loader_lib_prefix}/{dep_name}");
        if direct_target.len() <= dep_path.len() {
            direct.push((dep_path.as_str(), direct_target));
        } else {
            rpath.push((dep_path.as_str(), format!("@rpath/{dep_name}")));
        }
    }
    MachORewrites { direct, rpath }
}

#[cfg(target_os = "macos")]
fn relocate_macos_macho(
    p: &Path,
    toolchain_lib_dir: Option<&Path>,
    install_name_tool_bin: Option<&Path>,
) {
    let Some(load_info) = super::macho::inspect_macho_load_info(p) else {
        return;
    };
    let (in_lib_dir, lib_dir_opt) = resolve_lib_dir_opt(p, toolchain_lib_dir);
    let loader_lib_prefix = p
        .parent()
        .zip(lib_dir_opt.as_deref())
        .and_then(|(bin_dir, lib_dir)| relative_token_lib_prefix(bin_dir, lib_dir, "@loader_path"))
        .unwrap_or_else(|| "@loader_path/../lib".to_string());

    let rewrites = collect_macho_rewrites(&load_info, lib_dir_opt.as_deref(), &loader_lib_prefix);
    let has_rpath_dep = !rewrites.rpath.is_empty()
        || load_info
            .dylib_deps
            .iter()
            .any(|d| d.starts_with("@rpath/"));
    let needs_loader_rpath = has_rpath_dep
        && !load_info
            .rpaths
            .iter()
            .any(|r| r == &loader_lib_prefix || r == "@loader_path/../lib");
    let needs_exec_rpath = !in_lib_dir
        && has_rpath_dep
        && !load_info
            .rpaths
            .iter()
            .any(|r| r == "@executable_path/../lib");
    let desired_id = in_lib_dir
        .then(|| p.file_name().and_then(|n| n.to_str()))
        .flatten()
        .map(|fname| format!("@rpath/{fname}"));
    let needs_id = load_info.is_dylib
        && desired_id
            .as_ref()
            .is_some_and(|id| load_info.install_id.as_ref() != Some(id));

    if !needs_loader_rpath
        && !needs_exec_rpath
        && !needs_id
        && rewrites.direct.is_empty()
        && rewrites.rpath.is_empty()
    {
        return;
    }

    let mut modified = false;
    if let Some(tool_bin) = install_name_tool_bin {
        for (old_dep, new_dep) in &rewrites.direct {
            if run_relocation_tool(
                tool_bin,
                &["-change", old_dep, new_dep],
                p,
                "change-direct-loader-path",
            ) {
                modified = true;
            }
        }
        let loader_ready = !needs_loader_rpath
            || run_relocation_tool(
                tool_bin,
                &["-add_rpath", &loader_lib_prefix],
                p,
                "add-loader-rpath",
            );
        let exec_ready = !needs_exec_rpath
            || run_relocation_tool(
                tool_bin,
                &["-add_rpath", "@executable_path/../lib"],
                p,
                "add-executable-rpath",
            );
        if (needs_loader_rpath && loader_ready) || (needs_exec_rpath && exec_ready) {
            modified = true;
        }
        if needs_id {
            if let Some(id) = desired_id.as_deref() {
                if run_relocation_tool(tool_bin, &["-id", id], p, "set-dylib-id") {
                    modified = true;
                }
            }
        }
        if loader_ready || exec_ready {
            for (old_dep, new_dep) in &rewrites.rpath {
                if run_relocation_tool(
                    tool_bin,
                    &["-change", old_dep, new_dep],
                    p,
                    "change-rpath-dep",
                ) {
                    modified = true;
                }
            }
        }
    } else {
        // Zero-CLT fallback: patch in-place strings cleanly without header growth.
        let direct_pairs: Vec<(&str, &str)> = rewrites
            .direct
            .iter()
            .map(|(o, n)| (*o, n.as_str()))
            .collect();
        let id_fallback = needs_id.then_some(desired_id.as_deref()).flatten();
        if let Ok(patched) =
            super::macho::patch_macho_dylib_paths_in_place(p, &direct_pairs, id_fallback)
        {
            modified = patched;
        }
        if !rewrites.rpath.is_empty() || needs_loader_rpath || needs_exec_rpath {
            tracing::warn!(
                "install_name_tool unavailable; skipped Mach-O header-expanding rpath rewrites"
            );
        }
    }

    if modified {
        let codesign_bin = Path::new("/usr/bin/codesign");
        if codesign_bin.is_file() {
            let _ = run_relocation_tool(
                codesign_bin,
                &["--force", "--sign", "-"],
                p,
                "ad-hoc-codesign",
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn relocate_linux_elf(p: &Path, dir: &Path, toolchain_lib_dir: Option<&Path>) {
    let Some(load_info) = super::elf::inspect_elf_load_info(p) else {
        return;
    };
    if !load_info.is_dynamic_binary {
        return;
    }
    let (_, lib_dir_opt) = resolve_lib_dir_opt(p, toolchain_lib_dir);
    let bin_dir = p.parent().unwrap_or(dir);
    let desired = compute_linux_rpath_tokens(bin_dir, lib_dir_opt.as_deref(), &load_info.rpaths);
    if load_info.has_dt_rpath && load_info.rpaths == desired {
        return;
    }
    let joined = desired.join(":");
    match Command::new("patchelf")
        .arg("--force-rpath")
        .arg("--set-rpath")
        .arg(&joined)
        .arg(p)
        .output()
    {
        Ok(output) if !output.status.success() => {
            tracing::warn!(
                status = ?output.status.code(),
                "patchelf --force-rpath --set-rpath returned non-zero status"
            );
        }
        Err(err) => {
            tracing::warn!(error = %err, "Failed to spawn patchelf");
        }
        Ok(_) => {}
    }
}

fn walk_and_relocate(
    dir: &Path,
    toolchain_lib_dir: Option<&Path>,
    #[cfg(target_os = "macos")] install_name_tool_bin: Option<&Path>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let p = entry.path();
        // Reject symlinks before directory checks to prevent escaping `.dmz/toolchains`.
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let next_lib_dir = toolchain_lib_dir
                .map(Path::to_path_buf)
                .or_else(|| Some(p.join("lib")));
            walk_and_relocate(
                &p,
                next_lib_dir.as_deref(),
                #[cfg(target_os = "macos")]
                install_name_tool_bin,
            );
            continue;
        }
        if !meta.is_file() {
            continue;
        }

        let mut magic = [0u8; 4];
        if File::open(&p)
            .and_then(|mut f| f.read_exact(&mut magic))
            .is_err()
        {
            continue;
        }

        #[cfg(target_os = "macos")]
        if is_macho_magic(magic) {
            relocate_macos_macho(&p, toolchain_lib_dir, install_name_tool_bin);
        }

        #[cfg(target_os = "linux")]
        if magic == *b"\x7fELF" {
            relocate_linux_elf(&p, dir, toolchain_lib_dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_relative_loader_lib_prefix_handles_bin_lib_and_nested_dynload() {
        let tc = Path::new("/ws/.dmz/toolchains/python");
        let lib = tc.join("lib");
        assert_eq!(
            relative_token_lib_prefix(&tc.join("bin"), &lib, "@loader_path").as_deref(),
            Some("@loader_path/../lib")
        );
        assert_eq!(
            relative_token_lib_prefix(&lib, &lib, "@loader_path").as_deref(),
            Some("@loader_path")
        );
        assert_eq!(
            relative_token_lib_prefix(&lib.join("python3.12/lib-dynload"), &lib, "@loader_path")
                .as_deref(),
            Some("@loader_path/../..")
        );
    }

    #[test]
    fn test_compute_linux_rpath_tokens_handles_depth_and_preserves_existing() {
        let tc = Path::new("/ws/.dmz/toolchains/python");
        let lib = tc.join("lib");
        assert_eq!(
            compute_linux_rpath_tokens(&tc.join("bin"), Some(&lib), &[]),
            vec!["$ORIGIN/../lib", "$ORIGIN"]
        );
        assert_eq!(
            compute_linux_rpath_tokens(
                &lib.join("python3.12/lib-dynload"),
                Some(&lib),
                &[
                    "$ORIGIN/../..".to_string(),
                    "/host/build/lib".to_string(),
                    "$ORIGIN/extra".to_string()
                ]
            ),
            vec!["$ORIGIN/../..", "$ORIGIN", "$ORIGIN/extra"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_relocate_skips_symlinked_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let outside_dir = ws.join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        let mut outside_bin = File::create(outside_dir.join("escape_bin")).unwrap();
        outside_bin.write_all(b"\x7fELF").unwrap();

        let tc_dir = ws.join(".dmz/toolchains/python");
        fs::create_dir_all(&tc_dir).unwrap();
        std::os::unix::fs::symlink(&outside_dir, tc_dir.join("escaped_link")).unwrap();

        assert!(relocate_bundled_binaries(ws).is_ok());
        assert_eq!(
            fs::read(outside_dir.join("escape_bin")).unwrap(),
            b"\x7fELF"
        );
    }
}
