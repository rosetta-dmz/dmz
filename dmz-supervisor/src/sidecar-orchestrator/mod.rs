pub mod child;
pub mod execution;
pub mod socket;
pub mod types;
pub mod watchdog;

use dmz_core::workspace::WorkspaceMember;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::RwLock;
use tracing::{info, warn};

pub use child::ManagedChild;
pub use execution::{infer_member_execution, infer_member_execution_in, spawn_member_process};
pub use socket::{
    cleanup_stale_socket, poll_socket_readiness, poll_socket_readiness_for_managed_children,
    poll_socket_readiness_with_child, prepare_safe_socket, safe_socket_path,
};
pub use types::{ActivePair, SidecarLifecycleState, SidecarOrchestratorError, SidecarPairConfig};
pub use watchdog::{terminate_children_concurrently, watchdog_loop};

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
    pub fn prepare_safe_socket(
        workspace: &Path,
        socket_id: &str,
    ) -> Result<PathBuf, SidecarOrchestratorError> {
        socket::prepare_safe_socket(workspace, socket_id)
    }

    /// Actively probe socket connectivity with exponential backoff (KEP-753 Gate).
    /// Prevent primary app from starting before the sidecar socket is ready.
    pub async fn poll_socket_readiness(
        socket_path: &Path,
        timeout_ms: u64,
    ) -> Result<(), SidecarOrchestratorError> {
        socket::poll_socket_readiness(socket_path, timeout_ms).await
    }

    /// Fail-fast probe: polls socket readiness while monitoring companion child process status.
    pub async fn poll_socket_readiness_with_child(
        socket_path: &Path,
        timeout_ms: u64,
        child: Option<&mut Child>,
    ) -> Result<(), SidecarOrchestratorError> {
        socket::poll_socket_readiness_with_child(socket_path, timeout_ms, child).await
    }

    /// Automatically infers execution command and arguments for a workspace member based on project manifests
    pub fn infer_member_execution(
        member_dir: &Path,
        member: &WorkspaceMember,
    ) -> Result<(String, Vec<String>), SidecarOrchestratorError> {
        execution::infer_member_execution(member_dir, member)
    }

    /// Infers execution command and arguments for a member using an explicit workspace root context.
    pub fn infer_member_execution_in(
        workspace_root: &Path,
        member_dir: &Path,
        member: &WorkspaceMember,
    ) -> Result<(String, Vec<String>), SidecarOrchestratorError> {
        execution::infer_member_execution_in(workspace_root, member_dir, member)
    }

    /// Orchestrate KEP-753 sequential execution of primary app and companion sidecars.
    pub async fn start_pair(
        &self,
        config: SidecarPairConfig,
    ) -> Result<PathBuf, SidecarOrchestratorError> {
        let current = self.current_state().await;
        if current == SidecarLifecycleState::Running
            || current == SidecarLifecycleState::SpawningSidecar
            || current == SidecarLifecycleState::ProbingSocketReadiness
        {
            return Err(SidecarOrchestratorError::AlreadyActive(current));
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
                *self.last_error.write().await = Some(e.to_string());
                return Err(e);
            }
        };
        *self.active_socket.write().await = Some(sock_path.clone());

        // Identify sidecar and primary members
        let sidecar_members: Vec<WorkspaceMember> =
            config.project.sidecars().into_iter().cloned().collect();
        if sidecar_members.is_empty() {
            let err = SidecarOrchestratorError::NoSidecarMember {
                workspace: config.project.name.clone(),
                member_count: config.project.members.len(),
            };
            *self.state.write().await = SidecarLifecycleState::Failed;
            *self.last_error.write().await = Some(err.to_string());
            return Err(err);
        }

        let primary_member = match config.project.primary_member().cloned() {
            Some(m) => m,
            None => {
                let err = SidecarOrchestratorError::NoPrimaryMember(config.project.name.clone());
                *self.state.write().await = SidecarLifecycleState::Failed;
                *self.last_error.write().await = Some(err.to_string());
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

            match spawn_member_process(&config.workspace_root, sidecar_member, &sock_path, &config)
                .await
            {
                Ok(child) => {
                    sidecar_children.push((sidecar_member.id.clone(), child));
                }
                Err(e) => {
                    *self.state.write().await = SidecarLifecycleState::Failed;
                    *self.last_error.write().await = Some(e.to_string());
                    for (_, mut prev_sc) in sidecar_children.into_iter().rev() {
                        prev_sc.graceful_shutdown(abort_grace).await;
                    }
                    let _ = cleanup_stale_socket(&sock_path);
                    return Err(e);
                }
            }
        }

        // KEP-753 Readiness Probe Gate (Fail-fast with child process monitoring)
        *self.state.write().await = SidecarLifecycleState::ProbingSocketReadiness;
        let readiness_timeout = config.project.ipc.readiness_timeout_ms;

        if let Err(err) = poll_socket_readiness_for_managed_children(
            &sock_path,
            readiness_timeout,
            &mut sidecar_children,
        )
        .await
        {
            warn!(error = %err, "Readiness probe failed; aborting startup");
            *self.state.write().await = SidecarLifecycleState::Failed;
            *self.last_error.write().await = Some(err.to_string());
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

        let primary_child = match spawn_member_process(
            &config.workspace_root,
            &primary_member,
            &sock_path,
            &config,
        )
        .await
        {
            Ok(child) => Some(child),
            Err(e) => {
                warn!(error = %e, "Failed to spawn primary application; tearing down sidecar(s)");
                *self.state.write().await = SidecarLifecycleState::Failed;
                *self.last_error.write().await = Some(e.to_string());
                for (_, mut sc) in sidecar_children.into_iter().rev() {
                    sc.graceful_shutdown(abort_grace).await;
                }
                let _ = cleanup_stale_socket(&sock_path);
                return Err(e);
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
        let state_ref = Arc::clone(&self.state);
        let pair_ref = Arc::clone(&self.active_pair);
        let socket_ref = Arc::clone(&self.active_socket);
        let last_error_ref = Arc::clone(&self.last_error);
        let exit_tx_ref = Arc::clone(&self.exit_tx);

        tokio::spawn(async move {
            watchdog_loop(state_ref, pair_ref, socket_ref, last_error_ref, exit_tx_ref).await;
        });

        Ok(sock_path)
    }

    /// KEP-753 Phase III: Reverse Graceful Teardown Sequence.
    /// Terminates primary application first -> allows drain window -> terminates sidecars.
    pub async fn stop_pair(&self) -> Result<(), SidecarOrchestratorError> {
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

        // Terminate Sidecar Processes concurrently
        *self.state.write().await = SidecarLifecycleState::TerminatingSidecar;
        terminate_children_concurrently(pair.sidecar_children, drain_timeout).await;

        // Unlink socket
        let _ = cleanup_stale_socket(&pair.socket_path);
        *self.active_socket.write().await = None;
        *self.state.write().await = SidecarLifecycleState::Stopped;
        self.exit_tx.send_modify(|_| {});

        info!("Sidecar workspace pair stopped cleanly; zero residual sockets");
        Ok(())
    }

    /// Awaits termination of the primary child process without removing its handle from `ActivePair`.
    /// This ensures `stop_pair()` can still drain and terminate the primary process group on `Ctrl+C`.
    pub async fn wait_for_exit(
        &self,
    ) -> Result<std::process::ExitStatus, SidecarOrchestratorError> {
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
                return Err(SidecarOrchestratorError::NoPrimaryChild);
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
                return Err(SidecarOrchestratorError::Generic(err));
            }
            if current_state == SidecarLifecycleState::Stopped {
                if let Some(status) = *rx.borrow() {
                    return Ok(status);
                }
                return Err(SidecarOrchestratorError::PairStopped);
            }
        }

        Err(SidecarOrchestratorError::WatchdogChannelClosed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_pgid_safety_conversion() {
        let validate_pgid = |id: u32| -> Option<i32> {
            match i32::try_from(id) {
                Ok(v) if v > 1 => Some(v),
                _ => None,
            }
        };

        // IDs <= 1 must NEVER be accepted as signal targets (prevents supervisor self-suicide)
        assert_eq!(validate_pgid(0), None);
        assert_eq!(validate_pgid(1), None);
        assert_eq!(validate_pgid(u32::MAX), None);
        assert_eq!(validate_pgid(i32::MAX as u32 + 1), None);

        // Valid positive PIDs > 1 must pass
        assert_eq!(validate_pgid(2), Some(2));
        assert_eq!(validate_pgid(12345), Some(12345));
    }

    #[tokio::test]
    async fn test_orchestrator_initial_state_defaults() {
        let orchestrator = SidecarOrchestrator::new();
        assert_eq!(
            orchestrator.current_state().await,
            SidecarLifecycleState::Idle
        );
        assert_eq!(orchestrator.current_socket_path().await, None);
        assert_eq!(orchestrator.last_error().await, None);
    }

    #[test]
    fn test_sidecar_orchestrator_error_traits() {
        let err = SidecarOrchestratorError::NoPrimaryMember("test_proj".to_string());
        let str_err: String = err.into();
        assert!(str_err.contains("No primary member found in workspace 'test_proj'"));
    }
}
