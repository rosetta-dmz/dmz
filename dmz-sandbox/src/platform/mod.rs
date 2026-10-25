use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
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

    // Augment PATH to ensure user toolchain binaries are discoverable in non-login environments
    let home = std::env::var("HOME").unwrap_or_default();
    let mut current_path = env_map.get("PATH").cloned().unwrap_or_else(|| {
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".to_string())
    });

<<<<<<< Updated upstream
    // Prepend bundled DMZ toolchains (<workspace>/.dmz/toolchains/*/bin) if present
=======
    // Inject bundled toolchain paths if available (.dmz/toolchains/*/bin)
>>>>>>> Stashed changes
    let bundled_toolchains_dir = resolved_config.workspace_path.join(".dmz/toolchains");
    if bundled_toolchains_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&bundled_toolchains_dir) {
            for entry in entries.flatten() {
<<<<<<< Updated upstream
                let bin_dir = entry.path().join("bin");
                if bin_dir.exists() {
                    current_path = format!("{}:{}", bin_dir.display(), current_path);
=======
                let bin = entry.path().join("bin");
                if bin.exists() {
                    current_path = format!("{}:{}", bin.display(), current_path);
>>>>>>> Stashed changes
                }
            }
        }
        if bundled_toolchains_dir.join("rust").exists() {
            env_map.insert(
                "RUSTUP_HOME".into(),
                bundled_toolchains_dir
                    .join("rust")
                    .to_string_lossy()
                    .to_string(),
            );
            if !env_map.contains_key("CARGO_HOME") {
                env_map.insert(
                    "CARGO_HOME".into(),
                    resolved_config
                        .workspace_path
                        .join(".cargo")
                        .to_string_lossy()
                        .to_string(),
                );
            }
<<<<<<< Updated upstream
=======
            let rust_lib = bundled_toolchains_dir
                .join("rust/lib")
                .to_string_lossy()
                .to_string();
            let existing_dyld = env_map
                .get("DYLD_LIBRARY_PATH")
                .cloned()
                .unwrap_or_default();
            env_map.insert(
                "DYLD_LIBRARY_PATH".into(),
                if existing_dyld.is_empty() {
                    rust_lib.clone()
                } else {
                    format!("{}:{}", rust_lib, existing_dyld)
                },
            );
            let existing_ld = env_map.get("LD_LIBRARY_PATH").cloned().unwrap_or_default();
            env_map.insert(
                "LD_LIBRARY_PATH".into(),
                if existing_ld.is_empty() {
                    rust_lib
                } else {
                    format!("{}:{}", rust_lib, existing_ld)
                },
            );
>>>>>>> Stashed changes
        }
        if bundled_toolchains_dir.join("go").exists() {
            env_map.insert(
                "GOROOT".into(),
                bundled_toolchains_dir
                    .join("go")
                    .to_string_lossy()
                    .to_string(),
            );
        }
        if bundled_toolchains_dir.join("python").exists() {
            env_map.insert(
                "PYTHONHOME".into(),
                bundled_toolchains_dir
                    .join("python")
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

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
            current_path = format!("{}:{}", bin, current_path);
        }
    }

    // Workspace node_modules/.bin in PATH if it exists
    let node_bin = resolved_config
        .workspace_path
        .join("node_modules")
        .join(".bin");
    if node_bin.exists() {
        current_path = format!("{}:{}", node_bin.display(), current_path);
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

    // Set PYTHONPATH and VIRTUAL_ENV if python environments exist
    let venv = resolved_config.workspace_path.join(".venv");
    let dmz_venv = resolved_config.workspace_path.join(".dmz").join("venv");
    if venv.exists() {
        env_map.insert("VIRTUAL_ENV".into(), venv.to_string_lossy().to_string());
    } else if dmz_venv.exists() {
        env_map.insert("VIRTUAL_ENV".into(), dmz_venv.to_string_lossy().to_string());
    }

    let dmz_site_packages = dmz_venv
        .join("lib")
        .join("python3.11")
        .join("site-packages");
    if dmz_site_packages.exists() {
        env_map.insert(
            "PYTHONPATH".into(),
            dmz_site_packages.to_string_lossy().to_string(),
        );
    }

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
    Err(PlatformError::UnsupportedPlatform(
        "Current operating system is not supported by DMZ sandbox".into(),
    ))
}
