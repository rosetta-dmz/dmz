use crate::platform::{PlatformError, SandboxConfig};
use nix::sched::{unshare, CloneFlags};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    // Validate IPC socket path length and enforce restrictive permissions (0o700)
    if let Some(ref sock_path) = config.ipc_socket_path {
        let path_len = sock_path.as_os_str().as_encoded_bytes().len();
        // Linux POSIX sockaddr_un.sun_path is strictly 108 bytes (including null terminator)
        if path_len >= 108 {
            return Err(PlatformError::NamespaceError(format!(
                "IPC socket path exceeds Linux sun_path limit ({} >= 108 bytes): {:?}",
                path_len, sock_path
            )));
        }

        if let Some(parent) = sock_path.parent() {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            builder.mode(0o700);
            builder.create(parent).map_err(|e| {
                PlatformError::PlatformFault(format!(
                    "Failed to create secure socket directory {:?}: {}",
                    parent, e
                ))
            })?;
        }
    }

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&config.workspace_path);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    // Assign dedicated process group to prevent orphaned zombie processes upon termination
    cmd.process_group(0);

    let allow_network = config.allow_network;

    // Closure passed to `pre_exec` runs in the child process between fork and exec.
    // Invoke only async-signal-safe POSIX syscalls via libc/nix without heap allocations.
    // File descriptor and socket resources created within this closure are closed before exec.
    unsafe {
        cmd.pre_exec(move || {
            // A. Prevent privilege escalation via setuid binaries inside the sandbox
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }

            // B. Unshare user and mount namespaces for containment
            let flags = if allow_network {
                CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS
            } else {
                CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWNET
            };

            // Enforce loud failure
            unshare(flags).map_err(|err| std::io::Error::from_raw_os_error(err as i32))?;

            // C. Prevent mount event leakage to host mount namespace
            if libc::mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }

            // D. When network is isolated, bring up loopback 'lo' using async-signal-safe ioctl
            if !allow_network {
                let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
                if sock < 0 {
                    return Err(std::io::Error::last_os_error());
                }

                let mut ifr: libc::ifreq = std::mem::zeroed();
                ifr.ifr_name[0] = b'l' as libc::c_char;
                ifr.ifr_name[1] = b'o' as libc::c_char;
                ifr.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;

                let ioctl_res = libc::ioctl(sock, libc::SIOCSIFFLAGS, &ifr);
                let _ = libc::close(sock);

                if ioctl_res != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }

            Ok(())
        });
    }

    let status = cmd.status().map_err(|err| {
        PlatformError::NamespaceError(format!(
            "Failed to execute sandboxed process in Linux namespace: {}. Ensure unprivileged user namespaces are enabled or run with --no-sandbox.",
            err
        ))
    })?;

    Ok(status)
}
