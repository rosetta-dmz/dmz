use crate::ipc::{IpcRequest, IpcResponse};
use crate::{AgentConfig, AgentRunner, SidecarOrchestrator};
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;
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

#[derive(Clone)]
struct DispatchContext {
    started_at: Instant,
    is_running: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    agent_runner: AgentRunner,
    orchestrator: SidecarOrchestrator,
}

pub struct IpcServer {
    config: IpcServerConfig,
    started_at: Instant,
    is_running: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    agent_runner: AgentRunner,
    orchestrator: SidecarOrchestrator,
}

impl IpcServer {
    pub fn new(config: IpcServerConfig) -> Self {
        Self::with_managers(config, AgentRunner::new(), SidecarOrchestrator::new())
    }

    pub fn with_managers(
        config: IpcServerConfig,
        agent_runner: AgentRunner,
        orchestrator: SidecarOrchestrator,
    ) -> Self {
        Self {
            config,
            started_at: Instant::now(),
            is_running: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            agent_runner,
            orchestrator,
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
        self.shutdown_notify.notify_waiters();
    }

    fn dispatch_context(&self) -> DispatchContext {
        DispatchContext {
            started_at: self.started_at,
            is_running: self.is_running.clone(),
            shutdown_notify: self.shutdown_notify.clone(),
            agent_runner: self.agent_runner.clone(),
            orchestrator: self.orchestrator.clone(),
        }
    }

    #[cfg(unix)]
    async fn listen_unix(&self) -> Result<(), String> {
        use tokio::net::UnixListener;

        if self.config.socket_path.exists() {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }

        let listener = UnixListener::bind(&self.config.socket_path).map_err(|e| {
            format!(
                "Failed to bind Unix socket at {:?}: {}",
                self.config.socket_path, e
            )
        })?;

        info!(path = ?self.config.socket_path, "DMZ Unix IPC server listening");

        while self.is_running.load(Ordering::SeqCst) {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _addr)) => {
                            let ctx = self.dispatch_context();
                            tokio::spawn(async move {
                                Self::handle_connection(stream, ctx).await;
                            });
                        }
                        Err(e) => warn!("Unix IPC accept connection error: {}", e),
                    }
                }
                _ = self.shutdown_notify.notified() => {
                    break;
                }
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

        let pipe_name = self.config.socket_path.to_string_lossy().into_owned();
        info!(pipe = %pipe_name, "DMZ Windows Named Pipe IPC server listening");

        // Per official Tokio docs: claim the first pipe instance before entering the accept loop
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .map_err(|e| format!("Failed to create initial Windows Named Pipe: {}", e))?;

        while self.is_running.load(Ordering::SeqCst) {
            tokio::select! {
                connect_res = server.connect() => {
                    match connect_res {
                        Ok(()) => {
                            let connected_client = server;
                            // Instantiate replacement pipe BEFORE spawning handler so incoming clients never get NotFound
                            server = ServerOptions::new()
                                .create(&pipe_name)
                                .map_err(|e| format!("Failed to create subsequent Windows Named Pipe: {}", e))?;
                            let ctx = self.dispatch_context();
                            tokio::spawn(async move {
                                Self::handle_connection(connected_client, ctx).await;
                            });
                        }
                        Err(e) => warn!("Windows Named Pipe connect error: {}", e),
                    }
                }
                _ = self.shutdown_notify.notified() => {
                    break;
                }
            }
        }

        info!("Windows Named Pipe server stopped");
        Ok(())
    }

    async fn handle_connection<S>(stream: S, ctx: DispatchContext)
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
                        Ok(req) => Self::dispatch_request(req, &ctx).await,
                        Err(err) => {
                            IpcResponse::error(format!("Invalid IPC JSON payload: {}", err))
                        }
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

    async fn dispatch_request(req: IpcRequest, ctx: &DispatchContext) -> IpcResponse {
        match req {
            IpcRequest::Ping => IpcResponse::ok("pong"),

            IpcRequest::GetStatus => {
                let active_agents = ctx.agent_runner.list_active_agents().await;
                let sidecar_state = ctx.orchestrator.current_state().await;
                let status_data = serde_json::json!({
                    "status": "active",
                    "uptime_seconds": ctx.started_at.elapsed().as_secs(),
                    "active_agents": active_agents.len(),
                    "active_agent_ids": active_agents,
                    "sidecar_state": format!("{:?}", sidecar_state),
                });
                IpcResponse::ok_with_data("Supervisor active", status_data)
            }

            IpcRequest::StartAgent {
                agent_id,
                allowed_domains,
                scratchpad_bytes,
            } => {
                info!(agent = %agent_id, domains = ?allowed_domains, "Spawning agent process via IPC");
                let (binary_or_script, args) = if cfg!(windows) {
                    (
                        "ping.exe".to_string(),
                        vec![
                            "-n".to_string(),
                            "3600".to_string(),
                            "127.0.0.1".to_string(),
                        ],
                    )
                } else {
                    ("sleep".to_string(), vec!["3600".to_string()])
                };

                let config = AgentConfig {
                    agent_id: agent_id.clone(),
                    binary_or_script,
                    args,
                    envs: HashMap::new(),
                    allowed_domains,
                    scratchpad_bytes: scratchpad_bytes.max(1024 * 1024),
                };

                match ctx.agent_runner.spawn_agent(config).await {
                    Ok(()) => IpcResponse::ok(format!("Agent '{}' spawned successfully", agent_id)),
                    Err(e) => IpcResponse::error(e),
                }
            }

            IpcRequest::StopAgent { agent_id } => {
                info!(agent = %agent_id, "Stopping agent process via IPC");
                match ctx.agent_runner.terminate_agent(&agent_id).await {
                    Ok(()) => IpcResponse::ok(format!("Agent '{}' terminated", agent_id)),
                    Err(e) => IpcResponse::error(e),
                }
            }

            IpcRequest::RunSandbox {
                workspace_path,
                command,
                args,
                allow_network,
            } => {
                info!(cmd = %command, workspace = %workspace_path, "Executing sandboxed task via IPC");
                let ws_path = PathBuf::from(&workspace_path);
                let store_path = ws_path.join(".dmz").join("store");
                let config = SandboxConfig {
                    workspace_path: ws_path,
                    closure_path: store_path,
                    command,
                    args,
                    envs: std::env::vars().collect(),
                    allow_network,
                    ipc_socket_path: None,
                };

                let exec_res = tokio::task::spawn_blocking(move || run_sandboxed(&config)).await;

                match exec_res {
                    Ok(Ok(status)) => {
                        let code = status.code().unwrap_or(-1);
                        IpcResponse::ok_with_data(
                            format!("Sandbox command completed (exit code {})", code),
                            serde_json::json!({
                                "success": status.success(),
                                "exit_code": code,
                            }),
                        )
                    }
                    Ok(Err(e)) => IpcResponse::error(format!("Sandbox execution failed: {}", e)),
                    Err(e) => IpcResponse::error(format!("Sandbox task join error: {}", e)),
                }
            }

            IpcRequest::GetSidecarStatus => {
                let state = ctx.orchestrator.current_state().await;
                let socket = ctx
                    .orchestrator
                    .current_socket_path()
                    .await
                    .map(|p| p.to_string_lossy().to_string());
                let last_error = ctx.orchestrator.last_error().await;
                let status_data = serde_json::json!({
                    "state": format!("{:?}", state),
                    "socket": socket,
                    "last_error": last_error,
                });
                IpcResponse::ok_with_data("Sidecar status query", status_data)
            }

            IpcRequest::StopSidecarPair => {
                info!("StopSidecarPair requested via IPC");
                match ctx.orchestrator.stop_pair().await {
                    Ok(()) => IpcResponse::ok("Sidecar pair stopped"),
                    Err(e) => IpcResponse::error(e),
                }
            }

            IpcRequest::Shutdown => {
                info!("Shutdown request received via IPC");
                ctx.is_running.store(false, Ordering::SeqCst);
                ctx.shutdown_notify.notify_waiters();
                IpcResponse::ok("Supervisor shutting down")
            }
        }
    }
}
