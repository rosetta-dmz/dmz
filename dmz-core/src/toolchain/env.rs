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

        if toolchains_dir.join("juce").exists() {
            env_map.insert(
                "JUCE_PATH".into(),
                toolchains_dir.join("juce").to_string_lossy().to_string(),
            );
        }

        if toolchains_dir.join("pkgconfig").exists() {
            let pkg_bin = toolchains_dir.join("pkgconfig/bin/pkg-config");
            if pkg_bin.exists() {
                env_map.insert("PKG_CONFIG".into(), pkg_bin.to_string_lossy().to_string());
            }
        }
    }
}
