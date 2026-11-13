use std::sync::Arc;
use std::time::Duration;
use tokio::process::Child;
use tokio::time::sleep;
use tracing::{info, warn};

pub struct ManagedChild {
    pub(crate) child: Child,
    stderr_tail: Arc<std::sync::Mutex<Vec<String>>>,
    #[cfg(unix)]
    pub(crate) pgid: Option<i32>,
}

impl ManagedChild {
    pub fn new(child: Child, stderr_tail: Arc<std::sync::Mutex<Vec<String>>>) -> Self {
        #[cfg(unix)]
        let pgid = child.id().and_then(|id| match i32::try_from(id) {
            Ok(v) if v > 1 => Some(v),
            _ => None,
        });

        Self {
            child,
            stderr_tail,
            #[cfg(unix)]
            pgid,
        }
    }

    pub fn recent_stderr(&self) -> String {
        if let Ok(guard) = self.stderr_tail.lock() {
            if !guard.is_empty() {
                return format!(" (stderr: {})", guard.join(" | "));
            }
        }
        String::new()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub async fn graceful_shutdown(&mut self, grace_period: Duration) {
        if let Ok(Some(_)) = self.child.try_wait() {
            return;
        }

        #[cfg(unix)]
        {
            if let Some(pgid) = self.pgid {
                // CRITICAL DEFENSE: Never signal pgid <= 1 (prevents supervisor self-suicide).
                if pgid > 1 {
                    // SAFETY: `pgid` is a validated positive process group ID (> 1)
                    // created via `cmd.process_group(0)` for this child.
                    unsafe {
                        libc::killpg(pgid, libc::SIGTERM);
                    }
                }
            }
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
                        if pgid > 1 {
                            // SAFETY: Force-kill process group on timeout escalation.
                            unsafe {
                                libc::killpg(pgid, libc::SIGKILL);
                            }
                        }
                    }
                }
                if let Err(e) = self.child.kill().await {
                    tracing::debug!(
                        error = %e,
                        "Non-fatal error calling child.kill() after grace period expiry"
                    );
                }
            }
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            #[cfg(unix)]
            if let Some(pgid) = self.pgid {
                // CRITICAL DEFENSE: Never signal pgid <= 1 (prevents supervisor self-suicide).
                if pgid > 1 {
                    // SAFETY: Emergency fallback SIGKILL to eliminate process group leaks on drop.
                    unsafe {
                        libc::killpg(pgid, libc::SIGKILL);
                    }
                }
            }
            if let Err(e) = self.child.start_kill() {
                tracing::debug!(
                    error = %e,
                    "Non-fatal error calling child.start_kill() in ManagedChild::drop"
                );
            }
        }
    }
}
