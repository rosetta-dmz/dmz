use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path, push_unique_path,
    ToolchainDiscoverer,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

/// Discovers official standalone Rust distributions (Rustup sysroots, cargo, rustc, and shared libraries).
#[derive(Debug, Default, Clone)]
pub struct RustDiscoverer;

impl ToolchainDiscoverer for RustDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Rust
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut rustc_candidates: Vec<PathBuf> = Vec::new();

        // 1. Check official standalone RUSTUP_HOME toolchains
        if let Ok(rustup_home) = std::env::var("RUSTUP_HOME") {
            let tc_dir = PathBuf::from(rustup_home).join("toolchains");
            if let Ok(entries) = fs::read_dir(&tc_dir) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin/rustc");
                    if bin.exists() {
                        push_unique_path(&mut rustc_candidates, bin);
                    }
                }
            }
        }

        // 2. Check standard user ~/.rustup/toolchains
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let tc_dir = PathBuf::from(&home).join(".rustup/toolchains");
            if let Ok(entries) = fs::read_dir(&tc_dir) {
                for entry in entries.flatten() {
                    let bin = entry.path().join("bin/rustc");
                    if bin.exists() {
                        push_unique_path(&mut rustc_candidates, bin);
                    }
                }
            }

            let cargo_rustc = PathBuf::from(&home).join(".cargo/bin/rustc");
            if cargo_rustc.exists() {
                push_unique_path(&mut rustc_candidates, cargo_rustc);
            }
        }

        // 3. Fallback to which rustc
        if let Ok(p) = which::which("rustc") {
            push_unique_path(&mut rustc_candidates, p);
        }

        if rustc_candidates.is_empty() {
            return Ok(None);
        }

        // Evaluate candidates, prioritizing standalone official toolchains
        let mut chosen_sysroot: Option<PathBuf> = None;
        let mut chosen_version: Option<String> = None;

        // Sort candidates: prioritize paths that are NOT non-standalone
        rustc_candidates.sort_by_key(|c| is_non_standalone_path(c));

        for candidate in rustc_candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }

            if let Ok(output) = Command::new(&candidate)
                .arg("--print")
                .arg("sysroot")
                .output()
            {
                if output.status.success() {
                    let sysroot_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let sysroot = PathBuf::from(&sysroot_str);
                    if sysroot.exists() {
                        let is_non_standalone = is_non_standalone_path(&sysroot);
                        if is_non_standalone {
                            emit_non_relocatable_notice(&sysroot);
                        }

                        let version = Command::new(&candidate)
                            .arg("--version")
                            .output()
                            .ok()
                            .and_then(|o| String::from_utf8(o.stdout).ok())
                            .unwrap_or_else(|| "rustc".to_string())
                            .trim()
                            .to_string();

                        // If chosen_sysroot already has a standalone candidate, don't overwrite with non-standalone
                        if chosen_sysroot.is_none()
                            || (!is_non_standalone
                                && is_non_standalone_path(chosen_sysroot.as_ref().unwrap()))
                        {
                            chosen_sysroot = Some(sysroot);
                            chosen_version = Some(version);
                            if !is_non_standalone {
                                break;
                            }
                        }
                    }
                }
            }
        }

        let sysroot = match chosen_sysroot {
            Some(s) => s,
            None => return Ok(None),
        };
        let version = chosen_version.unwrap_or_else(|| "rustc".to_string());

        info!(
            sysroot = ?sysroot,
            version = %version,
            "Discovered host Rust toolchain sysroot"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        // Binaries: cargo, rustc, rustdoc
        for bin_name in &["cargo", "rustc", "rustdoc"] {
            let bin_path = sysroot.join("bin").join(bin_name);
            if bin_path.exists() {
                bin_files.push((bin_path, format!("bin/{}", bin_name)));
            } else if let Ok(found) = which::which(bin_name) {
                bin_files.push((found, format!("bin/{}", bin_name)));
            }
        }

        // Shared libraries directly under <sysroot>/lib/ (e.g. librustc_driver, libstd, libLLVM)
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

        // Standard libraries in <sysroot>/lib/rustlib/<target>/lib
        let rustlib_dir = sysroot.join("lib/rustlib");
        if rustlib_dir.exists() {
            collect_files_recursive(&rustlib_dir, &rustlib_dir, &mut lib_files, "lib/rustlib");
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version,
            root_dir: sysroot,
            bin_files,
            lib_files,
        }))
    }
}
