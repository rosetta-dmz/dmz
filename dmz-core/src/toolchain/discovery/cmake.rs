use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_non_standalone_path,
    push_system_bin_candidates, push_unique_path, ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers official standalone CMake distributions (binaries and standard module libraries).
#[derive(Debug, Default, Clone)]
pub struct CMakeDiscoverer;

impl ToolchainDiscoverer for CMakeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::CMake
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1. Explicit CMAKE_ROOT or CMAKE_HOME environment variables
        if let Ok(root) = std::env::var("CMAKE_ROOT").or_else(|_| std::env::var("CMAKE_HOME")) {
            let p = PathBuf::from(root);
            if p.join("bin/cmake").exists() {
                push_unique_path(&mut candidates, p.join("bin/cmake"));
            } else if p.join("cmake").exists() {
                push_unique_path(&mut candidates, p.join("cmake"));
            }
        }

        // 2. Standard user and system standalone directories
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let home_path = PathBuf::from(home);
            push_unique_path(
                &mut candidates,
                home_path.join(".local/share/cmake/bin/cmake"),
            );
            push_unique_path(&mut candidates, home_path.join(".local/bin/cmake"));
            push_unique_path(&mut candidates, home_path.join(".cmake/bin/cmake"));
            push_unique_path(
                &mut candidates,
                home_path.join("Applications/CMake.app/Contents/bin/cmake"),
            );
        }

        push_unique_path(&mut candidates, PathBuf::from("/opt/cmake/bin/cmake"));
        push_unique_path(
            &mut candidates,
            PathBuf::from("/Applications/CMake.app/Contents/bin/cmake"),
        );
        push_unique_path(
            &mut candidates,
            PathBuf::from("C:\\Program Files\\CMake\\bin\\cmake.exe"),
        );

        // 3. System PATH and standard host bin fallback (supports both CLI and GUI launch environments)
        push_system_bin_candidates(&mut candidates, "cmake");

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        // Prioritize standalone distributions over machine-specific package managers
        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let mut chosen_cmake: Option<PathBuf> = None;
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
                        .unwrap_or("cmake")
                        .trim()
                        .to_string();

                    let is_non_standalone = is_non_standalone_path(&candidate);
                    if chosen_cmake.is_none()
                        || (!is_non_standalone
                            && is_non_standalone_path(chosen_cmake.as_ref().unwrap()))
                    {
                        chosen_cmake = Some(candidate);
                        chosen_version = Some(version_str);
                        if !is_non_standalone {
                            break;
                        }
                    }
                }
            }
        }

        let cmake_bin = match chosen_cmake {
            Some(bin) => bin,
            None => return Ok(None),
        };
        let version = chosen_version.unwrap_or_else(|| "cmake".to_string());

        // Canonicalize cmake_bin so command-line symlinks (e.g. /usr/local/bin/cmake -> /Applications/CMake.app/Contents/bin/cmake)
        // resolve to the actual standalone installation directory where share/cmake-*/Modules resides.
        let canonical_bin = fs::canonicalize(&cmake_bin).unwrap_or_else(|_| cmake_bin.clone());
        let bin_dir = canonical_bin.parent().unwrap_or(Path::new(""));
        let root_dir = bin_dir.parent().unwrap_or(bin_dir).to_path_buf();

        info!(
            cmake = ?canonical_bin,
            root = ?root_dir,
            version = %version,
            "Discovered standalone CMake distribution"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        // Capture core binaries: cmake, ctest, cpack
        for name in &["cmake", "ctest", "cpack"] {
            let p = bin_dir.join(name);
            if p.exists() {
                bin_files.push((p, format!("bin/{}", name)));
            } else if cfg!(windows) {
                let exe = bin_dir.join(format!("{}.exe", name));
                if exe.exists() {
                    bin_files.push((exe, format!("bin/{}.exe", name)));
                }
            }
        }

        if bin_files.is_empty() && canonical_bin.exists() {
            bin_files.push((canonical_bin.clone(), "bin/cmake".to_string()));
        }

        // Capture standard CMake modules under share/cmake-* or share/cmake
        let share_dir = root_dir.join("share");
        if share_dir.exists() {
            if let Ok(entries) = fs::read_dir(&share_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    let fname = entry.file_name();
                    let s = fname.to_string_lossy();
                    if p.is_dir() && s.starts_with("cmake") {
                        collect_files_recursive(&p, &root_dir, &mut lib_files, "");
                    }
                }
            }
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::CMake,
            version,
            root_dir,
            bin_files,
            lib_files,
        }))
    }
}
