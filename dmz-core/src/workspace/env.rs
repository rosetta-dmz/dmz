// Hermetic runtime environment variable generation and binary resolution.
// Prepares process execution environments, injecting bundled toolchains,
// node_modules binaries, virtualenvs, and network isolation flags.

use super::schema::WorkspaceProjectConfig;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Returns the canonical Rust target triple for the current host platform.
pub fn host_target_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => "unknown-target",
    }
}

/// Resolves an executable command name against the provided `PATH` string and working directory.
pub fn resolve_executable_in_env(cmd_name: &str, path_env: &str, cwd: &Path) -> String {
    let cmd_path = Path::new(cmd_name);
    if cmd_path.is_absolute() || cmd_name.contains('/') || cmd_name.contains('\\') {
        let candidate = if cmd_path.is_absolute() {
            cmd_path.to_path_buf()
        } else {
            cwd.join(cmd_path)
        };
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
        return cmd_name.to_string();
    }

    let sep = if cfg!(windows) { ';' } else { ':' };
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ""]
    } else {
        &[""]
    };

    for dir_str in path_env.split(sep) {
        if dir_str.is_empty() {
            continue;
        }
        let dir = Path::new(dir_str);
        for ext in exts {
            let candidate = dir.join(format!("{}{}", cmd_name, ext));
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    cmd_name.to_string()
}

/// Builds a hermetic runtime environment variable map for a workspace member.
///
/// # Async Runtime Note
/// Performs synchronous filesystem checks for toolchains and virtualenvs.
/// When invoking from an asynchronous Tokio runtime context, offload via `tokio::task::spawn_blocking`.
pub fn build_member_env(
    config: &WorkspaceProjectConfig,
    workspace_root: &Path,
    member_dir: &Path,
    sock_path: &Path,
    allow_network: bool,
) -> HashMap<String, String> {
    let mut env_map: HashMap<String, String> = std::env::vars().collect();
    let separator = if cfg!(windows) { ";" } else { ":" };

    // 1. Delegate bundled toolchain injection to single source of truth
    crate::toolchain::ToolchainEnv::inject_toolchain_paths(workspace_root, &mut env_map);
    if member_dir != workspace_root {
        crate::toolchain::ToolchainEnv::inject_toolchain_paths(member_dir, &mut env_map);
    }

    // 2. Prepend member and root node_modules/.bin to PATH and configure NODE_PATH
    let mut current_path = env_map
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());

    inject_node_env(
        member_dir,
        workspace_root,
        &mut env_map,
        &mut current_path,
        separator,
    );
    inject_python_env(
        member_dir,
        workspace_root,
        &mut env_map,
        &mut current_path,
        separator,
    );

    env_map.insert("PATH".to_string(), current_path);
    env_map.remove("PYTHONHOME");
    env_map.insert(
        "DMZ_WORKSPACE".to_string(),
        workspace_root.display().to_string(),
    );
    env_map.insert(
        "RUSTC_HOST_TRIPLE".to_string(),
        host_target_triple().to_string(),
    );

    // 3. Inject IPC socket environment variables when a socket path is provided
    if !sock_path.as_os_str().is_empty() {
        let sock_str = sock_path.to_string_lossy().to_string();
        env_map.insert(config.ipc.env_var.clone(), sock_str.clone());
        env_map.insert("DMZ_SIDECAR_SOCK".to_string(), sock_str);
    }

    // 4. Offline enforcement variables when network is disallowed
    if !allow_network {
        inject_offline_env(&mut env_map);
    }

    env_map
}

pub fn inject_node_env(
    member_dir: &Path,
    workspace_root: &Path,
    env_map: &mut HashMap<String, String>,
    current_path: &mut String,
    separator: &str,
) {
    for base in [member_dir, workspace_root] {
        let node_bin = base.join("node_modules/.bin");
        if node_bin.is_dir() {
            let nb_str = node_bin.to_string_lossy().to_string();
            if !current_path.contains(&nb_str) {
                *current_path = if current_path.is_empty() {
                    nb_str
                } else {
                    format!("{}{}{}", nb_str, separator, current_path)
                };
            }
        }
    }

    let member_nm = member_dir.join("node_modules");
    if member_nm.is_dir() {
        let nm_str = member_nm.to_string_lossy().to_string();
        let existing = env_map.get("NODE_PATH").cloned().unwrap_or_default();
        env_map.insert(
            "NODE_PATH".to_string(),
            if existing.is_empty() {
                nm_str
            } else {
                format!("{}{}{}", nm_str, separator, existing)
            },
        );
    }
}

pub fn inject_python_env(
    member_dir: &Path,
    workspace_root: &Path,
    env_map: &mut HashMap<String, String>,
    current_path: &mut String,
    separator: &str,
) {
    let venv_candidates = [
        member_dir.join(".venv"),
        member_dir.join(".dmz/venv"),
        workspace_root.join(".venv"),
        workspace_root.join(".dmz/venv"),
    ];
    if let Some(venv) = venv_candidates.iter().find(|p| p.is_dir()) {
        env_map.insert(
            "VIRTUAL_ENV".to_string(),
            venv.to_string_lossy().to_string(),
        );
        let venv_bin = if cfg!(windows) {
            venv.join("Scripts")
        } else {
            venv.join("bin")
        };
        if venv_bin.is_dir() {
            let vb_str = venv_bin.to_string_lossy().to_string();
            if !current_path.contains(&vb_str) {
                *current_path = format!("{}{}{}", vb_str, separator, current_path);
            }
        }

        let mut site_packages = None;
        let lib_dir = venv.join("lib");
        if lib_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&lib_dir) {
                for entry in entries.flatten() {
                    let sp = entry.path().join("site-packages");
                    if sp.is_dir() {
                        site_packages = Some(sp);
                        break;
                    }
                }
            }
        }
        if site_packages.is_none() {
            let win_sp = venv.join("Lib/site-packages");
            if win_sp.is_dir() {
                site_packages = Some(win_sp);
            }
        }
        if let Some(sp) = site_packages {
            let sp_str = sp.to_string_lossy().to_string();
            let existing_py = env_map.get("PYTHONPATH").cloned().unwrap_or_default();
            env_map.insert(
                "PYTHONPATH".to_string(),
                if existing_py.is_empty() {
                    sp_str
                } else {
                    format!("{}{}{}", sp_str, separator, existing_py)
                },
            );
        }
    }
}

pub fn inject_offline_env(env_map: &mut HashMap<String, String>) {
    env_map.insert("CARGO_NET_OFFLINE".to_string(), "true".to_string());
    env_map.insert("PIP_NO_INDEX".to_string(), "1".to_string());
    env_map.insert("GOPROXY".to_string(), "off".to_string());
    env_map.insert("npm_config_offline".to_string(), "true".to_string());
    env_map.insert("YARN_OFFLINE".to_string(), "true".to_string());
    env_map.insert("PNPM_OFFLINE".to_string(), "true".to_string());
    env_map.insert("COMPOSER_DISABLE_NETWORK".to_string(), "1".to_string());
}

impl WorkspaceProjectConfig {
    pub fn build_member_env(
        &self,
        workspace_root: &Path,
        member_dir: &Path,
        sock_path: &Path,
        allow_network: bool,
    ) -> HashMap<String, String> {
        build_member_env(self, workspace_root, member_dir, sock_path, allow_network)
    }

    pub fn host_target_triple() -> &'static str {
        host_target_triple()
    }

    pub fn resolve_executable_in_env(cmd_name: &str, path_env: &str, cwd: &Path) -> String {
        resolve_executable_in_env(cmd_name, path_env, cwd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_target_triple_valid() {
        let triple = host_target_triple();
        assert!(!triple.is_empty());
        assert_ne!(triple, "unknown-target");
    }

    #[test]
    fn test_inject_offline_env() {
        let mut env = HashMap::new();
        inject_offline_env(&mut env);
        assert_eq!(env.get("CARGO_NET_OFFLINE").map(String::as_str), Some("true"));
        assert_eq!(env.get("PIP_NO_INDEX").map(String::as_str), Some("1"));
        assert_eq!(env.get("GOPROXY").map(String::as_str), Some("off"));
        assert_eq!(env.get("npm_config_offline").map(String::as_str), Some("true"));
    }

    #[test]
    fn test_resolve_executable_in_env() {
        let cwd = Path::new(".");
        let resolved = resolve_executable_in_env("nonexistent_binary_xyz_123", "", cwd);
        assert_eq!(resolved, "nonexistent_binary_xyz_123");
    }

    #[test]
    fn test_build_member_env_offline_flag() {
        let config = WorkspaceProjectConfig::default();
        let env = build_member_env(&config, Path::new("."), Path::new("."), Path::new(""), false);
        assert_eq!(env.get("CARGO_NET_OFFLINE").map(String::as_str), Some("true"));
    }
}

