use crate::ipc::{IpcRequest, IpcResponse};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{error, info, warn};

pub struct IpcServerConfig {
    pub socket_path: PathBuf,
}

impl Default for IpcServerConfig {
    fn default() -> Self {
        #[cfg(unix)]
        let socket_path = std::env::temp_dir().join("dmz_supervisor.sock");

        #[cfg(windows)]
        let socket_path = PathBuf::from(r"\\.\pipe\dmz_supervisor");

        Self { socket_path }
    }
}

pub struct IpcServer {
    config: IpcServerConfig,
    is_running: Arc<AtomicBool>,
}

impl IpcServer {
    pub fn new(config: IpcServerConfig) -> Self {
        Self {
            config,
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn start(&self) -> Result<(), String> {
        self.is_running.store(true, Ordering::SeqCst);

        #[cfg(unix)]
        {
            self.listen_unix().await
        }

        #[cfg(windows)]
        {
            self.listen_windows().await
        }
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }

    #[cfg(unix)]
    async fn listen_unix(&self) -> Result<(), String> {
        use tokio::net::UnixListener;

        if self.config.socket_path.exists() {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }

        let listener = UnixListener::bind(&self.config.socket_path)
            .map_err(|e| format!("Failed to bind Unix socket at {:?}: {}", self.config.socket_path, e))?;

        info!(path = ?self.config.socket_path, "DMZ Unix IPC server listening");

        while self.is_running.load(Ordering::SeqCst) {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _addr)) => {
                            let running = self.is_running.clone();
                            tokio::spawn(async move {
                                Self::handle_connection(stream, running).await;
                            });
                        }
                        Err(e) => warn!("Unix IPC accept connection error: {}", e),
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            }
        }

        if self.config.socket_path.exists() {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }

        info!("Unix IPC server stopped");
        Ok(())
    }

    #[cfg(windows)]
    async fn listen_windows(&self) -> Result<(), String> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let pipe_name = self.config.socket_path.to_string_lossy();
        info!(pipe = %pipe_name, "DMZ Windows Named Pipe IPC server listening");

        while self.is_running.load(Ordering::SeqCst) {
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)
                .map_err(|e| format!("Failed to create Windows Named Pipe: {}", e))?;

            match server.connect().await {
                Ok(_) => {
                    let running = self.is_running.clone();
                    tokio::spawn(async move {
                        Self::handle_connection(server, running).await;
                    });
                }
                Err(e) => warn!("Windows Named Pipe connect error: {}", e),
            }
        }

        info!("Windows Named Pipe server stopped");
        Ok(())
    }

    async fn handle_connection<S>(stream: S, is_running: Arc<AtomicBool>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match buf_reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let request_res: Result<IpcRequest, _> = serde_json::from_str(line.trim());
                    let response = match request_res {
                        Ok(req) => Self::dispatch_request(req, &is_running).await,
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

    async fn dispatch_request(req: IpcRequest, is_running: &AtomicBool) -> IpcResponse {
        match req {
            IpcRequest::Ping => IpcResponse::ok("pong"),

            IpcRequest::GetStatus => {
                let status_data = serde_json::json!({
                    "status": "active",
                    "uptime_seconds": 0,
                    "active_agents": 0
                });
                IpcResponse::ok_with_data("Supervisor active", status_data)
            }

            IpcRequest::StartAgent {
                agent_id,
                allowed_domains,
                scratchpad_bytes: _,
            } => {
                info!(agent = %agent_id, domains = ?allowed_domains, "Spawning agent process");
                IpcResponse::ok(format!("Agent '{}' spawned successfully", agent_id))
            }

            IpcRequest::StopAgent { agent_id } => {
                info!(agent = %agent_id, "Stopping agent process");
                IpcResponse::ok(format!("Agent '{}' terminated", agent_id))
            }

            IpcRequest::RunSandbox {
                workspace_path,
                command,
                args: _,
                allow_network: _,
            } => {
                info!(cmd = %command, workspace = %workspace_path, "Executing sandboxed task");
                IpcResponse::ok("Sandbox command dispatched")
            }

            IpcRequest::Shutdown => {
                info!("Shutdown request received via IPC");
                is_running.store(false, Ordering::SeqCst);
                IpcResponse::ok("Supervisor shutting down")
            }
        }
    }
}