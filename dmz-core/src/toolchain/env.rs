use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Inject bundled toolchain binaries and environment variables into runtime execution environments.
pub struct ToolchainEnv;

impl ToolchainEnv {
    fn prepend_path_var(
        env_map: &mut HashMap<String, String>,
        key: &str,
        path: &str,
        separator: &str,
    ) {
        if path.is_empty() {
            return;
        }
        let existing = env_map.get(key).cloned().unwrap_or_default();
        if existing.is_empty() {
            env_map.insert(key.to_string(), path.to_string());
        } else if !existing.split(separator).any(|p| p == path) {
            env_map.insert(
                key.to_string(),
                format!("{}{}{}", path, separator, existing),
            );
        }
    }

    /// Inject bundled toolchains into an environment variable map (for sandbox/shell executions).
    pub fn inject_toolchain_paths(workspace_root: &Path, env_map: &mut HashMap<String, String>) {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        if !toolchains_dir.exists() {
            return;
        }

        let mut bin_paths = Vec::new();
        if let Ok(entries) = fs::read_dir(&toolchains_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == "applesdk" && !Self::is_genuine_apple_developer_dir(&entry.path()) {
                    continue;
                }
                let bin = entry.path().join("bin");
                if bin.is_dir() {
                    bin_paths.push(bin.to_string_lossy().to_string());
                }
                #[cfg(windows)]
                {
                    let scripts = entry.path().join("Scripts");
                    if scripts.is_dir() {
                        bin_paths.push(scripts.to_string_lossy().to_string());
                    }
                }
            }
        }
        bin_paths.sort();

        if !bin_paths.is_empty() {
            #[cfg(windows)]
            if let Some(val) = env_map.remove("Path") {
                if !env_map.contains_key("PATH") {
                    env_map.insert("PATH".into(), val);
                }
            }

            let separator = if cfg!(windows) { ";" } else { ":" };
            for bin in bin_paths.into_iter().rev() {
                Self::prepend_path_var(env_map, "PATH", &bin, separator);
            }
        }

        if toolchains_dir.join("rust").exists() {
            env_map.insert(
                "RUSTUP_HOME".into(),
                toolchains_dir.join("rust").to_string_lossy().to_string(),
            );
            if !env_map.contains_key("CARGO_HOME") {
                env_map.insert(
                    "CARGO_HOME".into(),
                    workspace_root.join(".cargo").to_string_lossy().to_string(),
                );
            }
            let rust_lib = toolchains_dir.join("rust/lib");
            if rust_lib.is_dir() {
                let rust_lib_str = rust_lib.to_string_lossy().to_string();
                #[cfg(target_os = "macos")]
                Self::prepend_path_var(env_map, "DYLD_LIBRARY_PATH", &rust_lib_str, ":");
                #[cfg(unix)]
                Self::prepend_path_var(env_map, "LD_LIBRARY_PATH", &rust_lib_str, ":");
            }
        }

        if toolchains_dir.join("go").exists() {
            let go_dir = toolchains_dir.join("go");
            let has_go_bin = Self::find_binary(&go_dir, "bin/go").is_some();
            let has_go_pkg_or_src = go_dir.join("pkg").is_dir() || go_dir.join("src").is_dir();
            if has_go_bin || has_go_pkg_or_src {
                env_map.insert("GOROOT".into(), go_dir.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("python").exists() {
            let py_dir = toolchains_dir.join("python");
            if Self::find_binary(&py_dir, "bin/python3").is_some()
                || Self::find_binary(&py_dir, "bin/python").is_some()
                || py_dir.join("python.exe").is_file()
                || py_dir.join("bin").is_dir()
            {
                env_map.remove("PYTHONHOME");
                env_map.insert("PYTHONNOUSERSITE".into(), "1".into());
                env_map.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
                let py_lib = py_dir.join("lib");
                if py_lib.is_dir() {
                    let py_lib_str = py_lib.to_string_lossy().to_string();
                    #[cfg(target_os = "macos")]
                    Self::prepend_path_var(env_map, "DYLD_LIBRARY_PATH", &py_lib_str, ":");
                    #[cfg(unix)]
                    Self::prepend_path_var(env_map, "LD_LIBRARY_PATH", &py_lib_str, ":");
                }
            }
        }

        if toolchains_dir.join("node").exists() {
            let node_modules = toolchains_dir.join("node/lib/node_modules");
            if node_modules.is_dir() {
                let separator = if cfg!(windows) { ";" } else { ":" };
                Self::prepend_path_var(
                    env_map,
                    "NODE_PATH",
                    &node_modules.to_string_lossy(),
                    separator,
                );
            }
        }

        if toolchains_dir.join("php").exists() {
            let php_dir = toolchains_dir.join("php");
            if Self::find_binary(&php_dir, "bin/php").is_some() || php_dir.join("php.ini").exists()
            {
                env_map.insert("PHPRC".into(), php_dir.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("ruby").exists() {
            let ruby_dir = toolchains_dir.join("ruby");
            if Self::find_binary(&ruby_dir, "bin/ruby").is_some()
                || Self::find_binary(&ruby_dir, "bin/bundle").is_some()
                || ruby_dir.join("gems").is_dir()
            {
                env_map.insert("GEM_HOME".into(), ruby_dir.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("cc").exists() {
            let cc_dir = toolchains_dir.join("cc");
            let cc_bin = Self::find_binary(&cc_dir, "bin/clang");
            let gcc_bin = Self::find_binary(&cc_dir, "bin/gcc");
            let generic_cc = Self::find_binary(&cc_dir, "bin/cc");

            if let Some(bin) = cc_bin {
                env_map.insert("CC".into(), bin.to_string_lossy().to_string());
                if let Some(cxx) = Self::find_binary(&cc_dir, "bin/clang++") {
                    env_map.insert("CXX".into(), cxx.to_string_lossy().to_string());
                }
            } else if let Some(bin) = gcc_bin {
                env_map.insert("CC".into(), bin.to_string_lossy().to_string());
                if let Some(gxx) = Self::find_binary(&cc_dir, "bin/g++") {
                    env_map.insert("CXX".into(), gxx.to_string_lossy().to_string());
                }
            } else if let Some(bin) = generic_cc {
                env_map.insert("CC".into(), bin.to_string_lossy().to_string());
            }

            let llvm_ar = Self::find_binary(&cc_dir, "bin/llvm-ar");
            let ar_bin = Self::find_binary(&cc_dir, "bin/ar");
            if let Some(bin) = llvm_ar {
                env_map.insert("AR".into(), bin.to_string_lossy().to_string());
            } else if let Some(bin) = ar_bin {
                env_map.insert("AR".into(), bin.to_string_lossy().to_string());
            }

            let llvm_ranlib = Self::find_binary(&cc_dir, "bin/llvm-ranlib");
            let ranlib_bin = Self::find_binary(&cc_dir, "bin/ranlib");
            if let Some(bin) = llvm_ranlib {
                env_map.insert("RANLIB".into(), bin.to_string_lossy().to_string());
            } else if let Some(bin) = ranlib_bin {
                env_map.insert("RANLIB".into(), bin.to_string_lossy().to_string());
            }

            let cc_lib = cc_dir.join("lib");
            if cc_lib.is_dir() {
                let cc_lib_str = cc_lib.to_string_lossy().to_string();
                #[cfg(target_os = "macos")]
                Self::prepend_path_var(env_map, "DYLD_LIBRARY_PATH", &cc_lib_str, ":");
                #[cfg(unix)]
                Self::prepend_path_var(env_map, "LD_LIBRARY_PATH", &cc_lib_str, ":");
            }
        }

        if toolchains_dir.join("cmake").exists() {
            let cmake_dir = toolchains_dir.join("cmake");
            if let Some(cmake_bin) = Self::find_binary(&cmake_dir, "bin/cmake") {
                env_map.insert("CMAKE".into(), cmake_bin.to_string_lossy().to_string());
            }
            let share_dir = cmake_dir.join("share");
            if share_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&share_dir) {
                    for entry in entries.flatten() {
                        let fname = entry.file_name();
                        if fname.to_string_lossy().starts_with("cmake") && entry.path().is_dir() {
                            env_map.insert(
                                "CMAKE_ROOT".into(),
                                entry.path().to_string_lossy().to_string(),
                            );
                            break;
                        }
                    }
                }
            }
        }

        let dmz_toolchain_cmake = toolchains_dir.join("dmz.toolchain.cmake");
        let has_dmz_toolchain = dmz_toolchain_cmake.exists();
        if has_dmz_toolchain
            && (toolchains_dir.join("cmake").exists()
                || workspace_root.join("CMakeLists.txt").exists())
        {
            let tc_str = dmz_toolchain_cmake.to_string_lossy().to_string();
            env_map.insert("CMAKE_TOOLCHAIN_FILE".into(), tc_str.clone());
            env_map.insert("HOST_CMAKE_TOOLCHAIN_FILE".into(), tc_str.clone());
            env_map.insert("TARGET_CMAKE_TOOLCHAIN_FILE".into(), tc_str);
        }

        if toolchains_dir.join("ninja").exists() {
            let ninja_dir = toolchains_dir.join("ninja");
            if let Some(ninja_bin) = Self::find_binary(&ninja_dir, "bin/ninja") {
                env_map.insert("NINJA".into(), ninja_bin.to_string_lossy().to_string());
                env_map.insert("CMAKE_GENERATOR".into(), "Ninja".into());
            }
        }

        if toolchains_dir.join("make").exists() {
            let make_dir = toolchains_dir.join("make");
            let make_bin = Self::find_binary(&make_dir, "bin/make")
                .or_else(|| Self::find_binary(&make_dir, "bin/mingw32-make"));
            if let Some(bin) = make_bin {
                env_map.insert("MAKE".into(), bin.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("juce").exists() {
            env_map.insert(
                "JUCE_PATH".into(),
                toolchains_dir.join("juce").to_string_lossy().to_string(),
            );
        }

        if toolchains_dir.join("androidndk").exists() {
            let ndk_dir = toolchains_dir.join("androidndk");
            let ndk_str = ndk_dir.to_string_lossy().to_string();
            env_map.insert("ANDROID_NDK_HOME".into(), ndk_str.clone());
            env_map.insert("ANDROID_NDK_ROOT".into(), ndk_str);
            let cmake_tc = ndk_dir.join("build/cmake/android.toolchain.cmake");
            if cmake_tc.exists() && !has_dmz_toolchain {
                let cmake_tc_str = cmake_tc.to_string_lossy().to_string();
                env_map.insert("CMAKE_TOOLCHAIN_FILE".into(), cmake_tc_str.clone());
                env_map.insert("HOST_CMAKE_TOOLCHAIN_FILE".into(), cmake_tc_str.clone());
                env_map.insert("TARGET_CMAKE_TOOLCHAIN_FILE".into(), cmake_tc_str);
            }
        }

        if toolchains_dir.join("applesdk").exists() {
            let apple_dir = toolchains_dir.join("applesdk");
            let is_genuine = Self::is_genuine_apple_developer_dir(&apple_dir);
            if is_genuine {
                env_map.insert(
                    "DEVELOPER_DIR".into(),
                    apple_dir.to_string_lossy().to_string(),
                );
            }

            // Export SDKROOT when .sdk directory is located
            if let Some(sdk_path) = Self::find_apple_sdk_root(&apple_dir) {
                env_map.insert("SDKROOT".into(), sdk_path.to_string_lossy().to_string());
                if is_genuine && !env_map.contains_key("DEVELOPER_DIR") {
                    env_map.insert(
                        "DEVELOPER_DIR".into(),
                        apple_dir.to_string_lossy().to_string(),
                    );
                }
            }
        }

        if toolchains_dir.join("pkgconfig").exists() {
            let pkg_dir = toolchains_dir.join("pkgconfig");
            let pkg_bin = Self::find_binary(&pkg_dir, "bin/pkg-config")
                .or_else(|| Self::find_binary(&pkg_dir, "bin/pkgconf"));
            if let Some(bin) = pkg_bin {
                env_map.insert("PKG_CONFIG".into(), bin.to_string_lossy().to_string());
            }

            let pc_lib = toolchains_dir.join("pkgconfig/lib/pkgconfig");
            let pc_share = toolchains_dir.join("pkgconfig/share/pkgconfig");
            let mut pc_paths = Vec::new();
            if pc_lib.is_dir() {
                pc_paths.push(pc_lib.to_string_lossy().to_string());
            }
            if pc_share.is_dir() {
                pc_paths.push(pc_share.to_string_lossy().to_string());
            }
            if !pc_paths.is_empty() {
                let separator = if cfg!(windows) { ";" } else { ":" };
                for pc_path in pc_paths {
                    Self::prepend_path_var(env_map, "PKG_CONFIG_PATH", &pc_path, separator);
                }
            }
        }
    }

    /// Locate an executable binary within a toolchain directory, checking both extensionless (Unix)
    /// and `.exe` (Windows) paths.
    pub fn find_binary(dir: &Path, rel_path: &str) -> Option<std::path::PathBuf> {
        let p = dir.join(rel_path);
        if p.exists() {
            return Some(p);
        }
        let exe = dir.join(format!("{}.exe", rel_path));
        if exe.exists() {
            return Some(exe);
        }
        None
    }

    /// Check whether the directory is a genuine, authentic Apple Xcode, Command Line Tools root, or SDK distribution.
    /// Apple toolchain specifications: (`man 1 xcrun`), DEVELOPER_DIR must point to a directory
    /// containing developer tools (such as `usr/bin/xcrun` or `bin/xcrun`) and platform SDKs (`SDKs` or `Platforms`).
    pub fn is_genuine_apple_developer_dir(apple_dir: &Path) -> bool {
        let has_xcrun =
            apple_dir.join("usr/bin/xcrun").exists() || apple_dir.join("bin/xcrun").exists();
        let has_sdk_structure = apple_dir.join("Platforms").is_dir()
            || apple_dir.join("SDKs").is_dir()
            || apple_dir.join("Toolchains").is_dir();

        (has_xcrun || has_sdk_structure) && Self::find_apple_sdk_root(apple_dir).is_some()
    }

    /// Search for Apple SDK directory (*.sdk) within candidate subpaths.
    pub fn find_apple_sdk_root(apple_dir: &Path) -> Option<std::path::PathBuf> {
        if apple_dir.is_dir() && apple_dir.extension().is_some_and(|ext| ext == "sdk") {
            return Some(apple_dir.to_path_buf());
        }
        let sdk_candidates = [
            apple_dir.join("Platforms/MacOSX.platform/Developer/SDKs"),
            apple_dir.join("SDKs"),
        ];
        for parent in &sdk_candidates {
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() && p.extension().is_some_and(|ext| ext == "sdk") {
                        return Some(p);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inject_toolchain_paths_empty_path_avoids_trailing_separator() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let rust_bin = ws.join(".dmz/toolchains/rust/bin");
        fs::create_dir_all(&rust_bin).unwrap();
        fs::write(rust_bin.join("cargo"), "cargo").unwrap();

        let mut env_map = HashMap::new();
        env_map.insert("PATH".to_string(), String::new());

        ToolchainEnv::inject_toolchain_paths(ws, &mut env_map);

        let path = env_map.get("PATH").unwrap();
        assert!(!path.is_empty());
        assert!(
            !path.ends_with(':'),
            "Must not have trailing colon (CWE-426)"
        );
        assert!(
            !path.ends_with(';'),
            "Must not have trailing semicolon (CWE-426)"
        );
        assert!(path.replace('\\', "/").contains(".dmz/toolchains/rust/bin"));
    }

    #[test]
    fn test_inject_toolchain_paths_ecosystem_configurations() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let tc = ws.join(".dmz/toolchains");

        // Rust
        fs::create_dir_all(tc.join("rust/bin")).unwrap();
        // Go
        fs::create_dir_all(tc.join("go/bin")).unwrap();
        fs::write(tc.join("go/bin/go"), "go").unwrap();
        // Node
        fs::create_dir_all(tc.join("node/lib/node_modules")).unwrap();
        // CC
        fs::create_dir_all(tc.join("cc/bin")).unwrap();
        fs::write(tc.join("cc/bin/clang"), "clang").unwrap();
        fs::write(tc.join("cc/bin/clang++"), "clang++").unwrap();
        fs::write(tc.join("cc/bin/llvm-ar"), "llvm-ar").unwrap();
        fs::write(tc.join("cc/bin/llvm-ranlib"), "llvm-ranlib").unwrap();
        // CMake & Ninja
        fs::create_dir_all(tc.join("cmake/bin")).unwrap();
        fs::write(tc.join("cmake/bin/cmake"), "cmake").unwrap();
        fs::create_dir_all(tc.join("ninja/bin")).unwrap();
        fs::write(tc.join("ninja/bin/ninja"), "ninja").unwrap();
        // PkgConfig
        fs::create_dir_all(tc.join("pkgconfig/bin")).unwrap();
        fs::write(tc.join("pkgconfig/bin/pkg-config"), "pkg-config").unwrap();
        fs::create_dir_all(tc.join("pkgconfig/lib/pkgconfig")).unwrap();

        let mut env_map = HashMap::new();
        env_map.insert("PATH".to_string(), "/usr/bin".to_string());

        ToolchainEnv::inject_toolchain_paths(ws, &mut env_map);

        assert_eq!(
            Path::new(env_map.get("RUSTUP_HOME").unwrap()),
            tc.join("rust").as_path()
        );
        assert_eq!(
            Path::new(env_map.get("GOROOT").unwrap()),
            tc.join("go").as_path()
        );
        assert!(env_map
            .get("NODE_PATH")
            .unwrap()
            .replace('\\', "/")
            .contains("node/lib/node_modules"));
        assert_eq!(
            Path::new(env_map.get("CC").unwrap()),
            tc.join("cc/bin/clang").as_path()
        );
        assert_eq!(
            Path::new(env_map.get("CXX").unwrap()),
            tc.join("cc/bin/clang++").as_path()
        );
        assert_eq!(
            Path::new(env_map.get("AR").unwrap()),
            tc.join("cc/bin/llvm-ar").as_path()
        );
        assert_eq!(
            Path::new(env_map.get("RANLIB").unwrap()),
            tc.join("cc/bin/llvm-ranlib").as_path()
        );
        assert_eq!(
            Path::new(env_map.get("CMAKE").unwrap()),
            tc.join("cmake/bin/cmake").as_path()
        );
        assert_eq!(
            Path::new(env_map.get("NINJA").unwrap()),
            tc.join("ninja/bin/ninja").as_path()
        );
        assert_eq!(env_map.get("CMAKE_GENERATOR").unwrap(), "Ninja");
        assert_eq!(
            Path::new(env_map.get("PKG_CONFIG").unwrap()),
            tc.join("pkgconfig/bin/pkg-config").as_path()
        );
        assert!(env_map
            .get("PKG_CONFIG_PATH")
            .unwrap()
            .replace('\\', "/")
            .contains("pkgconfig/lib/pkgconfig"));
    }

    #[test]
    fn test_python_home_poisoning_prevention() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        fs::create_dir_all(ws.join(".dmz/toolchains/python/bin")).unwrap();
        fs::write(ws.join(".dmz/toolchains/python/bin/python3"), "py").unwrap();

        let mut env_map = HashMap::new();
        env_map.insert(
            "PYTHONHOME".to_string(),
            "/usr/local/Cellar/python".to_string(),
        );

        ToolchainEnv::inject_toolchain_paths(ws, &mut env_map);

        assert!(
            !env_map.contains_key("PYTHONHOME"),
            "PYTHONHOME must be removed"
        );
        assert_eq!(env_map.get("PYTHONNOUSERSITE").unwrap(), "1");
        assert_eq!(env_map.get("PYTHONDONTWRITEBYTECODE").unwrap(), "1");
    }

    #[test]
    fn test_find_binary_platform_extensions() {
        let temp = tempdir().unwrap();
        let dir = temp.path();

        let bin_sub = dir.join("bin");
        fs::create_dir_all(&bin_sub).unwrap();

        // Direct unix path
        fs::write(bin_sub.join("custom-tool"), "exec").unwrap();
        let found = ToolchainEnv::find_binary(dir, "bin/custom-tool");
        assert!(found.is_some());
        assert_eq!(found.unwrap(), bin_sub.join("custom-tool"));

        // Windows exe extension
        fs::write(bin_sub.join("win-tool.exe"), "exec").unwrap();
        let found_win = ToolchainEnv::find_binary(dir, "bin/win-tool");
        assert!(found_win.is_some());
        assert_eq!(found_win.unwrap(), bin_sub.join("win-tool.exe"));

        // Non-existent
        assert!(ToolchainEnv::find_binary(dir, "bin/missing").is_none());
    }

    #[test]
    fn test_apple_sdk_and_developer_dir_detection() {
        let temp = tempdir().unwrap();
        let apple_dir = temp.path().join("applesdk");

        // 1. Incomplete / non-genuine directory
        fs::create_dir_all(apple_dir.join("bin")).unwrap();
        fs::write(apple_dir.join("bin/xcrun"), "fake").unwrap();
        assert!(!ToolchainEnv::is_genuine_apple_developer_dir(&apple_dir));
        assert!(ToolchainEnv::find_apple_sdk_root(&apple_dir).is_none());

        // 2. Genuine directory with SDK
        let sdk_dir = apple_dir.join("Platforms/MacOSX.platform/Developer/SDKs/MacOSX14.0.sdk");
        fs::create_dir_all(&sdk_dir).unwrap();
        assert!(ToolchainEnv::is_genuine_apple_developer_dir(&apple_dir));
        assert_eq!(
            ToolchainEnv::find_apple_sdk_root(&apple_dir).unwrap(),
            sdk_dir
        );
    }
}
