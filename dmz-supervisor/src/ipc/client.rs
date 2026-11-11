use super::types::{IpcRequest, IpcResponse, MAX_IPC_FRAME_BYTES};
use std::path::PathBuf;

pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub async fn send_request(&self, req: &IpcRequest) -> Result<IpcResponse, String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

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
        let mut take_reader = (&mut buf_reader).take((MAX_IPC_FRAME_BYTES + 1) as u64);
        take_reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Failed to read IPC response: {}", e))?;

        if line.len() > MAX_IPC_FRAME_BYTES {
            return Err(format!(
                "IPC response exceeded maximum frame limit of {} bytes",
                MAX_IPC_FRAME_BYTES
            ));
        }

        serde_json::from_str(line.trim())
            .map_err(|e| format!("Failed to parse IPC response: {}", e))
    }
}
