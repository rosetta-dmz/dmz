use dmz_core::workspace::{
    cleanup_stale_socket, safe_socket_path, MemberRole, WorkspaceMember, WorkspaceProjectConfig,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidecarLifecycleState {
    #[default]
    Idle,
    CleaningStaleSocket,
    SpawningSidecar,
    ProbingSocketReadiness,
    SidecarReady,
    SpawningPrimary,
    Running,
    DrainingPrimary,
    TerminatingSidecar,
    Stopped,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SidecarPairConfig {
    pub workspace_root: PathBuf,
    pub project: WorkspaceProjectConfig,
    pub allow_network: bool,
}

struct ActivePair {
    config: SidecarPairConfig,
    socket_path: PathBuf,
    sidecar_child: Child,
    primary_child: Option<Child>,
}

#[derive(Clone, Default)]
pub struct SidecarOrchestrator {
    state: Arc<RwLock<SidecarLifecycleState>>,
    active_pair: Arc<RwLock<Option<ActivePair>>>,
    active_socket: Arc<RwLock<Option<PathBuf>>>,
}

impl SidecarOrchestrator {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(SidecarLifecycleState::Idle)),
            active_pair: Arc::new(RwLock::new(None)),
            active_socket: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn current_state(&self) -> SidecarLifecycleState {
        *self.state.read().await
    }

    pub async fn current_socket_path(&self) -> Option<PathBuf> {
        self.active_socket.read().await.clone()
    }

    /// Prepare POSIX safe socket path and remove stale dead sockets from previous runs
    pub fn prepare_safe_socket(workspace: &Path, socket_id: &str) -> Result<PathBuf, String> {
        let sock_path = safe_socket_path(workspace, socket_id);

        let is_pipe = sock_path.to_string_lossy().starts_with(r"\\.\pipe\");
        if !is_pipe {
            if let Some(parent) = sock_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create socket directory {:?}: {}", parent, e)
                })?;
            }
        }

        let _ = cleanup_stale_socket(&sock_path);
        Ok(sock_path)
    }

    /// Actively probe socket connectivity with exponential backoff (KEP-753 Gate).
    /// Prevent primary app from starting before the sidecar socket is ready.
    pub async fn poll_socket_readiness(socket_path: &Path, timeout_ms: u64) -> Result<(), String> {
        Self::poll_socket_readiness_with_child(socket_path, timeout_ms, None).await
    }

    /// Fail-fast probe: polls socket readiness while monitoring companion child process status.
    pub async fn poll_socket_readiness_with_child(
        socket_path: &Path,
        timeout_ms: u64,
        mut child: Option<&mut Child>,
    ) -> Result<(), String> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let mut backoff = Duration::from_millis(25);

        while start.elapsed() < timeout {
            // Fail-fast check: if the companion process crashed, abort immediately
            if let Some(ref mut c) = child {
                if let Ok(Some(exit_status)) = c.try_wait() {
                    return Err(format!(
                        "Companion service process exited prematurely with status {}",
                        exit_status
                    ));
                }
            }

            #[cfg(unix)]
            {
                if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
                    info!(
                        path = ?socket_path,
                        elapsed_ms = start.elapsed().as_millis(),
                        "Sidecar IPC socket verified ready (KEP-753 Gate passed)"
                    );
                    return Ok(());
                }
            }

            #[cfg(windows)]
            {
                let path_str = socket_path.to_string_lossy();
                if path_str.starts_with(r"\\.\pipe\") {
                    match std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(socket_path)
                    {
                        Ok(_) => {
                            info!(
                                path = ?socket_path,
                                elapsed_ms = start.elapsed().as_millis(),
                                "Sidecar Named Pipe verified ready"
                            );
                            return Ok(());
                        }
                        Err(e) => {
                            if let Some(code) = e.raw_os_error() {
                                if code == 231 || code == 5 {
                                    info!(
                                        path = ?socket_path,
                                        code = code,
                                        elapsed_ms = start.elapsed().as_millis(),
                                        "Sidecar Named Pipe verified ready (active server detected)"
                                    );
                                    return Ok(());
                                }
                            }
                        }
                    }
                } else if socket_path.exists() {
                    info!(
                        path = ?socket_path,
                        elapsed_ms = start.elapsed().as_millis(),
                        "Sidecar socket file verified ready"
                    );
                    return Ok(());
                }
            }

            sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_millis(250));
        }

        Err(format!(
            "Sidecar readiness probe TIMED OUT after {}ms waiting for socket {:?}",
            timeout_ms, socket_path
        ))
    }

    /// Orchestrate KEP-753 sequential execution of primary app and companion sidecar.
    pub async fn start_pair(&self, config: SidecarPairConfig) -> Result<PathBuf, String> {
        let current = self.current_state().await;
        if current == SidecarLifecycleState::Running
            || current == SidecarLifecycleState::SpawningSidecar
            || current == SidecarLifecycleState::ProbingSocketReadiness
        {
            return Err("A sidecar workspace pair is already active".to_string());
        }

        // Prepare safe socket
        *self.state.write().await = SidecarLifecycleState::CleaningStaleSocket;
        let socket_id = &config.project.ipc.socket_id;
        let sock_path = Self::prepare_safe_socket(&config.workspace_root, socket_id)?;
        *self.active_socket.write().await = Some(sock_path.clone());

        // Identify sidecar and primary members
        let sidecar_member = config
            .project
            .sidecars()
            .first()
            .copied()
            .cloned()
            .ok_or_else(|| "No sidecar member found in workspace configuration".to_string())?;

        let primary_member = config
            .project
            .primary_member()
            .cloned()
            .ok_or_else(|| "No primary member found in workspace configuration".to_string())?;

        // KEP-753 Phase I: Spawn Sidecar Process
        *self.state.write().await = SidecarLifecycleState::SpawningSidecar;
        info!(
            sidecar_id = %sidecar_member.id,
            socket = ?sock_path,
            "KEP-753 Phase I: Spawning companion sidecar process"
        );

        let mut sidecar_child = match self
            .spawn_member_process(&config.workspace_root, &sidecar_member, &sock_path, &config)
            .await
        {
            Ok(child) => child,
            Err(e) => {
                *self.state.write().await = SidecarLifecycleState::Failed;
                return Err(format!("Failed to spawn sidecar: {}", e));
            }
        };

        // KEP-753 Readiness Probe Gate (Fail-fast with child process monitoring)
        *self.state.write().await = SidecarLifecycleState::ProbingSocketReadiness;
        let readiness_timeout = config.project.ipc.readiness_timeout_ms;
        let abort_grace =
            Duration::from_millis(config.project.ipc.drain_timeout_ms.clamp(500, 2000));

        if let Err(err) = Self::poll_socket_readiness_with_child(
            &sock_path,
            readiness_timeout,
            Some(&mut sidecar_child),
        )
        .await
        {
            warn!(error = %err, "Readiness probe failed; aborting startup");
            *self.state.write().await = SidecarLifecycleState::Failed;
            // Clean up sidecar process
            Self::terminate_child_process(sidecar_child, abort_grace).await;
            let _ = cleanup_stale_socket(&sock_path);
            return Err(err);
        }

        *self.state.write().await = SidecarLifecycleState::SidecarReady;

        // KEP-753 Phase II: Launch Primary App with socket injected
        *self.state.write().await = SidecarLifecycleState::SpawningPrimary;
        info!(
            primary_id = %primary_member.id,
            socket = ?sock_path,
            "KEP-753 Phase II: Sidecar verified ready; launching primary application"
        );

        let primary_child = match self
            .spawn_member_process(&config.workspace_root, &primary_member, &sock_path, &config)
            .await
        {
            Ok(child) => Some(child),
            Err(e) => {
                warn!(error = %e, "Failed to spawn primary application; tearing down sidecar");
                *self.state.write().await = SidecarLifecycleState::Failed;
                Self::terminate_child_process(sidecar_child, abort_grace).await;
                let _ = cleanup_stale_socket(&sock_path);
                return Err(format!("Failed to spawn primary app: {}", e));
            }
        };

        // Transition to Running
        *self.state.write().await = SidecarLifecycleState::Running;

        let active = ActivePair {
            config,
            socket_path: sock_path.clone(),
            sidecar_child,
            primary_child,
        };

        *self.active_pair.write().await = Some(active);

        // Spawn background supervisor watchdog
        let state_ref = self.state.clone();
        let pair_ref = self.active_pair.clone();
        let socket_ref = self.active_socket.clone();

        tokio::spawn(async move {
            Self::watchdog_loop(state_ref, pair_ref, socket_ref).await;
        });

        Ok(sock_path)
    }

    /// KEP-753 Phase III: Reverse Graceful Teardown Sequence.
    /// Terminates primary application first $\rightarrow$ allows drain window $\rightarrow$ terminates sidecar.
    pub async fn stop_pair(&self) -> Result<(), String> {
        let mut guard = self.active_pair.write().await;
        let Some(mut pair) = guard.take() else {
            *self.state.write().await = SidecarLifecycleState::Stopped;
            return Ok(());
        };

        let drain_timeout = Duration::from_millis(pair.config.project.ipc.drain_timeout_ms);

        // Drain & Terminate Primary App
        *self.state.write().await = SidecarLifecycleState::DrainingPrimary;
        if let Some(primary_child) = pair.primary_child.take() {
            info!("KEP-753 Phase III: Signaling primary application to drain and terminate");
            Self::terminate_child_process(primary_child, drain_timeout).await;
        }

        // Terminate Sidecar Process
        *self.state.write().await = SidecarLifecycleState::TerminatingSidecar;
        info!("KEP-753 Phase III: Terminating companion sidecar process");
        Self::terminate_child_process(pair.sidecar_child, drain_timeout).await;

        // Unlink socket
        let _ = cleanup_stale_socket(&pair.socket_path);
        *self.active_socket.write().await = None;
        *self.state.write().await = SidecarLifecycleState::Stopped;

        info!("Sidecar workspace pair stopped cleanly; zero residual sockets");
        Ok(())
    }

    async fn spawn_member_process(
        &self,
        workspace_root: &Path,
        member: &WorkspaceMember,
        sock_path: &Path,
        config: &SidecarPairConfig,
    ) -> Result<Child, String> {
        let member_dir = if member.path.is_absolute() {
            member.path.clone()
        } else {
            workspace_root.join(&member.path)
        };

        let exec_cmd = member.exec_command.clone().unwrap_or_else(|| {
            if member_dir.join("package.json").is_file() {
                "npm".to_string()
            } else if member_dir.join("src-tauri/Cargo.toml").is_file()
                || member_dir.join("Cargo.toml").is_file()
            {
                "cargo".to_string()
            } else if member_dir.join("requirements.txt").is_file()
                || member_dir.join("pyproject.toml").is_file()
            {
                if cfg!(windows) {
                    "python".to_string()
                } else {
                    "python3".to_string()
                }
            } else {
                match member.role {
                    MemberRole::Primary | MemberRole::Member => {
                        if cfg!(windows) {
                            "cmd.exe".to_string()
                        } else {
                            "/bin/sh".to_string()
                        }
                    }
                    MemberRole::Sidecar | MemberRole::Service => {
                        if cfg!(windows) {
                            "python".to_string()
                        } else {
                            "python3".to_string()
                        }
                    }
                }
            }
        });

        let mut cmd = Command::new(&exec_cmd);

        if let Some(ref args) = member.exec_args {
            cmd.args(args);
        }

        // Put process in its own process group on Unix so children don't become zombies
        #[cfg(unix)]
        cmd.process_group(0);

        // Inject socket environment variable
        cmd.env(&config.project.ipc.env_var, sock_path);
        cmd.env("DMZ_SIDECAR_SOCK", sock_path);
        cmd.current_dir(&member_dir);

        // Pipe stdout and stderr for multiplexed logging
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Process spawn failed for '{}': {}", member.id, e))?;

        let member_id = member.id.clone();
        let color_code = match member.role {
            MemberRole::Primary => "\x1b[36m",                       // Cyan
            MemberRole::Sidecar | MemberRole::Service => "\x1b[35m", // Magenta
            MemberRole::Member => "\x1b[33m",                        // Yellow
        };

        if let Some(stdout) = child.stdout.take() {
            let id = member_id.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    println!("{}[{}]\x1b[0m\t{}", color_code, id, line);
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let id = member_id.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    eprintln!("\x1b[31m[{}]\x1b[0m\t{}", id, line);
                }
            });
        }

        Ok(child)
    }

    async fn terminate_child_process(mut child: Child, grace_period: Duration) {
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                unsafe {
                    // Send SIGTERM to entire process group
                    libc::killpg(pid as i32, libc::SIGTERM);
                }
            }
        }

        #[cfg(windows)]
        {
            let _ = child.kill().await;
            return;
        }

        tokio::select! {
            _ = child.wait() => {
                info!("Process exited gracefully");
            }
            _ = sleep(grace_period) => {
                warn!("Process failed to exit gracefully within {:?}; sending SIGKILL to process group", grace_period);
                #[cfg(unix)]
                {
                    if let Some(pid) = child.id() {
                        unsafe {
                            libc::killpg(pid as i32, libc::SIGKILL);
                        }
                    }
                }
                let _ = child.kill().await;
            }
        }
    }

    /// Awaits termination of the primary child process if active
    pub async fn wait_for_exit(&self) -> Result<std::process::ExitStatus, String> {
        let mut child_to_wait = {
            let mut guard = self.active_pair.write().await;
            if let Some(ref mut pair) = *guard {
                pair.primary_child.take()
            } else {
                None
            }
        };

        if let Some(ref mut child) = child_to_wait {
            child.wait().await.map_err(|e| format!("Wait error: {}", e))
        } else {
            Err("No active primary child to wait on".to_string())
        }
    }

    async fn watchdog_loop(
        state: Arc<RwLock<SidecarLifecycleState>>,
        pair: Arc<RwLock<Option<ActivePair>>>,
        socket: Arc<RwLock<Option<PathBuf>>>,
    ) {
        loop {
            sleep(Duration::from_millis(500)).await;

            let mut guard = pair.write().await;
            let Some(ref mut active) = *guard else {
                break;
            };

            // Check whether primary app exited
            if let Some(ref mut prim) = active.primary_child {
                if let Ok(Some(status)) = prim.try_wait() {
                    info!(
                        status = ?status.code(),
                        "Primary application exited; triggering sidecar teardown"
                    );
                    drop(guard);
                    // Trigger teardown
                    let mut guard = pair.write().await;
                    if let Some(pair_val) = guard.take() {
                        let drain_timeout =
                            Duration::from_millis(pair_val.config.project.ipc.drain_timeout_ms);
                        *state.write().await = SidecarLifecycleState::TerminatingSidecar;
                        Self::terminate_child_process(pair_val.sidecar_child, drain_timeout).await;
                        let _ = cleanup_stale_socket(&pair_val.socket_path);
                        *socket.write().await = None;
                        *state.write().await = SidecarLifecycleState::Stopped;
                    }
                    break;
                }
            }

            // Check whether sidecar crashed unexpectedly
            if let Ok(Some(status)) = active.sidecar_child.try_wait() {
                error!(
                    exit_code = ?status.code(),
                    "Companion sidecar crashed unexpectedly while primary application was active!"
                );
                *state.write().await = SidecarLifecycleState::Failed;
                break;
            }
        }
    }
}
