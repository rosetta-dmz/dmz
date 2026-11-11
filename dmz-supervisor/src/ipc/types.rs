use crate::{AgentRunner, ServiceRunner, SidecarOrchestrator};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tokio::sync::Notify;

/// POSIX.1-2017 limit for sockaddr_un.sun_path (104 bytes on macOS/BSD, 108 on Linux).
pub const MAX_UNIX_SOCKET_PATH: usize = 104;

/// Maximum allowable IPC payload frame size to defend against memory exhaustion (CWE-400).
pub const MAX_IPC_FRAME_BYTES: usize = 64 * 1024; // 64 KB

#[derive(Debug, Error)]
pub enum IpcServerError {
    #[error("Unix domain socket path length ({len} bytes) exceeds POSIX maximum limit ({max} bytes): {path:?}")]
    SocketPathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },

    #[error("Failed to prepare IPC socket directory {path:?}: {source}")]
    SocketDirectoryPreparation {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to set 0o700 permissions on IPC socket directory {path:?}: {source}")]
    SocketPermission {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to bind IPC socket at {path:?}: {source}")]
    SocketBindFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Peer credential authentication rejected: {0}")]
    PeerAuthenticationFailed(String),

    #[error("IPC payload exceeds maximum allowed frame size of {max_bytes} bytes (received {bytes} bytes)")]
    PayloadTooLarge { bytes: usize, max_bytes: usize },

    #[error("I/O error during IPC communication: {0}")]
    Io(#[from] std::io::Error),

    #[error("IPC JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IPC server error: {0}")]
    Generic(String),
}

impl IpcServerError {
    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl From<IpcServerError> for String {
    fn from(err: IpcServerError) -> Self {
        err.to_string()
    }
}

#[derive(Debug, Error)]
pub enum IpcClientError {
    #[error("Failed to serialize IPC request: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Failed to connect to IPC socket {path:?}: {source}")]
    ConnectFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("I/O error during IPC communication: {0}")]
    Io(#[from] std::io::Error),

    #[error(
        "IPC response exceeded maximum frame limit of {max_bytes} bytes (received {bytes} bytes)"
    )]
    PayloadTooLarge { bytes: usize, max_bytes: usize },

    #[error("IPC client error: {0}")]
    Generic(String),
}

impl IpcClientError {
    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl From<IpcClientError> for String {
    fn from(err: IpcClientError) -> Self {
        err.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcRequest {
    Ping,
    GetStatus,
    StartAgent {
        agent_id: String,
        allowed_domains: Vec<String>,
        scratchpad_bytes: usize,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Option<Vec<String>>,
    },
    StopAgent {
        agent_id: String,
    },
    RunSandbox {
        workspace_path: String,
        command: String,
        args: Vec<String>,
        allow_network: bool,
    },
    GetSidecarStatus,
    StopSidecarPair,
    GetServiceStatus,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum IpcResponse {
    Ok {
        message: String,
        payload: Option<serde_json::Value>,
    },
    Error {
        message: String,
    },
}

impl IpcResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self::Ok {
            message: message.into(),
            payload: None,
        }
    }

    pub fn ok_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self::Ok {
            message: message.into(),
            payload: Some(data),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct IpcServerConfig {
    pub socket_path: PathBuf,
}

impl Default for IpcServerConfig {
    fn default() -> Self {
        #[cfg(unix)]
        let socket_path = {
            // SAFETY: getuid is an async-signal-safe POSIX syscall with no preconditions.
            let uid = unsafe { libc::getuid() };

            let dir = if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
                let candidate = PathBuf::from(runtime_dir).join("dmz");
                if candidate.join("supervisor.sock").as_os_str().len() < MAX_UNIX_SOCKET_PATH {
                    candidate
                } else {
                    std::env::temp_dir().join(format!("dmz_supervisor_{}", uid))
                }
            } else {
                std::env::temp_dir().join(format!("dmz_supervisor_{}", uid))
            };

            let socket_candidate = dir.join("supervisor.sock");
            let (chosen_dir, chosen_sock) =
                if socket_candidate.as_os_str().len() < MAX_UNIX_SOCKET_PATH {
                    (dir, socket_candidate)
                } else {
                    let fallback_dir = std::env::temp_dir().join("dmz");
                    let fallback_sock = fallback_dir.join(format!("s_{}.sock", uid));
                    (fallback_dir, fallback_sock)
                };

            if let Err(e) = std::fs::create_dir_all(&chosen_dir) {
                eprintln!("Warning: Failed to create IPC socket directory: {}", e);
            }
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) =
                std::fs::set_permissions(&chosen_dir, std::fs::Permissions::from_mode(0o700))
            {
                eprintln!(
                    "Warning: Failed to set 0o700 permissions on IPC socket directory: {}",
                    e
                );
            }
            chosen_sock
        };

        #[cfg(windows)]
        let socket_path = {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
            PathBuf::from(format!(r"\\.\pipe\dmz_supervisor_{}", user))
        };

        Self { socket_path }
    }
}

#[derive(Clone)]
pub struct DispatchContext {
    pub started_at: Instant,
    pub is_running: Arc<AtomicBool>,
    pub shutdown_notify: Arc<Notify>,
    pub agent_runner: AgentRunner,
    pub orchestrator: SidecarOrchestrator,
    pub service_runner: Option<ServiceRunner>,
}
