use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Injects bundled toolchain binaries and environment variables into runtime execution environments.
pub struct ToolchainEnv;

impl ToolchainEnv {
    /// Injects bundled toolchains into an environment variable map (for sandbox/shell executions).
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
                if bin.exists() {
                    bin_paths.push(bin.to_string_lossy().to_string());
                }
            }
        }

        if !bin_paths.is_empty() {
            let existing_path = env_map
                .get("PATH")
                .cloned()
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
            let separator = if cfg!(windows) { ";" } else { ":" };
            let new_path = format!(
                "{}{}{}",
                bin_paths.join(separator),
                separator,
                existing_path
            );
            env_map.insert("PATH".into(), new_path);
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
            #[cfg(target_os = "macos")]
            {
                let rust_lib = toolchains_dir
                    .join("rust/lib")
                    .to_string_lossy()
                    .to_string();
                let existing = env_map
                    .get("DYLD_LIBRARY_PATH")
                    .cloned()
                    .unwrap_or_default();
                env_map.insert(
                    "DYLD_LIBRARY_PATH".into(),
                    if existing.is_empty() {
                        rust_lib
                    } else {
                        format!("{}:{}", rust_lib, existing)
                    },
                );
            }
            #[cfg(unix)]
            {
                let rust_lib = toolchains_dir
                    .join("rust/lib")
                    .to_string_lossy()
                    .to_string();
                let existing = env_map.get("LD_LIBRARY_PATH").cloned().unwrap_or_default();
                env_map.insert(
                    "LD_LIBRARY_PATH".into(),
                    if existing.is_empty() {
                        rust_lib
                    } else {
                        format!("{}:{}", rust_lib, existing)
                    },
                );
            }
        }

        if toolchains_dir.join("go").exists() {
            let go_dir = toolchains_dir.join("go");
            let has_go_bin = go_dir.join("bin/go").exists() || go_dir.join("bin/go.exe").exists();
            let has_go_pkg_or_src = go_dir.join("pkg").is_dir() || go_dir.join("src").is_dir();
            if has_go_bin || has_go_pkg_or_src {
                env_map.insert("GOROOT".into(), go_dir.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("python").exists() {
            let py_dir = toolchains_dir.join("python");
            let has_py_bin = py_dir.join("bin/python3").exists()
                || py_dir.join("bin/python").exists()
                || py_dir.join("bin/pip").exists()
                || py_dir.join("python.exe").exists();
            let has_stdlib = py_dir.join("lib").exists() || py_dir.join("Lib").exists();
            if has_py_bin && has_stdlib {
                env_map.insert("PYTHONHOME".into(), py_dir.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("node").exists() {
            let node_modules = toolchains_dir.join("node/lib/node_modules");
            if node_modules.is_dir() {
                let existing = env_map.get("NODE_PATH").cloned().unwrap_or_default();
                let separator = if cfg!(windows) { ";" } else { ":" };
                let nm_str = node_modules.to_string_lossy().to_string();
                let new_val = if existing.is_empty() {
                    nm_str
                } else {
                    format!("{}{}{}", nm_str, separator, existing)
                };
                env_map.insert("NODE_PATH".into(), new_val);
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
                        if fname.to_string_lossy().starts_with("cmake") {
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

            // Only export DEVELOPER_DIR if it is a genuine, verified Xcode or Command Line Tools installation with xcrun.
            // Per Apple toolchain specifications (man 1 xcrun), pointing DEVELOPER_DIR to an arbitrary directory
            // containing only loose binaries breaks xcrun and causes /usr/bin/cc to exit with status 1.
            let has_xcrun =
                apple_dir.join("usr/bin/xcrun").exists() || apple_dir.join("bin/xcrun").exists();
            if has_xcrun && Self::is_genuine_apple_developer_dir(&apple_dir) {
                env_map.insert(
                    "DEVELOPER_DIR".into(),
                    apple_dir.to_string_lossy().to_string(),
                );
            }

            // Always export SDKROOT if an authentic .sdk directory is located; never point to a bare non-SDK directory.
            if let Some(sdk_path) = Self::find_apple_sdk_root(&apple_dir) {
                env_map.insert("SDKROOT".into(), sdk_path.to_string_lossy().to_string());
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
                let existing = env_map.get("PKG_CONFIG_PATH").cloned().unwrap_or_default();
                let separator = if cfg!(windows) { ";" } else { ":" };
                let new_val = if existing.is_empty() {
                    pc_paths.join(separator)
                } else {
                    format!("{}{}{}", pc_paths.join(separator), separator, existing)
                };
                env_map.insert("PKG_CONFIG_PATH".into(), new_val);
            }
        }
    }

    /// Locates an executable binary within a toolchain directory, checking both extensionless (Unix)
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

    /// Checks whether the directory is a genuine, authentic Apple Xcode, Command Line Tools root, or SDK distribution.
    ///
    /// Per Apple toolchain specifications (`man 1 xcrun`), DEVELOPER_DIR must point to a directory
    /// containing developer tools (such as `usr/bin/xcrun` or `bin/xcrun`) and platform SDKs (`SDKs` or `Platforms`).
    pub fn is_genuine_apple_developer_dir(apple_dir: &Path) -> bool {
        let has_xcrun =
            apple_dir.join("usr/bin/xcrun").exists() || apple_dir.join("bin/xcrun").exists();
        let has_sdk_structure = apple_dir.join("Platforms").is_dir()
            || apple_dir.join("SDKs").is_dir()
            || apple_dir.join("Toolchains").is_dir();

        (has_xcrun || has_sdk_structure) && Self::find_apple_sdk_root(apple_dir).is_some()
    }

    /// Searches for an authentic Apple SDK directory (*.sdk) within candidate subpaths.
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
