use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, is_macos_usr_bin_stub, push_system_bin_candidates, push_unique_path,
    redact_home_prefix, resolve_prefix_root, select_strict_standalone_verified,
    ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

/// Discovers official standalone CMake distributions (binaries and standard module libraries).
#[derive(Debug, Default, Clone)]
pub struct CMakeDiscoverer;

impl ToolchainDiscoverer for CMakeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::CMake
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let candidates = collect_cmake_candidates();
        if candidates.is_empty() {
            return Ok(None);
        }

        let Some((cmake_bin, version)) =
            select_strict_standalone_verified(candidates, probe_cmake_version)
        else {
            return Ok(None);
        };

        let canonical_bin = fs::canonicalize(&cmake_bin).unwrap_or_else(|_| cmake_bin.clone());
        let Some(bin_dir) = canonical_bin.parent().filter(|p| !p.as_os_str().is_empty()) else {
            return Ok(None);
        };
        let Some(root_dir) = resolve_prefix_root(&canonical_bin) else {
            return Ok(None);
        };

        let redacted_bin = redact_home_prefix(&canonical_bin);
        let redacted_root = redact_home_prefix(&root_dir);
        info!(
            cmake = %redacted_bin,
            root = %redacted_root,
            version = %version,
            "Discovered standalone CMake distribution"
        );

        let bin_files = collect_cmake_binaries(bin_dir, &canonical_bin);
        let lib_files = collect_cmake_modules(&root_dir);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::CMake,
            version,
            root_dir,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_cmake_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(root) = std::env::var_os("CMAKE_ROOT")
        .or_else(|| std::env::var_os("CMAKE_HOME"))
        .filter(|v| !v.is_empty())
    {
        let p = PathBuf::from(root);
        if p.join("bin/cmake").is_file() {
            push_unique_path(&mut candidates, p.join("bin/cmake"));
        } else if p.join("cmake").is_file() {
            push_unique_path(&mut candidates, p.join("cmake"));
        }
    }

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
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

    push_system_bin_candidates(&mut candidates, "cmake");
    candidates.retain(|c| c.is_file());
    candidates
}

fn probe_cmake_version(candidate: &Path) -> Option<String> {
    if is_macos_usr_bin_stub(candidate) || !candidate.is_file() {
        return None;
    }

    let output = Command::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("cmake")
            .trim()
            .to_string(),
    )
}

fn collect_cmake_binaries(bin_dir: &Path, canonical_bin: &Path) -> Vec<(PathBuf, String)> {
    let mut bin_files = Vec::new();
    for name in &["cmake", "ctest", "cpack"] {
        let p = bin_dir.join(name);
        if p.is_file() {
            bin_files.push((p, format!("bin/{}", name)));
        } else if cfg!(windows) {
            let exe = bin_dir.join(format!("{}.exe", name));
            if exe.is_file() {
                bin_files.push((exe, format!("bin/{}.exe", name)));
            }
        }
    }

    if bin_files.is_empty() && canonical_bin.is_file() {
        bin_files.push((canonical_bin.to_path_buf(), "bin/cmake".to_string()));
    }

    bin_files
}

fn collect_cmake_modules(root_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();
    let share_dir = root_dir.join("share");
    if let Ok(entries) = fs::read_dir(&share_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let s = entry.file_name().to_string_lossy().into_owned();
            if p.is_dir() && s.starts_with("cmake") {
                collect_files_recursive(&p, root_dir, &mut lib_files, "");
            }
        }
    }
    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_cmake_collects_modules_and_rejects_homebrew() {
        let brew = vec![
            PathBuf::from("/opt/homebrew/bin/cmake"),
            PathBuf::from("/usr/local/Cellar/cmake/3.30.0/bin/cmake"),
        ];
        assert_eq!(
            select_strict_standalone_verified(brew, |_| Some("3.30.0".to_string())),
            None
        );

        let temp = tempdir().unwrap();
        let contents = temp.path().join("CMake.app/Contents");
        let bin_dir = contents.join("bin");
        let modules_dir = contents.join("share/cmake-3.30/Modules");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&modules_dir).unwrap();
        fs::write(bin_dir.join("cmake"), "cmake").unwrap();
        fs::write(bin_dir.join("ctest"), "ctest").unwrap();
        fs::write(modules_dir.join("FindZLIB.cmake"), "# zlib").unwrap();

        let cmake_bin = bin_dir.join("cmake");
        let prefix = resolve_prefix_root(&cmake_bin).unwrap();
        assert_eq!(prefix, fs::canonicalize(&contents).unwrap());

        let libs = collect_cmake_modules(&prefix);
        assert!(libs
            .iter()
            .any(|(_, rel)| rel == "share/cmake-3.30/Modules/FindZLIB.cmake"));
    }
}
