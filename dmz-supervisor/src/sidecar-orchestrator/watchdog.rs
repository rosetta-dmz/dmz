use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{error, info};

use super::child::ManagedChild;
use super::socket::cleanup_stale_socket;
use super::types::{ActivePair, SidecarLifecycleState};

pub async fn terminate_children_concurrently(
    children: Vec<(String, ManagedChild)>,
    drain_timeout: Duration,
) {
    if children.is_empty() {
        return;
    }
    let mut join_set = tokio::task::JoinSet::new();
    for (id, mut sc) in children {
        join_set.spawn(async move {
            info!(
                sidecar_id = %id,
                "KEP-753 Phase III: Terminating companion sidecar process"
            );
            sc.graceful_shutdown(drain_timeout).await;
        });
    }
    while join_set.join_next().await.is_some() {}
}

pub async fn watchdog_loop(
    state: Arc<RwLock<SidecarLifecycleState>>,
    pair: Arc<RwLock<Option<ActivePair>>>,
    socket: Arc<RwLock<Option<PathBuf>>>,
    last_error: Arc<RwLock<Option<String>>>,
    exit_tx: Arc<tokio::sync::watch::Sender<Option<std::process::ExitStatus>>>,
) {
    loop {
        sleep(Duration::from_millis(75)).await;

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
                let Some(pair_val) = guard.take() else {
                    drop(guard);
                    break;
                };
                drop(guard);

                let drain_timeout =
                    Duration::from_millis(pair_val.config.project.ipc.drain_timeout_ms);
                *state.write().await = SidecarLifecycleState::TerminatingSidecar;
                terminate_children_concurrently(pair_val.sidecar_children, drain_timeout).await;
                if let Err(e) = cleanup_stale_socket(&pair_val.socket_path) {
                    tracing::warn!(
                        socket = ?pair_val.socket_path,
                        error = %e,
                        "Failed to remove stale sidecar socket during primary exit teardown"
                    );
                }
                *socket.write().await = None;
                *state.write().await = SidecarLifecycleState::Stopped;
                let _ = exit_tx.send(Some(status));
                break;
            }
        }

        // Check whether any companion sidecar crashed unexpectedly (non-zero exit)
        let mut crashed_sidecar = None;
        let mut finished_clean_idx = Vec::new();
        for (idx, (id, sc)) in active.sidecar_children.iter_mut().enumerate() {
            if let Ok(Some(status)) = sc.try_wait() {
                if status.success() {
                    finished_clean_idx.push(idx);
                } else {
                    crashed_sidecar = Some((id.clone(), status, sc.recent_stderr()));
                    break;
                }
            }
        }
        for idx in finished_clean_idx.into_iter().rev() {
            active.sidecar_children.remove(idx);
        }

        if let Some((crashed_id, status, stderr_info)) = crashed_sidecar {
            let err = format!(
                "Companion sidecar '{}' crashed unexpectedly with status {:?}{}",
                crashed_id,
                status.code(),
                stderr_info
            );
            error!(
                sidecar_id = %crashed_id,
                exit_code = ?status.code(),
                "Companion sidecar crashed unexpectedly while primary application was active!"
            );
            let Some(mut pair_val) = guard.take() else {
                drop(guard);
                break;
            };
            drop(guard);

            let drain_timeout = Duration::from_millis(pair_val.config.project.ipc.drain_timeout_ms);
            if let Some(mut prim) = pair_val.primary_child.take() {
                prim.graceful_shutdown(drain_timeout).await;
            }
            terminate_children_concurrently(pair_val.sidecar_children, drain_timeout).await;
            if let Err(e) = cleanup_stale_socket(&pair_val.socket_path) {
                tracing::warn!(
                    socket = ?pair_val.socket_path,
                    error = %e,
                    "Failed to remove stale sidecar socket after sidecar crash teardown"
                );
            }
            *socket.write().await = None;
            *last_error.write().await = Some(err);
            *state.write().await = SidecarLifecycleState::Failed;
            exit_tx.send_modify(|_| {});
            break;
        }
    }
}
