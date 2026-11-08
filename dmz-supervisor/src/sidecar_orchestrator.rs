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

struct ManagedChild {
    child: Child,
    #[cfg(unix)]
    pgid: Option<i32>,
}

impl ManagedChild {
    fn new(child: Child) -> Self {
        #[cfg(unix)]
        let pgid = child.id().map(|id| id as i32);
        Self {
            child,
            #[cfg(unix)]
            pgid,
        }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    async fn graceful_shutdown(&mut self, grace_period: Duration) {
        if let Ok(Some(_)) = self.child.try_wait() {
            return;
        }

        #[cfg(unix)]
        {
            if let Some(pgid) = self.pgid {
                // SAFETY: `pgid` is a positive process group ID created via `cmd.process_group(0)` for this child.
                unsafe {
                    // Send SIGTERM to entire process group
                    libc::killpg(pgid, libc::SIGTERM);
                }
            }
        }

        #[cfg(windows)]
        {
            let _ = self.child.kill().await;
            return;
        }

        tokio::select! {
            _ = self.child.wait() => {
                info!("Process exited gracefully");
            }
            _ = sleep(grace_period) => {
                warn!(
                    "Process failed to exit gracefully within {:?}; sending SIGKILL to process group",
                    grace_period
                );
                #[cfg(unix)]
                {
                    if let Some(pgid) = self.pgid {
                        // SAFETY: `pgid` is a positive process group ID created via `cmd.process_group(0)` for this child.
                        unsafe {
                            libc::killpg(pgid, libc::SIGKILL);
                        }
                    }
                }
                let _ = self.child.kill().await;
            }
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            #[cfg(unix)]
            if let Some(pgid) = self.pgid {
                // SAFETY: `pgid` is a positive process group ID created via `cmd.process_group(0)` for this child.
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
            let _ = self.child.start_kill();
        }
    }
}

struct ActivePair {
    config: SidecarPairConfig,
    socket_path: PathBuf,
    sidecar_children: Vec<(String, ManagedChild)>,
    primary_child: Option<ManagedChild>,
}

#[derive(Clone)]
pub struct SidecarOrchestrator {
    state: Arc<RwLock<SidecarLifecycleState>>,
    active_pair: Arc<RwLock<Option<ActivePair>>>,
    active_socket: Arc<RwLock<Option<PathBuf>>>,
    last_error: Arc<RwLock<Option<String>>>,
    exit_tx: Arc<tokio::sync::watch::Sender<Option<std::process::ExitStatus>>>,
    exit_rx: tokio::sync::watch::Receiver<Option<std::process::ExitStatus>>,
}

impl Default for SidecarOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SidecarOrchestrator {
    pub fn new() -> Self {
        let (exit_tx, exit_rx) = tokio::sync::watch::channel(None);
        Self {
            state: Arc::new(RwLock::new(SidecarLifecycleState::Idle)),
            active_pair: Arc::new(RwLock::new(None)),
            active_socket: Arc::new(RwLock::new(None)),
            last_error: Arc::new(RwLock::new(None)),
            exit_tx: Arc::new(exit_tx),
            exit_rx,
        }
    }

    pub async fn current_state(&self) -> SidecarLifecycleState {
        *self.state.read().await
    }

    pub async fn current_socket_path(&self) -> Option<PathBuf> {
        self.active_socket.read().await.clone()
    }

    pub async fn last_error(&self) -> Option<String> {
        self.last_error.read().await.clone()
    }

    pub async fn reset_state(&self) {
        *self.state.write().await = SidecarLifecycleState::Idle;
        *self.last_error.write().await = None;
        let _ = self.exit_tx.send_replace(None);
        let mut guard = self.active_pair.write().await;
        if let Some(mut pair) = guard.take() {
            if let Some(mut prim) = pair.primary_child.take() {
                prim.graceful_shutdown(Duration::from_millis(500)).await;
            }
            for (_, mut sc) in pair.sidecar_children.into_iter().rev() {
                sc.graceful_shutdown(Duration::from_millis(500)).await;
            }
            let _ = cleanup_stale_socket(&pair.socket_path);
        }
        *self.active_socket.write().await = None;
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
        if timeout_ms == 0 {
            // Zero timeout explicitly requests skipping socket probe (process alive check only)
            if let Some(ref mut c) = child {
                if let Ok(Some(exit_status)) = c.try_wait() {
                    return Err(format!(
                        "Companion service process exited prematurely with status {}",
                        exit_status
                    ));
                }
            }
            info!(
                path = ?socket_path,
                "Readiness gate timeout is 0ms; socket probe skipped, verified process active"
            );
            return Ok(());
        }

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
                let connected = tokio::time::timeout(
                    Duration::from_millis(200),
                    tokio::net::UnixStream::connect(socket_path),
                )
                .await
                .is_ok_and(|res| res.is_ok());

                if connected {
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

    async fn poll_socket_readiness_for_managed_children(
        socket_path: &Path,
        timeout_ms: u64,
        children: &mut [(String, ManagedChild)],
    ) -> Result<(), String> {
        if timeout_ms == 0 {
            for (id, child) in children.iter_mut() {
                if let Ok(Some(exit_status)) = child.try_wait() {
                    return Err(format!(
                        "Companion service '{}' exited prematurely with status {}",
                        id, exit_status
                    ));
                }
            }
            info!(
                path = ?socket_path,
                "Readiness gate timeout is 0ms; socket probe skipped, verified companion process(es) active"
            );
            return Ok(());
        }

        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        let mut backoff = Duration::from_millis(25);

        while start.elapsed() < timeout {
            for (id, child) in children.iter_mut() {
                if let Ok(Some(exit_status)) = child.try_wait() {
                    return Err(format!(
                        "Companion service '{}' exited prematurely with status {}",
                        id, exit_status
                    ));
                }
            }

            #[cfg(unix)]
            {
                let connected = tokio::time::timeout(
                    Duration::from_millis(200),
                    tokio::net::UnixStream::connect(socket_path),
                )
                .await
                .is_ok_and(|res| res.is_ok());

                if connected {
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

    /// Orchestrate KEP-753 sequential execution of primary app and companion sidecars.
    pub async fn start_pair(&self, config: SidecarPairConfig) -> Result<PathBuf, String> {
        let current = self.current_state().await;
        if current == SidecarLifecycleState::Running
            || current == SidecarLifecycleState::SpawningSidecar
            || current == SidecarLifecycleState::ProbingSocketReadiness
        {
            return Err("A sidecar workspace pair is already active".to_string());
        }

        // Reset previous error & exit channel
        *self.last_error.write().await = None;
        let _ = self.exit_tx.send_replace(None);

        // Prepare safe socket
        *self.state.write().await = SidecarLifecycleState::CleaningStaleSocket;
        let socket_id = &config.project.ipc.socket_id;
        let sock_path = match Self::prepare_safe_socket(&config.workspace_root, socket_id) {
            Ok(p) => p,
            Err(e) => {
                *self.state.write().await = SidecarLifecycleState::Failed;
                *self.last_error.write().await = Some(e.clone());
                return Err(e);
            }
        };
        *self.active_socket.write().await = Some(sock_path.clone());

        // Identify sidecar and primary members
        let sidecar_members: Vec<WorkspaceMember> =
            config.project.sidecars().into_iter().cloned().collect();
        if sidecar_members.is_empty() {
            let err = format!(
                "No sidecar or service member found in workspace '{}' (has {} members). At least one member must have role 'sidecar' or 'service'.",
                config.project.name,
                config.project.members.len()
            );
            *self.state.write().await = SidecarLifecycleState::Failed;
            *self.last_error.write().await = Some(err.clone());
            return Err(err);
        }

        let primary_member = match config.project.primary_member().cloned() {
            Some(m) => m,
            None => {
                let err = format!(
                    "No primary member found in workspace '{}'. At least one member must have role 'primary'.",
                    config.project.name
                );
                *self.state.write().await = SidecarLifecycleState::Failed;
                *self.last_error.write().await = Some(err.clone());
                return Err(err);
            }
        };

        let abort_grace =
            Duration::from_millis(config.project.ipc.drain_timeout_ms.clamp(500, 2000));

        // KEP-753 Phase I: Spawn all declared Sidecar / Service Processes
        *self.state.write().await = SidecarLifecycleState::SpawningSidecar;
        let mut sidecar_children: Vec<(String, ManagedChild)> =
            Vec::with_capacity(sidecar_members.len());

        for sidecar_member in &sidecar_members {
            info!(
                sidecar_id = %sidecar_member.id,
                socket = ?sock_path,
                "KEP-753 Phase I: Spawning companion sidecar process"
            );

            match self
                .spawn_member_process(&config.workspace_root, sidecar_member, &sock_path, &config)
                .await
            {
                Ok(child) => {
                    sidecar_children.push((sidecar_member.id.clone(), child));
                }
                Err(e) => {
                    let err = format!("Failed to spawn sidecar '{}': {}", sidecar_member.id, e);
                    *self.state.write().await = SidecarLifecycleState::Failed;
                    *self.last_error.write().await = Some(err.clone());
                    for (_, mut prev_sc) in sidecar_children.into_iter().rev() {
                        prev_sc.graceful_shutdown(abort_grace).await;
                    }
                    let _ = cleanup_stale_socket(&sock_path);
                    return Err(err);
                }
            }
        }

        // KEP-753 Readiness Probe Gate (Fail-fast with child process monitoring)
        *self.state.write().await = SidecarLifecycleState::ProbingSocketReadiness;
        let readiness_timeout = config.project.ipc.readiness_timeout_ms;

        if let Err(err) = Self::poll_socket_readiness_for_managed_children(
            &sock_path,
            readiness_timeout,
            &mut sidecar_children,
        )
        .await
        {
            warn!(error = %err, "Readiness probe failed; aborting startup");
            *self.state.write().await = SidecarLifecycleState::Failed;
            *self.last_error.write().await = Some(err.clone());
            for (_, mut sc) in sidecar_children.into_iter().rev() {
                sc.graceful_shutdown(abort_grace).await;
            }
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
                let err = format!("Failed to spawn primary app '{}': {}", primary_member.id, e);
                warn!(error = %e, "Failed to spawn primary application; tearing down sidecar(s)");
                *self.state.write().await = SidecarLifecycleState::Failed;
                *self.last_error.write().await = Some(err.clone());
                for (_, mut sc) in sidecar_children.into_iter().rev() {
                    sc.graceful_shutdown(abort_grace).await;
                }
                let _ = cleanup_stale_socket(&sock_path);
                return Err(err);
            }
        };

        // Transition to Running
        *self.state.write().await = SidecarLifecycleState::Running;
        *self.last_error.write().await = None;

        let active = ActivePair {
            config,
            socket_path: sock_path.clone(),
            sidecar_children,
            primary_child,
        };

        *self.active_pair.write().await = Some(active);

        // Spawn background supervisor watchdog
        let state_ref = self.state.clone();
        let pair_ref = self.active_pair.clone();
        let socket_ref = self.active_socket.clone();
        let last_error_ref = self.last_error.clone();
        let exit_tx_ref = self.exit_tx.clone();

        tokio::spawn(async move {
            Self::watchdog_loop(state_ref, pair_ref, socket_ref, last_error_ref, exit_tx_ref).await;
        });

        Ok(sock_path)
    }

    /// KEP-753 Phase III: Reverse Graceful Teardown Sequence.
    /// Terminates primary application first $\rightarrow$ allows drain window $\rightarrow$ terminates sidecars.
    pub async fn stop_pair(&self) -> Result<(), String> {
        let mut guard = self.active_pair.write().await;
        let Some(mut pair) = guard.take() else {
            *self.state.write().await = SidecarLifecycleState::Stopped;
            self.exit_tx.send_modify(|_| {});
            return Ok(());
        };
        drop(guard);

        let drain_timeout = Duration::from_millis(pair.config.project.ipc.drain_timeout_ms);

        // Drain & Terminate Primary App
        *self.state.write().await = SidecarLifecycleState::DrainingPrimary;
        if let Some(mut primary_child) = pair.primary_child.take() {
            info!("KEP-753 Phase III: Signaling primary application to drain and terminate");
            primary_child.graceful_shutdown(drain_timeout).await;
        }

        // Terminate Sidecar Processes in reverse startup order
        *self.state.write().await = SidecarLifecycleState::TerminatingSidecar;
        for (id, mut sc) in pair.sidecar_children.into_iter().rev() {
            info!(
                sidecar_id = %id,
                "KEP-753 Phase III: Terminating companion sidecar process"
            );
            sc.graceful_shutdown(drain_timeout).await;
        }

        // Unlink socket
        let _ = cleanup_stale_socket(&pair.socket_path);
        *self.active_socket.write().await = None;
        *self.state.write().await = SidecarLifecycleState::Stopped;
        self.exit_tx.send_modify(|_| {});

        info!("Sidecar workspace pair stopped cleanly; zero residual sockets");
        Ok(())
    }

    async fn spawn_member_process(
        &self,
        workspace_root: &Path,
        member: &WorkspaceMember,
        sock_path: &Path,
        config: &SidecarPairConfig,
    ) -> Result<ManagedChild, String> {
        let member_dir = config
            .project
            .resolve_member_layouts(workspace_root, None)
            .into_iter()
            .find(|l| l.member.id == member.id)
            .map(|l| l.host_path)
            .unwrap_or_else(|| {
                if member.path.is_absolute() {
                    member.path.clone()
                } else {
                    workspace_root.join(&member.path)
                }
            });
        let member_dir = std::fs::canonicalize(&member_dir).unwrap_or(member_dir);
        if !member_dir.exists() {
            return Err(format!(
                "Workspace member directory does not exist: {:?}",
                member_dir
            ));
        }

        let (exec_cmd, default_args) = member.resolve_command(&member_dir, workspace_root)?;

        let mut cmd = Command::new(&exec_cmd);
        if !default_args.is_empty() {
            cmd.args(&default_args);
        }

        // Put process in its own process group on Unix so children don't become zombies
        #[cfg(unix)]
        cmd.process_group(0);

        cmd.current_dir(&member_dir);

        // Inject unified hermetic environment (toolchains, .venv, node_modules/.bin, IPC socket, offline flags)
        let envs = config.project.build_member_env(
            workspace_root,
            &member_dir,
            sock_path,
            config.allow_network,
        );
        cmd.envs(envs);
        cmd.env_remove("PYTHONHOME");

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

        Ok(ManagedChild::new(child))
    }

    /// Awaits termination of the primary child process without removing its handle from `ActivePair`.
    /// This ensures `stop_pair()` can still drain and terminate the primary process group on `Ctrl+C`.
    pub async fn wait_for_exit(&self) -> Result<std::process::ExitStatus, String> {
        let mut rx = self.exit_rx.clone();

        if let Some(status) = *rx.borrow_and_update() {
            return Ok(status);
        }

        {
            let guard = self.active_pair.read().await;
            let has_primary = guard.as_ref().is_some_and(|p| p.primary_child.is_some());
            if !has_primary {
                if let Some(status) = *rx.borrow() {
                    return Ok(status);
                }
                return Err("No active primary child to wait on".to_string());
            }
        }

        loop {
            if rx.changed().await.is_err() {
                break;
            }
            if let Some(status) = *rx.borrow_and_update() {
                return Ok(status);
            }
            let current_state = *self.state.read().await;
            if current_state == SidecarLifecycleState::Failed {
                let err = self
                    .last_error
                    .read()
                    .await
                    .clone()
                    .unwrap_or_else(|| "Companion sidecar failed".to_string());
                return Err(err);
            }
            if current_state == SidecarLifecycleState::Stopped {
                if let Some(status) = *rx.borrow() {
                    return Ok(status);
                }
                return Err("Workspace pair stopped".to_string());
            }
        }

        Err("Watchdog channel closed before primary exit".to_string())
    }

    async fn watchdog_loop(
        state: Arc<RwLock<SidecarLifecycleState>>,
        pair: Arc<RwLock<Option<ActivePair>>>,
        socket: Arc<RwLock<Option<PathBuf>>>,
        last_error: Arc<RwLock<Option<String>>>,
        exit_tx: Arc<tokio::sync::watch::Sender<Option<std::process::ExitStatus>>>,
    ) {
        loop {
            sleep(Duration::from_millis(50)).await;

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
                    let pair_val = guard.take().unwrap();
                    drop(guard);

                    let drain_timeout =
                        Duration::from_millis(pair_val.config.project.ipc.drain_timeout_ms);
                    *state.write().await = SidecarLifecycleState::TerminatingSidecar;
                    for (_, mut sc) in pair_val.sidecar_children.into_iter().rev() {
                        sc.graceful_shutdown(drain_timeout).await;
                    }
                    let _ = cleanup_stale_socket(&pair_val.socket_path);
                    *socket.write().await = None;
                    *state.write().await = SidecarLifecycleState::Stopped;
                    let _ = exit_tx.send(Some(status));
                    break;
                }
            }

            // Check whether any companion sidecar crashed unexpectedly
            let mut crashed_sidecar = None;
            for (id, sc) in active.sidecar_children.iter_mut() {
                if let Ok(Some(status)) = sc.try_wait() {
                    crashed_sidecar = Some((id.clone(), status));
                    break;
                }
            }

            if let Some((crashed_id, status)) = crashed_sidecar {
                let err = format!(
                    "Companion sidecar '{}' crashed unexpectedly with status {:?}",
                    crashed_id,
                    status.code()
                );
                error!(
                    sidecar_id = %crashed_id,
                    exit_code = ?status.code(),
                    "Companion sidecar crashed unexpectedly while primary application was active!"
                );
                let mut pair_val = guard.take().unwrap();
                drop(guard);

                let drain_timeout =
                    Duration::from_millis(pair_val.config.project.ipc.drain_timeout_ms);
                if let Some(mut prim) = pair_val.primary_child.take() {
                    prim.graceful_shutdown(drain_timeout).await;
                }
                for (_, mut sc) in pair_val.sidecar_children.into_iter().rev() {
                    sc.graceful_shutdown(drain_timeout).await;
                }
                let _ = cleanup_stale_socket(&pair_val.socket_path);
                *socket.write().await = None;
                *last_error.write().await = Some(err);
                *state.write().await = SidecarLifecycleState::Failed;
                exit_tx.send_modify(|_| {});
                break;
            }
        }
    }
}
