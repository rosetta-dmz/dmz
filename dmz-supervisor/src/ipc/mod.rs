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
