use crate::platform::{PlatformError, SandboxConfig};
use nix::sched::{unshare, CloneFlags};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&config.workspace_path);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    if !config.allow_network {
        unsafe {
            cmd.pre_exec(|| {
                // First try unsharing network namespace directly (works if process has CAP_NET_ADMIN / root)
                if unshare(CloneFlags::CLONE_NEWNET).is_err() {
                    // Fall back to unprivileged user namespace + network namespace
                    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!("Failed to unshare Linux network/user namespace: {}", e),
                        )
                    })?;
                }
                Ok(())
            });
        }
    }

    let status = cmd.status().map_err(|err| {
        PlatformError::NamespaceError(format!(
            "Failed to execute sandboxed process in Linux namespace: {}. Ensure unprivileged user namespaces are enabled or run with --no-sandbox.",
            err
        ))
    })?;

    Ok(status)
}
