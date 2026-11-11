use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
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

/// Unified interface for OS-native containerless sandbox runners
pub trait SandboxRunner {
    fn run(&self, config: &SandboxConfig) -> Result<ExitStatus, PlatformError>;
}

/// Default native sandbox runner dispatching to OS-specific kernels
pub struct NativeSandboxRunner;

impl SandboxRunner for NativeSandboxRunner {
    fn run(&self, config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
        run_sandboxed(config)
    }
}

/// Primary public entry point for executing a command in the DMZ containerless sandbox
pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    if !config.workspace_path.exists() {
        return Err(PlatformError::PlatformFault(format!(
            "Workspace path does not exist: {:?}",
            config.workspace_path
        )));
    }

    let absolute_command = resolve_command(&config.command);
    let (cache_dir, tmp_dir) = ensure_sandbox_directories(&config.workspace_path)?;

    let mut resolved_config = config.clone();
    resolved_config.command = absolute_command;
    resolved_config.envs = build_sandbox_environment(&resolved_config, &cache_dir, &tmp_dir);

    dispatch_platform_sandbox(&resolved_config)
}

fn resolve_command(command: &str) -> String {
    if PathBuf::from(command).is_absolute() {
        command.to_string()
    } else {
        which::which(command)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| command.to_string())
    }
}

fn ensure_sandbox_directories(workspace: &Path) -> Result<(PathBuf, PathBuf), PlatformError> {
    let cache_dir = workspace.join(".dmz").join("cache");
    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        PlatformError::PlatformFault(format!(
            "Failed to create sandbox cache directory {:?}: {}",
            cache_dir, e
        ))
    })?;

    let tmp_dir = workspace.join(".dmz").join("tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| {
        PlatformError::PlatformFault(format!(
            "Failed to create sandbox temporary directory {:?}: {}",
            tmp_dir, e
        ))
    })?;

    Ok((cache_dir, tmp_dir))
}

fn build_sandbox_environment(
    config: &SandboxConfig,
    cache_dir: &Path,
    tmp_dir: &Path,
) -> Vec<(String, String)> {
    let mut env_map: HashMap<String, String> = config.envs.iter().cloned().collect();

    let tmp_str = tmp_dir.to_string_lossy().to_string();
    env_map.insert("TMPDIR".into(), tmp_str.clone());
    env_map.insert("TEMP".into(), tmp_str.clone());
    env_map.insert("TMP".into(), tmp_str);

    dmz_core::toolchain::ToolchainEnv::inject_toolchain_paths(&config.workspace_path, &mut env_map);
    inject_path_fallbacks(&mut env_map, &config.workspace_path, &config.closure_path);
    inject_python_env(&mut env_map, &config.workspace_path);
    inject_ecosystem_caches(&mut env_map, cache_dir);

    if !config.allow_network {
        inject_offline_hints(&mut env_map);
    }

    if let Some(ref sock_path) = config.ipc_socket_path {
        env_map.insert(
            "DMZ_SIDECAR_SOCK".into(),
            sock_path.to_string_lossy().to_string(),
        );
    }

    env_map.into_iter().collect()
}

fn inject_path_fallbacks(env_map: &mut HashMap<String, String>, workspace: &Path, closure: &Path) {
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

    let has_bundled_toolchains =
        workspace.join(".dmz").join("toolchains").is_dir() || closure.join("toolchains").is_dir();

    if !has_bundled_toolchains && !home.is_empty() {
        let mut toolchain_bins = vec![
            format!("{}/.cargo/bin", home),
            format!("{}/.local/bin", home),
            format!("{}/go/bin", home),
            format!("{}/.pnpm", home),
            format!("{}/.local/share/pnpm", home),
        ];

        #[cfg(target_os = "macos")]
        {
            toolchain_bins.push("/opt/homebrew/bin".to_string());
            toolchain_bins.push("/usr/local/bin".to_string());
        }

        #[cfg(target_os = "linux")]
        {
            toolchain_bins.push("/usr/local/bin".to_string());
        }

        for bin in &toolchain_bins {
            if Path::new(bin).exists() && !current_path.contains(bin) {
                current_path = format!("{}{}{}", bin, separator, current_path);
            }
        }
    }

    let node_bin = workspace.join("node_modules").join(".bin");
    if node_bin.exists() {
        current_path = format!("{}{}{}", node_bin.display(), separator, current_path);
    }
    env_map.insert("PATH".into(), current_path);

    let node_modules = workspace.join("node_modules");
    if node_modules.exists() {
        env_map.insert(
            "NODE_PATH".into(),
            node_modules.to_string_lossy().to_string(),
        );
    }
}

fn inject_python_env(env_map: &mut HashMap<String, String>, workspace: &Path) {
    let venv = workspace.join(".venv");
    let dmz_venv = workspace.join(".dmz").join("venv");
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
}

fn inject_ecosystem_caches(env_map: &mut HashMap<String, String>, cache_dir: &Path) {
    env_map.insert(
        "CARGO_HOME".into(),
        cache_dir.join("cargo").to_string_lossy().to_string(),
    );
    env_map.insert(
        "npm_config_cache".into(),
        cache_dir.join("npm").to_string_lossy().to_string(),
    );
    env_map.insert(
        "PIP_CACHE_DIR".into(),
        cache_dir.join("pip").to_string_lossy().to_string(),
    );
    env_map.insert(
        "GOCACHE".into(),
        cache_dir.join("go").to_string_lossy().to_string(),
    );
    env_map.insert(
        "GOPATH".into(),
        cache_dir.join("gopath").to_string_lossy().to_string(),
    );
}

fn inject_offline_hints(env_map: &mut HashMap<String, String>) {
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

fn dispatch_platform_sandbox(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    #[cfg(target_os = "macos")]
    return macos::run_sandboxed(config);

    #[cfg(target_os = "linux")]
    return linux::run_sandboxed(config);

    #[cfg(target_os = "windows")]
    return windows::run_sandboxed(config);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err(PlatformError::UnsupportedPlatform(
        "Current operating system is not supported by DMZ sandbox".into(),
    ))
}
