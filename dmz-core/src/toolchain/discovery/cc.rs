use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path,
    push_system_bin_candidates, push_unique_path, ToolchainDiscoverer,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers standalone host C/C++ compiler toolchains (Clang, GCC, companion archivers, linkers, and builtin headers).
#[derive(Debug, Default, Clone)]
pub struct CcDiscoverer;

impl ToolchainDiscoverer for CcDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Cc
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1. Explicit environment variables
        for var_name in &[
            "CC",
            "CXX",
            "LLVM_HOME",
            "CLANG_HOME",
            "GCC_HOME",
            "DEVELOPER_DIR",
        ] {
            if let Ok(val) = std::env::var(var_name) {
                let p = PathBuf::from(val);
                if p.is_file() {
                    push_unique_path(&mut candidates, p);
                } else if p.is_dir() {
                    for sub in &[
                        "bin/clang",
                        "bin/gcc",
                        "bin/cc",
                        "usr/bin/clang",
                        "Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
                    ] {
                        let candidate = p.join(sub);
                        if candidate.exists() {
                            push_unique_path(&mut candidates, candidate);
                        }
                    }
                }
            }
        }

        // 2. Standard user and system standalone directories
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            push_unique_path(&mut candidates, home_path.join(".local/bin/clang"));
            push_unique_path(&mut candidates, home_path.join(".local/bin/gcc"));
            push_unique_path(&mut candidates, home_path.join(".local/bin/cc"));
            push_unique_path(&mut candidates, home_path.join(".llvm/bin/clang"));
        }

        push_unique_path(&mut candidates, PathBuf::from("/opt/llvm/bin/clang"));
        push_unique_path(&mut candidates, PathBuf::from("/opt/gcc/bin/gcc"));
        push_unique_path(
            &mut candidates,
            PathBuf::from("C:\\Program Files\\LLVM\\bin\\clang.exe"),
        );

        // Apple Xcode & Command Line Tools standard paths
        #[cfg(target_os = "macos")]
        {
            push_unique_path(
                &mut candidates,
                PathBuf::from("/Library/Developer/CommandLineTools/usr/bin/clang"),
            );
            push_unique_path(
                &mut candidates,
                PathBuf::from(
                    "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang",
                ),
            );
        }

        // 3. System PATH fallback
        for name in &["clang", "gcc", "cc"] {
            push_system_bin_candidates(&mut candidates, name);
        }

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        // Prioritize standalone distributions over machine-specific package managers
        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_cc: Option<PathBuf> = None;
        let mut chosen_version: Option<String> = None;

        for candidate in candidates {
            if is_non_standalone_path(&candidate) {
                emit_non_relocatable_notice(&candidate);
            }

            if let Ok(output) = Command::new(&candidate).arg("--version").output() {
                if output.status.success() {
                    let version_str = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("cc")
                        .trim()
                        .to_string();

                    let is_non_standalone = is_non_standalone_path(&candidate);
                    if chosen_cc.is_none()
                        || (!is_non_standalone
                            && is_non_standalone_path(chosen_cc.as_ref().unwrap()))
                    {
                        chosen_cc = Some(candidate);
                        chosen_version = Some(version_str);
                        if !is_non_standalone {
                            break;
                        }
                    }
                }
            }
        }

        let cc_bin = match chosen_cc {
            Some(bin) => bin,
            None => return Ok(None),
        };
        let version = chosen_version.unwrap_or_else(|| "cc".to_string());

        let bin_dir = cc_bin.parent().unwrap_or(Path::new(""));
        let root_dir = bin_dir.parent().unwrap_or(bin_dir).to_path_buf();

        info!(
            cc = ?cc_bin,
            root = ?root_dir,
            version = %version,
            "Discovered standalone C/C++ compiler toolchain"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        // 1. Primary compiler binary
        let primary_name = cc_bin
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cc")
            .to_string();
        bin_files.push((cc_bin.clone(), format!("bin/{}", primary_name)));

        // 2. Companion tools in the same bin directory or system fallback
        let is_clang = primary_name.contains("clang");
        let companion_names = if is_clang {
            vec![
                "clang++",
                "llvm-ar",
                "ar",
                "llvm-ranlib",
                "ranlib",
                "lld",
                "ld.lld",
                "ld",
            ]
        } else {
            vec!["g++", "ar", "ranlib", "ld"]
        };

        for name in companion_names {
            let p = bin_dir.join(name);
            let target_name = format!("bin/{}", name);
            if p.exists() {
                if !bin_files.iter().any(|(_, rel)| rel == &target_name) {
                    bin_files.push((p, target_name));
                }
            } else if cfg!(windows) {
                let exe = bin_dir.join(format!("{}.exe", name));
                if exe.exists() {
                    let target_exe = format!("bin/{}.exe", name);
                    if !bin_files.iter().any(|(_, rel)| rel == &target_exe) {
                        bin_files.push((exe, target_exe));
                    }
                }
            } else if let Ok(found) = which::which(name) {
                if !bin_files.iter().any(|(_, rel)| rel == &target_name) {
                    bin_files.push((found, target_name));
                }
            }
        }

        // 3. Compiler builtin headers (e.g. clang resource dir include for stddef.h, stdint.h)
        if is_clang {
            if let Ok(res_output) = Command::new(&cc_bin).arg("-print-resource-dir").output() {
                if res_output.status.success() {
                    let res_dir_str = String::from_utf8_lossy(&res_output.stdout)
                        .trim()
                        .to_string();
                    let res_dir = PathBuf::from(res_dir_str);
                    let inc_dir = res_dir.join("include");
                    if inc_dir.is_dir() {
                        collect_files_recursive(
                            &inc_dir,
                            &inc_dir,
                            &mut lib_files,
                            "lib/clang/include",
                        );
                    }
                }
            }
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version,
            root_dir,
            bin_files,
            lib_files,
        }))
    }
}
