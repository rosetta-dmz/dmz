use super::super::types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainProfile,
};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_deduped_file, push_system_bin_candidates, push_unique_path,
    redact_home_prefix, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

const MINIMAL_RUST_COMPANION_BINS: &[&str] = &["cargo", "rustc"];
const FULL_RUST_COMPANION_BINS: &[&str] = &[
    "cargo",
    "rustc",
    "rustdoc",
    "rust-analyzer",
    "cargo-clippy",
    "clippy-driver",
];

/// Discovers official standalone Rust distributions (Rustup sysroots, cargo, rustc, and shared libraries).
#[derive(Debug, Default, Clone)]
pub struct RustDiscoverer;

impl ToolchainDiscoverer for RustDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Rust
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        self.discover_with_context(&ToolchainDiscoveryContext::default())
    }

    fn discover_with_context(
        &self,
        ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let rustc_candidates = collect_rustc_candidates();
        if rustc_candidates.is_empty() {
            return Ok(None);
        }

        let Some((_, (sysroot, version))) =
            select_strict_standalone_verified(rustc_candidates, probe_rustc_sysroot)
        else {
            return Ok(None);
        };

        let redacted_sysroot = redact_home_prefix(&sysroot);
        info!(
            sysroot = %redacted_sysroot,
            version = %version,
            profile = %ctx.profile,
            "Discovered host Rust toolchain sysroot"
        );

        let bin_files = collect_rust_binaries_for_profile(&sysroot, ctx.profile);
        let lib_files =
            collect_rust_libs_for_context(&sysroot, ctx.profile, ctx.target_platform.as_deref());

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version,
            root_dir: sysroot,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_rustc_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(rustup_home) = std::env::var_os("RUSTUP_HOME").filter(|v| !v.is_empty()) {
        push_rustup_toolchains(
            &mut candidates,
            &PathBuf::from(rustup_home).join("toolchains"),
        );
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let home_path = PathBuf::from(home);
        push_rustup_toolchains(&mut candidates, &home_path.join(".rustup/toolchains"));
        let cargo_rustc = home_path.join(".cargo/bin/rustc");
        if cargo_rustc.is_file() {
            push_unique_path(&mut candidates, cargo_rustc);
        }
    }

    push_system_bin_candidates(&mut candidates, "rustc");
    candidates
}

fn push_rustup_toolchains(candidates: &mut Vec<PathBuf>, toolchains_dir: &Path) {
    let Ok(entries) = fs::read_dir(toolchains_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let bin = entry.path().join("bin/rustc");
        if bin.is_file() {
            push_unique_path(candidates, bin);
        }
    }
}

fn probe_rustc_sysroot(candidate: &Path) -> Option<(PathBuf, String)> {
    if is_macos_usr_bin_stub(candidate) || !candidate.is_file() {
        return None;
    }

    let output = Command::new(candidate)
        .args(["--print", "sysroot"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let sysroot_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sysroot_str.is_empty() {
        return None;
    }

    let sysroot = PathBuf::from(&sysroot_str);
    if !sysroot.is_dir() || sysroot == Path::new("/") {
        return None;
    }
    if is_non_standalone_path(&sysroot) {
        emit_non_relocatable_notice(&sysroot);
        return None;
    }

    let version = Command::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rustc".to_string());

    Some((sysroot, version))
}

fn collect_rust_binaries_for_profile(
    sysroot: &Path,
    profile: ToolchainProfile,
) -> Vec<(PathBuf, String)> {
    let mut bin_files = Vec::new();
    let mut seen_bin = HashSet::new();
    let bins = match profile {
        ToolchainProfile::Minimal => MINIMAL_RUST_COMPANION_BINS,
        ToolchainProfile::Full => FULL_RUST_COMPANION_BINS,
    };

    for bin_name in bins {
        let bin_path = sysroot.join("bin").join(bin_name);
        if bin_path.is_file() {
            push_deduped_file(
                &mut bin_files,
                &mut seen_bin,
                bin_path,
                format!("bin/{}", bin_name),
            );
            continue;
        }

        let Ok(found) = which::which(bin_name) else {
            continue;
        };
        if is_non_standalone_path(&found) {
            emit_non_relocatable_notice(&found);
            continue;
        }
        if is_macos_usr_bin_stub(&found) || is_rustup_proxy(&found) {
            continue;
        }
        push_deduped_file(
            &mut bin_files,
            &mut seen_bin,
            found,
            format!("bin/{}", bin_name),
        );
    }

    bin_files
}

fn is_rustup_proxy(path: &Path) -> bool {
    if let Ok(target) = fs::read_link(path) {
        if target.to_string_lossy().contains("rustup") {
            return true;
        }
    }
    if let Some(parent) = path.parent() {
        if parent.ends_with(".cargo/bin") && parent.join("rustup").exists() {
            return true;
        }
    }
    false
}

fn collect_rust_libs_for_context(
    sysroot: &Path,
    profile: ToolchainProfile,
    target_platform: Option<&str>,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let root_lib_dir = sysroot.join("lib");
    if let Ok(entries) = fs::read_dir(&root_lib_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "dylib" || ext == "so" || ext == "dll" {
                    if let Some(file_name) = p.file_name().and_then(|f| f.to_str()) {
                        lib_files.push((p.clone(), format!("lib/{}", file_name)));
                    }
                }
            }
        }
    }

    let rustlib_dir = sysroot.join("lib/rustlib");
    if rustlib_dir.is_dir() {
        match profile {
            ToolchainProfile::Full => {
                collect_files_recursive(&rustlib_dir, &rustlib_dir, &mut lib_files, "lib/rustlib");
            }
            ToolchainProfile::Minimal => {
                collect_rustlib_minimal(&rustlib_dir, target_platform, &mut lib_files);
            }
        }
    }

    lib_files
}

fn collect_rustlib_minimal(
    rustlib_dir: &Path,
    target_platform: Option<&str>,
    lib_files: &mut Vec<(PathBuf, String)>,
) {
    let mut allowed_triples = HashSet::new();
    allowed_triples.insert(crate::workspace::host_target_triple().to_string());
    if let Some(target_triple) = crate::workspace::resolve_rust_target_triple(target_platform) {
        allowed_triples.insert(target_triple);
    }

    let Ok(entries) = fs::read_dir(rustlib_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_file() {
            lib_files.push((p, format!("lib/rustlib/{}", name)));
            continue;
        }
        if !p.is_dir() {
            continue;
        }
        if name == "src" || name == "share" {
            continue;
        }
        if name == "etc" || allowed_triples.contains(&name) {
            collect_files_recursive(&p, &p, lib_files, &format!("lib/rustlib/{}", name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_rust_profile_gates_dev_tools_and_foreign_targets() {
        let temp = tempdir().unwrap();
        let sysroot = temp.path().join("fake_sysroot");
        let bin = sysroot.join("bin");
        let host = crate::workspace::host_target_triple();
        let foreign_triple = if host == "x86_64-pc-windows-msvc" {
            "aarch64-apple-darwin"
        } else {
            "x86_64-pc-windows-msvc"
        };
        let host_rustlib = sysroot.join("lib/rustlib").join(host).join("lib");
        let foreign_rustlib = sysroot.join("lib/rustlib").join(foreign_triple).join("lib");

        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&host_rustlib).unwrap();
        fs::create_dir_all(&foreign_rustlib).unwrap();

        fs::write(bin.join("cargo"), "cargo").unwrap();
        fs::write(bin.join("rustc"), "rustc").unwrap();
        fs::write(bin.join("rust-analyzer"), "ra").unwrap();
        fs::write(bin.join("cargo-clippy"), "clippy").unwrap();
        fs::write(host_rustlib.join("libcore.rlib"), "core").unwrap();
        fs::write(foreign_rustlib.join("libcore.rlib"), "foreign").unwrap();

        let minimal_bins: Vec<String> =
            collect_rust_binaries_for_profile(&sysroot, ToolchainProfile::Minimal)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(minimal_bins.contains(&"bin/cargo".to_string()));
        assert!(minimal_bins.contains(&"bin/rustc".to_string()));
        assert!(!minimal_bins.contains(&"bin/rust-analyzer".to_string()));
        assert!(!minimal_bins.contains(&"bin/cargo-clippy".to_string()));

        let full_bins: Vec<String> =
            collect_rust_binaries_for_profile(&sysroot, ToolchainProfile::Full)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(full_bins.contains(&"bin/rust-analyzer".to_string()));
        assert!(full_bins.contains(&"bin/cargo-clippy".to_string()));

        let minimal_libs: Vec<String> =
            collect_rust_libs_for_context(&sysroot, ToolchainProfile::Minimal, None)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(minimal_libs.iter().any(|p| p.contains(host)));
        assert!(!minimal_libs.iter().any(|p| p.contains(foreign_triple)));
    }

    #[test]
    fn test_rust_rejects_homebrew_and_root_sysroot() {
        assert!(probe_rustc_sysroot(Path::new("/opt/homebrew/bin/rustc")).is_none());
        assert!(probe_rustc_sysroot(Path::new("/")).is_none());
        if cfg!(target_os = "macos") {
            assert!(is_macos_usr_bin_stub(Path::new("/usr/bin/rustc")));
            assert!(probe_rustc_sysroot(Path::new("/usr/bin/rustc")).is_none());
        }

        let candidates = vec![
            PathBuf::from("/opt/homebrew/bin/rustc"),
            PathBuf::from("/nix/store/123-rust/bin/rustc"),
        ];
        assert_eq!(
            select_strict_standalone_verified(candidates, |_| Some((
                PathBuf::from("/tmp"),
                "1.80".to_string()
            ))),
            None
        );
    }
}
