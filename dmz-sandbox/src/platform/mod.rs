use std::path::PathBuf;
use std::process::ExitStatus;
use std::fmt;
use std::error::Error;
use std::collections::HashMap;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
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
    // inside the workspace (.dmz/cache), which is automatically writable under workspace permissions.
    let sandbox_cache_dir = resolved_config.workspace_path.join(".dmz").join("cache");
    let _ = std::fs::create_dir_all(&sandbox_cache_dir);
    let cache_str = sandbox_cache_dir.to_string_lossy();

    let mut env_map: HashMap<String, String> = resolved_config.envs.into_iter().collect();
    env_map.insert("CARGO_HOME".into(), format!("{}/cargo", cache_str));
    env_map.insert("npm_config_cache".into(), format!("{}/npm", cache_str));
    env_map.insert("PIP_CACHE_DIR".into(), format!("{}/pip", cache_str));
    env_map.insert("GOCACHE".into(), format!("{}/go", cache_str));
    env_map.insert("GOPATH".into(), format!("{}/gopath", cache_str));
    resolved_config.envs = env_map.into_iter().collect();

    #[cfg(target_os = "macos")]
    return macos::run_sandboxed(&resolved_config);

    #[cfg(target_os = "linux")]
    return linux::run_sandboxed(&resolved_config);

    #[cfg(target_os = "windows")]
    return windows::run_sandboxed(&resolved_config);

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    Err(PlatformError::UnsupportedPlatform("Current operating system is not supported by DMZ sandbox".into()))
}
