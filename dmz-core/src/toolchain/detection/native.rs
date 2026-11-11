use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct NativeBuildDetector;

impl NativeBuildDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if dir.join("CMakeLists.txt").is_file() {
            ecosystems.insert(ToolchainEcosystem::CMake);
            ecosystems.insert(ToolchainEcosystem::Cc);
            if cmake_uses_pkg_config(dir) {
                ecosystems.insert(ToolchainEcosystem::PkgConfig);
            }
        }
        if dir.join("build.ninja").is_file() {
            ecosystems.insert(ToolchainEcosystem::Ninja);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if dir.join("Makefile").is_file()
            || dir.join("GNUmakefile").is_file()
            || dir.join("makefile").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Make);
            if workspace_requires_c_compiler_for_make(dir) {
                ecosystems.insert(ToolchainEcosystem::Cc);
            }
            if makefile_uses_pkg_config(dir) {
                ecosystems.insert(ToolchainEcosystem::PkgConfig);
            }
        }
        if dir.join("meson.build").is_file() {
            ecosystems.insert(ToolchainEcosystem::Cc);
            ecosystems.insert(ToolchainEcosystem::Ninja);
            ecosystems.insert(ToolchainEcosystem::PkgConfig);
        }
        if has_pkg_config_files(dir) {
            ecosystems.insert(ToolchainEcosystem::PkgConfig);
        }
    }
}

/// Returns true if the workspace root or any declared member explicitly contains `build.ninja`
/// or explicitly declares a `ninja` dependency in its manifest tables.
pub fn has_explicit_ninja_manifest(workspace_root: &Path) -> bool {
    if workspace_root.join("build.ninja").is_file() {
        return true;
    }
    let cargo_toml = workspace_root.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        if let Ok(val) = content.parse::<toml::Value>() {
            for section in &["dependencies", "build-dependencies", "dev-dependencies"] {
                if let Some(table) = val.get(*section).and_then(|v| v.as_table()) {
                    if table.contains_key("ninja") || table.contains_key("ninja-build") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check whether any Makefile in the given directory explicitly contains C/C++ compiler invocations.
pub fn makefile_has_native_c_recipes(dir: &Path) -> bool {
    for name in &["Makefile", "GNUmakefile", "makefile"] {
        let p = dir.join(name);
        if p.is_file() {
            if let Ok(content) = std::fs::read_to_string(&p) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('#') {
                        continue;
                    }
                    if trimmed.contains("gcc")
                        || trimmed.contains("g++")
                        || trimmed.contains("clang")
                        || trimmed.contains("clang++")
                        || trimmed.contains("cc ")
                        || trimmed.contains("c++ ")
                        || trimmed.contains("$(CC)")
                        || trimmed.contains("${CC}")
                        || trimmed.contains("$(CXX)")
                        || trimmed.contains("${CXX}")
                        || trimmed.contains(".c ")
                        || trimmed.contains(".cpp ")
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check whether the workspace contains native C/C++ source or header files.
pub fn workspace_has_native_c_sources(dir: &Path) -> bool {
    let extensions = [
        "c", "cpp", "cc", "cxx", "c++", "h", "hpp", "hxx", "h++", "m", "mm",
    ];

    fn scan_dir(dir: &Path, current_depth: usize, max_depth: usize, extensions: &[&str]) -> bool {
        if current_depth > max_depth {
            return false;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                        if extensions.contains(&ext) {
                            return true;
                        }
                    }
                } else if p.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with('.')
                        || name_str == "target"
                        || name_str == "node_modules"
                        || name_str == "dist"
                        || name_str == "build"
                        || name_str == "venv"
                        || name_str == ".venv"
                    {
                        continue;
                    }
                    if scan_dir(&p, current_depth + 1, max_depth, extensions) {
                        return true;
                    }
                }
            }
        }
        false
    }

    scan_dir(dir, 0, 3, &extensions)
}

/// Determine whether a workspace with a Makefile actually requires a C/C++ compiler.
pub fn workspace_requires_c_compiler_for_make(dir: &Path) -> bool {
    workspace_has_native_c_sources(dir) || makefile_has_native_c_recipes(dir)
}

/// Check whether CMake configuration explicitly references pkg-config / pkgconf modules.
pub fn cmake_uses_pkg_config(dir: &Path) -> bool {
    let p = dir.join("CMakeLists.txt");
    if let Ok(content) = std::fs::read_to_string(&p) {
        let lower = content.to_ascii_lowercase();
        if lower.contains("find_package(pkgconfig")
            || lower.contains("pkg_check_modules")
            || lower.contains("pkg_search_module")
        {
            return true;
        }
    }
    false
}

/// Check whether any Makefile in the given directory explicitly invokes pkg-config or pkgconf.
pub fn makefile_uses_pkg_config(dir: &Path) -> bool {
    for name in &["Makefile", "GNUmakefile", "makefile"] {
        let p = dir.join(name);
        if p.is_file() {
            if let Ok(content) = std::fs::read_to_string(&p) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('#') {
                        continue;
                    }
                    if trimmed.contains("pkg-config") || trimmed.contains("pkgconf") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check whether directory contains pkg-config metadata (.pc files or configure script checks).
pub fn has_pkg_config_files(dir: &Path) -> bool {
    for sub in &["pkgconfig", "lib/pkgconfig", "share/pkgconfig"] {
        let p = dir.join(sub);
        if p.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                for entry in entries.flatten() {
                    if entry.path().extension().and_then(|e| e.to_str()) == Some("pc") {
                        return true;
                    }
                }
            }
        }
    }
    for conf in &["configure.ac", "configure.in"] {
        let p = dir.join(conf);
        if p.is_file() {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if content.contains("PKG_CHECK_MODULES") || content.contains("PKG_PROG_PKG_CONFIG")
                {
                    return true;
                }
            }
        }
    }
    false
}
