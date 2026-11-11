#[cfg(unix)]
use super::auth::verify_peer_credentials;
#[cfg(unix)]
use super::socket::{prepare_socket_directory, validate_socket_path_length, SocketGuard};
use super::transport::handle_connection;
use super::types::DispatchContext;
pub use super::types::{
    IpcServerConfig, IpcServerError, MAX_IPC_FRAME_BYTES, MAX_UNIX_SOCKET_PATH,
};
use crate::{AgentRunner, ServiceRunner, SidecarOrchestrator};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;
use tracing::{info, warn};

pub struct IpcServer {
    config: IpcServerConfig,
    started_at: Instant,
    is_running: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    agent_runner: AgentRunner,
    orchestrator: SidecarOrchestrator,
    service_runner: Option<ServiceRunner>,
}

impl IpcServer {
    pub fn new(config: IpcServerConfig) -> Self {
        Self::with_all_managers(
            config,
            AgentRunner::new(),
            SidecarOrchestrator::new(),
            Some(ServiceRunner::new()),
        )
    }

    pub fn with_managers(
        config: IpcServerConfig,
        agent_runner: AgentRunner,
        orchestrator: SidecarOrchestrator,
    ) -> Self {
        Self::with_all_managers(config, agent_runner, orchestrator, None)
    }

    pub fn with_all_managers(
        config: IpcServerConfig,
        agent_runner: AgentRunner,
        orchestrator: SidecarOrchestrator,
        service_runner: Option<ServiceRunner>,
    ) -> Self {
        Self {
            config,
            started_at: Instant::now(),
            is_running: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            agent_runner,
            orchestrator,
            service_runner,
        }
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    pub fn config(&self) -> &IpcServerConfig {
        &self.config
    }

    pub async fn start(&self) -> Result<(), IpcServerError> {
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
            service_runner: self.service_runner.clone(),
        }
    }

    #[cfg(unix)]
    pub(crate) async fn listen_unix(&self) -> Result<(), IpcServerError> {
        use tokio::net::UnixListener;

        validate_socket_path_length(&self.config.socket_path)?;
        prepare_socket_directory(&self.config.socket_path).await?;

        if tokio::fs::try_exists(&self.config.socket_path)
            .await
            .unwrap_or(false)
        {
            let _ = tokio::fs::remove_file(&self.config.socket_path).await;
        }

        let listener = UnixListener::bind(&self.config.socket_path).map_err(|e| {
            IpcServerError::SocketBindFailed {
                path: self.config.socket_path.clone(),
                source: e,
            }
        })?;

        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = tokio::fs::set_permissions(
                &self.config.socket_path,
                std::fs::Permissions::from_mode(0o700),
            )
            .await
            {
                warn!(
                    path = ?self.config.socket_path,
                    error = %e,
                    "Failed to enforce 0o700 permissions on Unix IPC socket"
                );
            }
        }

        // RAII socket guard guarantees socket unlinking even if task is aborted or dropped
        let _socket_guard = SocketGuard::new(self.config.socket_path.clone());

        info!(path = ?self.config.socket_path, "DMZ Unix IPC server listening");

        let mut connection_set = tokio::task::JoinSet::new();

        while self.is_running.load(Ordering::SeqCst) {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _addr)) => {
                            if let Err(auth_err) = verify_peer_credentials(&stream) {
                                warn!("Rejected unauthorized IPC connection: {}", auth_err);
                                continue;
                            }
                            let ctx = self.dispatch_context();
                            connection_set.spawn(async move {
                                handle_connection(stream, ctx).await;
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

        // Gracefully drain in-flight connection handlers with bounded timeout
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            while connection_set.join_next().await.is_some() {}
        })
        .await;

        info!("Unix IPC server stopped");
        Ok(())
    }

    #[cfg(windows)]
    pub(crate) async fn listen_windows(&self) -> Result<(), IpcServerError> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let pipe_name = self.config.socket_path.to_string_lossy().into_owned();
        info!(pipe = %pipe_name, "DMZ Windows Named Pipe IPC server listening");

        // Per official Tokio docs: claim first pipe instance before entering accept loop
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .map_err(|e| IpcServerError::SocketBindFailed {
                path: self.config.socket_path.clone(),
                source: e,
            })?;

        let mut connection_set = tokio::task::JoinSet::new();

        while self.is_running.load(Ordering::SeqCst) {
            tokio::select! {
                connect_res = server.connect() => {
                    match connect_res {
                        Ok(()) => {
                            let connected_client = server;
                            // Instantiate replacement pipe BEFORE spawning handler so incoming clients never get NotFound
                            server = ServerOptions::new()
                                .create(&pipe_name)
                                .map_err(|e| IpcServerError::SocketBindFailed {
                                    path: self.config.socket_path.clone(),
                                    source: e,
                                })?;
                            let ctx = self.dispatch_context();
                            connection_set.spawn(async move {
                                handle_connection(connected_client, ctx).await;
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

        // Drain active connections with a bounded timeout
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            while connection_set.join_next().await.is_some() {}
        })
        .await;

        info!("Windows Named Pipe server stopped");
        Ok(())
    }
}
