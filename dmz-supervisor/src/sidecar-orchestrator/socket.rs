use dmz_core::workspace::{
    cleanup_stale_socket as core_cleanup_stale_socket, safe_socket_path as core_safe_socket_path,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Child;
use tokio::time::sleep;
use tracing::{info, warn};

use super::child::ManagedChild;
use super::types::SidecarOrchestratorError;

pub use dmz_core::workspace::{cleanup_stale_socket, safe_socket_path};

/// Prepare POSIX safe socket path and remove stale dead sockets from previous runs
pub fn prepare_safe_socket(
    workspace: &Path,
    socket_id: &str,
) -> Result<PathBuf, SidecarOrchestratorError> {
    let sock_path = core_safe_socket_path(workspace, socket_id);

    let is_pipe = sock_path.to_string_lossy().starts_with(r"\\.\pipe\");
    if !is_pipe {
        if let Some(parent) = sock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SidecarOrchestratorError::SocketPreparation(format!(
                    "Failed to create socket directory {:?}: {}",
                    parent, e
                ))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                {
                    warn!(
                        dir = ?parent,
                        error = %e,
                        "Failed to set 0o700 permissions on sidecar socket directory"
                    );
                }
            }
        }
    }

    if let Err(e) = core_cleanup_stale_socket(&sock_path) {
        warn!(
            socket = ?sock_path,
            error = %e,
            "Failed to clean up stale sidecar socket prior to launch"
        );
    }
    Ok(sock_path)
}

/// Actively probe socket connectivity with exponential backoff (KEP-753 Gate).
/// Prevent primary app from starting before the sidecar socket is ready.
pub async fn poll_socket_readiness(
    socket_path: &Path,
    timeout_ms: u64,
) -> Result<(), SidecarOrchestratorError> {
    poll_socket_readiness_with_child(socket_path, timeout_ms, None).await
}

/// Fail-fast probe: polls socket readiness while monitoring companion child process status.
pub async fn poll_socket_readiness_with_child(
    socket_path: &Path,
    timeout_ms: u64,
    mut child: Option<&mut Child>,
) -> Result<(), SidecarOrchestratorError> {
    if timeout_ms == 0 {
        // Zero timeout explicitly requests skipping socket probe (process alive check only)
        if let Some(ref mut c) = child {
            if let Ok(Some(exit_status)) = c.try_wait() {
                return Err(SidecarOrchestratorError::CompanionExitedPrematurely {
                    id: "companion".to_string(),
                    status: exit_status,
                    stderr_info: String::new(),
                });
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
                return Err(SidecarOrchestratorError::CompanionExitedPrematurely {
                    id: "companion".to_string(),
                    status: exit_status,
                    stderr_info: String::new(),
                });
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

    Err(SidecarOrchestratorError::ReadinessTimeout {
        timeout_ms,
        socket_path: socket_path.to_path_buf(),
    })
}

pub async fn poll_socket_readiness_for_managed_children(
    socket_path: &Path,
    timeout_ms: u64,
    children: &mut [(String, ManagedChild)],
) -> Result<(), SidecarOrchestratorError> {
    if timeout_ms == 0 {
        for (id, child) in children.iter_mut() {
            if let Ok(Some(exit_status)) = child.try_wait() {
                if !exit_status.success() {
                    let stderr_info = child.recent_stderr();
                    return Err(SidecarOrchestratorError::CompanionExitedPrematurely {
                        id: id.clone(),
                        status: exit_status,
                        stderr_info,
                    });
                }
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
        let mut all_exited_cleanly = !children.is_empty();
        for (id, child) in children.iter_mut() {
            match child.try_wait() {
                Ok(Some(exit_status)) => {
                    if !exit_status.success() {
                        let stderr_info = child.recent_stderr();
                        return Err(SidecarOrchestratorError::CompanionExitedPrematurely {
                            id: id.clone(),
                            status: exit_status,
                            stderr_info,
                        });
                    }
                }
                _ => {
                    all_exited_cleanly = false;
                }
            }
        }

        if all_exited_cleanly {
            info!(
                "Companion service(s) completed one-shot execution with exit code 0; proceeding to primary launch"
            );
            return Ok(());
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

    Err(SidecarOrchestratorError::ReadinessTimeout {
        timeout_ms,
        socket_path: socket_path.to_path_buf(),
    })
}
