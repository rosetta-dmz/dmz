use crate::platform::{PlatformError, SandboxConfig};
use nix::sched::{unshare, CloneFlags};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    if let Some(ref sock_path) = config.ipc_socket_path {
        if let Some(parent) = sock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

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
                // Try unsharing user namespace + mount namespace + network namespace (Bazel/Nix style full isolation)
                let flags_all =
                    CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWNET;
                if unshare(flags_all).is_err() {
                    // Fall back to unprivileged user namespace + network namespace if mount namespace is restricted
                    if unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET).is_err() {
                        // Fall back to network-only if process already has CAP_NET_ADMIN / root
                        unshare(CloneFlags::CLONE_NEWNET).map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!("Failed to unshare Linux network/user namespace: {}", e),
                            )
                        })?;
                    }
                }
                Ok(())
            });
        }
    } else {
        unsafe {
            cmd.pre_exec(|| {
                // Network allowed: unshare mount namespace if possible for filesystem containment
                let _ = unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS);
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
