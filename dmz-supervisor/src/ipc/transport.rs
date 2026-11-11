use super::dispatch::dispatch_request;
use super::types::{DispatchContext, IpcRequest, IpcResponse, MAX_IPC_FRAME_BYTES};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tracing::error;

/// Handles an active IPC stream connection with bounded frame reads and JSON serialization.
pub async fn handle_connection<S>(stream: S, ctx: DispatchContext)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        // Bound line reading to MAX_IPC_FRAME_BYTES + 1 to prevent unbounded memory consumption (CWE-400)
        let mut take_reader = (&mut buf_reader).take((MAX_IPC_FRAME_BYTES + 1) as u64);
        match take_reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(bytes_read) => {
                if bytes_read > MAX_IPC_FRAME_BYTES
                    || (!line.ends_with('\n') && bytes_read == MAX_IPC_FRAME_BYTES + 1)
                {
                    error!(
                        bytes_read,
                        max = MAX_IPC_FRAME_BYTES,
                        "IPC payload exceeded maximum allowed frame size; terminating connection"
                    );
                    let response = IpcResponse::error(format!(
                        "IPC payload exceeds maximum allowed size of {} bytes",
                        MAX_IPC_FRAME_BYTES
                    ));
                    if let Ok(mut resp_bytes) = serde_json::to_vec(&response) {
                        resp_bytes.push(b'\n');
                        let _ = writer.write_all(&resp_bytes).await;
                    }
                    break;
                }

                let request_res: Result<IpcRequest, _> = serde_json::from_str(line.trim());
                let response = match request_res {
                    Ok(req) => dispatch_request(req, &ctx).await,
                    Err(err) => IpcResponse::error(format!("Invalid IPC JSON payload: {}", err)),
                };

                if let Ok(mut resp_bytes) = serde_json::to_vec(&response) {
                    resp_bytes.push(b'\n');
                    if let Err(e) = writer.write_all(&resp_bytes).await {
                        error!("Failed to write IPC response: {}", e);
                        break;
                    }
                }
            }
            Err(e) => {
                error!("Error reading from IPC stream: {}", e);
                break;
            }
        }
    }
}
