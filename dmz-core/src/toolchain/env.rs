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
            env_map.insert(
                "GOROOT".into(),
                toolchains_dir.join("go").to_string_lossy().to_string(),
            );
        }

        if toolchains_dir.join("python").exists() {
            env_map.insert(
                "PYTHONHOME".into(),
                toolchains_dir.join("python").to_string_lossy().to_string(),
            );
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
            env_map.insert(
                "PHPRC".into(),
                toolchains_dir.join("php").to_string_lossy().to_string(),
            );
        }

        if toolchains_dir.join("ruby").exists() {
            env_map.insert(
                "GEM_HOME".into(),
                toolchains_dir.join("ruby").to_string_lossy().to_string(),
            );
        }

        if toolchains_dir.join("cc").exists() {
            let cc_dir = toolchains_dir.join("cc");
            let cc_bin = cc_dir.join("bin/clang");
            let gcc_bin = cc_dir.join("bin/gcc");
            let generic_cc = cc_dir.join("bin/cc");

            if cc_bin.exists() {
                env_map.insert("CC".into(), cc_bin.to_string_lossy().to_string());
                let cxx = cc_dir.join("bin/clang++");
                if cxx.exists() {
                    env_map.insert("CXX".into(), cxx.to_string_lossy().to_string());
                }
            } else if gcc_bin.exists() {
                env_map.insert("CC".into(), gcc_bin.to_string_lossy().to_string());
                let gxx = cc_dir.join("bin/g++");
                if gxx.exists() {
                    env_map.insert("CXX".into(), gxx.to_string_lossy().to_string());
                }
            } else if generic_cc.exists() {
                env_map.insert("CC".into(), generic_cc.to_string_lossy().to_string());
            }

            let llvm_ar = cc_dir.join("bin/llvm-ar");
            let ar_bin = cc_dir.join("bin/ar");
            if llvm_ar.exists() {
                env_map.insert("AR".into(), llvm_ar.to_string_lossy().to_string());
            } else if ar_bin.exists() {
                env_map.insert("AR".into(), ar_bin.to_string_lossy().to_string());
            }

            let llvm_ranlib = cc_dir.join("bin/llvm-ranlib");
            let ranlib_bin = cc_dir.join("bin/ranlib");
            if llvm_ranlib.exists() {
                env_map.insert("RANLIB".into(), llvm_ranlib.to_string_lossy().to_string());
            } else if ranlib_bin.exists() {
                env_map.insert("RANLIB".into(), ranlib_bin.to_string_lossy().to_string());
            }
        }

        if toolchains_dir.join("cmake").exists() {
            let cmake_dir = toolchains_dir.join("cmake");
            let cmake_bin = cmake_dir.join("bin/cmake");
            if cmake_bin.exists() {
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
            let ninja_bin = toolchains_dir.join("ninja/bin/ninja");
            if ninja_bin.exists() {
                env_map.insert("NINJA".into(), ninja_bin.to_string_lossy().to_string());
                env_map.insert("CMAKE_GENERATOR".into(), "Ninja".into());
            }
        }

        if toolchains_dir.join("make").exists() {
            let make_bin = toolchains_dir.join("make/bin/make");
            if make_bin.exists() {
                env_map.insert("MAKE".into(), make_bin.to_string_lossy().to_string());
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
            let apple_str = apple_dir.to_string_lossy().to_string();
            env_map.insert("DEVELOPER_DIR".into(), apple_str.clone());

            let mut found_sdk = false;
            let sdk_candidates = [
                apple_dir.join("Platforms/MacOSX.platform/Developer/SDKs"),
                apple_dir.join("SDKs"),
            ];
            for parent in &sdk_candidates {
                if let Ok(entries) = fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_dir() && p.extension().is_some_and(|ext| ext == "sdk") {
                            env_map.insert("SDKROOT".into(), p.to_string_lossy().to_string());
                            found_sdk = true;
                            break;
                        }
                    }
                }
                if found_sdk {
                    break;
                }
            }
            if !found_sdk {
                env_map.insert("SDKROOT".into(), apple_str);
            }
        }

        if toolchains_dir.join("pkgconfig").exists() {
            let pkg_bin = toolchains_dir.join("pkgconfig/bin/pkg-config");
            if pkg_bin.exists() {
                env_map.insert("PKG_CONFIG".into(), pkg_bin.to_string_lossy().to_string());
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
}
