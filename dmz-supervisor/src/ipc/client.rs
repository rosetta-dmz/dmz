use super::types::{IpcClientError, IpcRequest, IpcResponse, MAX_IPC_FRAME_BYTES};
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_IPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct IpcClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl IpcClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            timeout: DEFAULT_IPC_REQUEST_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn send_request(&self, req: &IpcRequest) -> Result<IpcResponse, IpcClientError> {
        self.send_request_with_timeout(req, self.timeout).await
    }

    pub async fn send_request_with_timeout(
        &self,
        req: &IpcRequest,
        timeout: Duration,
    ) -> Result<IpcResponse, IpcClientError> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let mut payload = serde_json::to_vec(req)?;
        payload.push(b'\n');

        let io_future = async {
            #[cfg(unix)]
            let stream = tokio::net::UnixStream::connect(&self.socket_path)
                .await
                .map_err(|e| IpcClientError::ConnectFailed {
                    path: self.socket_path.clone(),
                    source: e,
                })?;

            #[cfg(windows)]
            let stream = tokio::net::windows::named_pipe::ClientOptions::new()
                .open(&self.socket_path)
                .map_err(|e| IpcClientError::ConnectFailed {
                    path: self.socket_path.clone(),
                    source: e,
                })?;

            let (reader, mut writer) = tokio::io::split(stream);
            writer.write_all(&payload).await?;

            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            let mut take_reader = (&mut buf_reader).take((MAX_IPC_FRAME_BYTES + 1) as u64);
            take_reader.read_line(&mut line).await?;

            if line.len() > MAX_IPC_FRAME_BYTES {
                return Err(IpcClientError::PayloadTooLarge {
                    bytes: line.len(),
                    max_bytes: MAX_IPC_FRAME_BYTES,
                });
            }

            let resp: IpcResponse = serde_json::from_str(line.trim())?;
            Ok(resp)
        };

        tokio::time::timeout(timeout, io_future)
            .await
            .map_err(|_| IpcClientError::Timeout(timeout))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ipc_client_connect_failed_returns_typed_error() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let missing_sock = dir.path().join("nonexistent.sock");
        let client = IpcClient::new(&missing_sock);
        let res = client.send_request(&IpcRequest::Ping).await;
        assert!(matches!(res, Err(IpcClientError::ConnectFailed { .. })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_ipc_client_times_out_on_stalled_server() {
        let dir = tempfile::tempdir().expect("create short tempdir");
        let sock_path = dir.path().join("stall.sock");
        let listener = tokio::net::UnixListener::bind(&sock_path).expect("bind unix listener");

        let server_task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept stalled connection");
            tokio::time::sleep(Duration::from_millis(250)).await;
        });

        let client = IpcClient::new(&sock_path).with_timeout(Duration::from_millis(30));
        let res = client.send_request(&IpcRequest::Ping).await;
        assert!(matches!(res, Err(IpcClientError::Timeout(_))));

        let _ = server_task.await;
    }
}
