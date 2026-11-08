pub mod server;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcRequest {
    Ping,
    GetStatus,
    StartAgent {
        agent_id: String,
        allowed_domains: Vec<String>,
        scratchpad_bytes: usize,
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

pub struct IpcClient {
    socket_path: std::path::PathBuf,
}

impl IpcClient {
    pub fn new(socket_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub async fn send_request(&self, req: &IpcRequest) -> Result<IpcResponse, String> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut payload = serde_json::to_vec(req)
            .map_err(|e| format!("Failed to serialize IPC request: {}", e))?;
        payload.push(b'\n');

        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| {
                format!(
                    "Failed to connect to Unix IPC socket {:?}: {}",
                    self.socket_path, e
                )
            })?;

        #[cfg(windows)]
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&self.socket_path)
            .map_err(|e| {
                format!(
                    "Failed to connect to Windows Named Pipe {:?}: {}",
                    self.socket_path, e
                )
            })?;

        let (reader, mut writer) = tokio::io::split(stream);
        writer
            .write_all(&payload)
            .await
            .map_err(|e| format!("Failed to write IPC request: {}", e))?;

        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        buf_reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Failed to read IPC response: {}", e))?;

        serde_json::from_str(line.trim())
            .map_err(|e| format!("Failed to parse IPC response: {}", e))
    }
}
