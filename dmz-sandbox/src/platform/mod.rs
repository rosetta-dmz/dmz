use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::generate_seatbelt_profile;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Debug)]
pub struct SandboxConfig {
    pub workspace_path: PathBuf,
    pub closure_path: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
    pub allow_network: bool,
    pub ipc_socket_path: Option<PathBuf>,
}

impl SandboxConfig {
    pub fn new(
        workspace_path: impl Into<PathBuf>,
        closure_path: impl Into<PathBuf>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            workspace_path: workspace_path.into(),
            closure_path: closure_path.into(),
            command: command.into(),
            args: Vec::new(),
            envs: Vec::new(),
            allow_network: false,
            ipc_socket_path: None,
        }
    }

    pub fn with_ipc_socket(mut self, socket_path: impl Into<PathBuf>) -> Self {
        self.ipc_socket_path = Some(socket_path.into());
        self
    }
}

#[derive(Debug)]
pub enum PlatformError {
    SeatbeltError(String),
    NamespaceError(String),
    WindowsError(String),
    PlatformFault(String),
    UnsupportedPlatform(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::SeatbeltError(msg) => write!(f, "Seatbelt error: {}", msg),
            PlatformError::NamespaceError(msg) => write!(f, "Namespace error: {}", msg),
            PlatformError::WindowsError(msg) => write!(f, "Windows error: {}", msg),
            PlatformError::PlatformFault(msg) => write!(f, "Platform fault: {}", msg),
            PlatformError::UnsupportedPlatform(msg) => write!(f, "Unsupported platform: {}", msg),
        }
    }
}

impl Error for PlatformError {}

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    // Resolve relative commands (e.g. "cargo", "node", "python") to absolute paths
    let absolute_command = if PathBuf::from(&config.command).is_absolute() {
        config.command.clone()
    } else {
        which::which(&config.command)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| config.command.clone())
    };

    let mut resolved_config = config.clone();
    resolved_config.command = absolute_command;

    // Universal Ecosystem Cache Redirection:
    // Force all build caches (Cargo, npm, pip, go, etc.) into an isolated scratch directory
    // inside the workspace (.dmz/cache) - (automatically writable under workspace permissions).
    let sandbox_cache_dir = resolved_config.workspace_path.join(".dmz").join("cache");
    let _ = std::fs::create_dir_all(&sandbox_cache_dir);

    let mut env_map: HashMap<String, String> = resolved_config.envs.into_iter().collect();

    // Delegate bundled toolchain environment configuration to single source of truth in dmz-core
    dmz_core::toolchain::ToolchainEnv::inject_toolchain_paths(
        &resolved_config.workspace_path,
        &mut env_map,
    );

    // Augment PATH with host fallback and user toolchain directories
    let separator = if cfg!(windows) { ";" } else { ":" };
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let default_path = if cfg!(windows) {
        "C:\\Windows\\System32;C:\\Windows".to_string()
    } else {
        "/usr/bin:/bin:/usr/sbin:/sbin".to_string()
    };
    let mut current_path = env_map
        .get("PATH")
        .cloned()
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or(default_path));

    let toolchain_bins = [
        format!("{}/.cargo/bin", home),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        format!("{}/.local/bin", home),
        format!("{}/go/bin", home),
        format!("{}/.pnpm", home),
        format!("{}/.local/share/pnpm", home),
    ];
    for bin in &toolchain_bins {
        if PathBuf::from(bin).exists() && !current_path.contains(bin) {
            current_path = format!("{}{}{}", bin, separator, current_path);
        }
    }

    // Workspace node_modules/.bin in PATH when it exists
    let node_bin = resolved_config
        .workspace_path
        .join("node_modules")
        .join(".bin");
    if node_bin.exists() {
        current_path = format!("{}{}{}", node_bin.display(), separator, current_path);
    }
    env_map.insert("PATH".into(), current_path);

    // Set NODE_PATH
    let node_modules = resolved_config.workspace_path.join("node_modules");
    if node_modules.exists() {
        env_map.insert(
            "NODE_PATH".into(),
            node_modules.to_string_lossy().to_string(),
        );
    }

    // Set PYTHONPATH and VIRTUAL_ENV when python environments exist
    let venv = resolved_config.workspace_path.join(".venv");
    let dmz_venv = resolved_config.workspace_path.join(".dmz").join("venv");
    let active_venv = if venv.exists() {
        Some(venv)
    } else if dmz_venv.exists() {
        Some(dmz_venv)
    } else {
        None
    };

    if let Some(ref v) = active_venv {
        env_map.insert("VIRTUAL_ENV".into(), v.to_string_lossy().to_string());

        let mut site_packages_found = None;
        let lib_dir = v.join("lib");
        if lib_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&lib_dir) {
                for entry in entries.flatten() {
                    let sp = entry.path().join("site-packages");
                    if sp.is_dir() {
                        site_packages_found = Some(sp);
                        break;
                    }
                }
            }
        }
        if site_packages_found.is_none() {
            let win_sp = v.join("Lib").join("site-packages");
            if win_sp.is_dir() {
                site_packages_found = Some(win_sp);
            }
        }

        if let Some(sp) = site_packages_found {
            env_map.insert("PYTHONPATH".into(), sp.to_string_lossy().to_string());
        }
    }

    // Universal Ecosystem Cache Redirection
    env_map.insert(
        "CARGO_HOME".into(),
        sandbox_cache_dir
            .join("cargo")
            .to_string_lossy()
            .to_string(),
    );
    env_map.insert(
        "npm_config_cache".into(),
        sandbox_cache_dir.join("npm").to_string_lossy().to_string(),
    );
    env_map.insert(
        "PIP_CACHE_DIR".into(),
        sandbox_cache_dir.join("pip").to_string_lossy().to_string(),
    );
    env_map.insert(
        "GOCACHE".into(),
        sandbox_cache_dir.join("go").to_string_lossy().to_string(),
    );
    env_map.insert(
        "GOPATH".into(),
        sandbox_cache_dir
            .join("gopath")
            .to_string_lossy()
            .to_string(),
    );

    if !resolved_config.allow_network {
        env_map.insert("CARGO_NET_OFFLINE".into(), "true".into());
        env_map.insert("PIP_NO_INDEX".into(), "1".into());
        env_map.insert("GOPROXY".into(), "off".into());
        env_map.insert("npm_config_offline".into(), "true".into());
        env_map.insert("YARN_OFFLINE".into(), "true".into());
        env_map.insert("PNPM_OFFLINE".into(), "true".into());
        env_map.insert("COMPOSER_DISABLE_NETWORK".into(), "1".into());
        env_map.insert("HTTP_PROXY".into(), "http://127.0.0.1:0".into());
        env_map.insert("HTTPS_PROXY".into(), "http://127.0.0.1:0".into());
        env_map.insert("ALL_PROXY".into(), "http://127.0.0.1:0".into());
        env_map.insert("http_proxy".into(), "http://127.0.0.1:0".into());
        env_map.insert("https_proxy".into(), "http://127.0.0.1:0".into());
        env_map.insert("all_proxy".into(), "http://127.0.0.1:0".into());
        env_map.insert("NO_PROXY".into(), "localhost,127.0.0.1".into());
        env_map.insert("no_proxy".into(), "localhost,127.0.0.1".into());
    }

    if let Some(ref sock_path) = resolved_config.ipc_socket_path {
        env_map.insert(
            "DMZ_SIDECAR_SOCK".into(),
            sock_path.to_string_lossy().to_string(),
        );
    }

    resolved_config.envs = env_map.into_iter().collect();

    #[cfg(target_os = "macos")]
    return macos::run_sandboxed(&resolved_config);

    #[cfg(target_os = "linux")]
    return linux::run_sandboxed(&resolved_config);

    #[cfg(target_os = "windows")]
    return windows::run_sandboxed(&resolved_config);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err(PlatformError::UnsupportedPlatform(
        "Current operating system is not supported by DMZ sandbox".into(),
    ))
}
