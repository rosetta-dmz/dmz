use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Read/Write path for the active developer project repository
    pub workspace_path: PathBuf,
    /// Read-Only path for the cryptographically locked dependency store
    pub closure_path: PathBuf,
    /// Command/binary to execute inside the sandbox
    pub command: String,
    /// Command-line arguments
    pub args: Vec<String>,
    /// Environment variables for the isolated environment
    pub envs: HashMap<String, String>,
    /// Whether outbound network access is permitted (false by default for AI agents/builds)
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            closure_path: PathBuf::from("/nix/store"),
            command: String::from("/bin/sh"),
            args: Vec::new(),
            envs: HashMap::new(),
            allow_network: false,
        }
    }
}

#[derive(Debug)]
pub enum PlatformError {
    NamespaceError(String),
    SeatbeltError(String),
    JobObjectError(String),
    ExecutionFailed(std::io::Error),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NamespaceError(msg) => write!(f, "Linux Namespace Error: {}", msg),
            Self::SeatbeltError(msg) => write!(f, "macOS Seatbelt Error: {}", msg),
            Self::JobObjectError(msg) => write!(f, "Windows JobObject Error: {}", msg),
            Self::ExecutionFailed(err) => write!(f, "Process Execution Failed: {}", err),
        }
    }
}

impl std::error::Error for PlatformError {}

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

/// Dispatch sandbox execution to the host OS native kernel isolation mechanism
pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    #[cfg(target_os = "linux")]
    {
        linux::run_sandboxed(config)
    }

    #[cfg(target_os = "macos")]
    {
        macos::run_sandboxed(config)
    }

    #[cfg(target_os = "windows")]
    {
        windows::run_sandboxed(config)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(PlatformError::ExecutionFailed(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unsupported host platform for DMZ sandbox",
        )))
    }
}
