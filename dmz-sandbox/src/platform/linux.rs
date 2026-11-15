use crate::platform::{PlatformError, SandboxConfig, SandboxIsolationMode, SandboxPolicyOptions};
use nix::sched::{unshare, CloneFlags};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    run_sandboxed_with_options(config, &SandboxPolicyOptions::default())
}

pub fn run_sandboxed_with_options(
    config: &SandboxConfig,
    options: &SandboxPolicyOptions,
) -> Result<ExitStatus, PlatformError> {
    let (mut cmd, _posture) = build_command(config, options)?;
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let status = cmd.status().map_err(|err| {
        PlatformError::io(
            "Failed to execute sandboxed process in Linux namespace (ensure unprivileged user namespaces are enabled or use host mode)",
            err,
        )
    })?;

    Ok(status)
}

/// Builds a Linux-namespace-isolated `Command` and returns its active isolation posture label.
pub fn build_command(
    config: &SandboxConfig,
    options: &SandboxPolicyOptions,
) -> Result<(Command, &'static str), PlatformError> {
    if !config.allow_network
        && options.proxy_addr.is_some()
        && options.isolation_mode == SandboxIsolationMode::StrictKernel
    {
        return Err(PlatformError::UnsupportedPlatform(
            "StrictKernel isolation on Linux cannot combine CLONE_NEWNET with a host loopback proxy without a veth/slirp bridge; use BestEffort or empty allowed_domains".into(),
        ));
    }

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

    // Assign dedicated process group to prevent orphaned zombie processes upon termination
    cmd.process_group(0);

    let isolate_net_ns = !config.allow_network && options.proxy_addr.is_none();
    let strict_kernel = options.isolation_mode == SandboxIsolationMode::StrictKernel;

    let posture = if config.allow_network {
        "kernel-userns-network-allowed"
    } else if isolate_net_ns {
        "kernel-netns"
    } else {
        "linux-userns-proxy"
    };

    // SAFETY: Closure passed to `pre_exec` runs in the child process between fork and exec.
    // Invoke only async-signal-safe POSIX syscalls via libc/nix without heap allocations.
    // File descriptor and socket resources created within this closure are closed before exec.
    unsafe {
        cmd.pre_exec(move || {
            // A. Prevent privilege escalation via setuid binaries inside the sandbox
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }

            // B. Unshare user and mount namespaces for containment
            let flags_all = if isolate_net_ns {
                CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWNET
            } else {
                CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS
            };

            if unshare(flags_all).is_ok() {
                // Prevent mount event leakage to host mount namespace
                let _ = libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                );

                // When network is isolated, bring up loopback 'lo' using async-signal-safe ioctl
                if isolate_net_ns {
                    let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
                    if sock >= 0 {
                        let mut ifr: libc::ifreq = std::mem::zeroed();
                        ifr.ifr_name[0] = b'l' as libc::c_char;
                        ifr.ifr_name[1] = b'o' as libc::c_char;
                        ifr.ifr_ifru.ifru_flags =
                            (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
                        let _ = libc::ioctl(sock, libc::SIOCSIFFLAGS as _, &ifr);
                        let _ = libc::close(sock);
                    }
                }
            } else if strict_kernel && isolate_net_ns {
                return Err(std::io::Error::last_os_error());
            } else if isolate_net_ns {
                // Fall back to unprivileged user + network namespace if mount namespace is restricted
                if unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET).is_err() {
                    // Fall back to network-only namespace if process already has CAP_NET_ADMIN / root
                    let _ = unshare(CloneFlags::CLONE_NEWNET);
                }
            } else {
                let _ = unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS);
            }

            Ok(())
        });
    }

    Ok((cmd, posture))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_linux_strict_kernel_rejects_proxy_with_netns_and_best_effort_reports_posture() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = SandboxConfig::new(dir.path(), PathBuf::from("/nix/store"), "/bin/true");
        config.allow_network = false;

        let proxy_opts = SandboxPolicyOptions {
            isolation_mode: SandboxIsolationMode::StrictKernel,
            proxy_addr: Some("127.0.0.1:41234".parse().unwrap()),
            writable_paths: Vec::new(),
            ..SandboxPolicyOptions::default()
        };
        let strict_res = build_command(&config, &proxy_opts);
        assert!(matches!(
            strict_res,
            Err(PlatformError::UnsupportedPlatform(_))
        ));

        let best_effort_opts = SandboxPolicyOptions {
            isolation_mode: SandboxIsolationMode::BestEffort,
            proxy_addr: Some("127.0.0.1:41234".parse().unwrap()),
            writable_paths: Vec::new(),
            ..SandboxPolicyOptions::default()
        };
        let (_, proxy_posture) = build_command(&config, &best_effort_opts).unwrap();
        assert_eq!(proxy_posture, "linux-userns-proxy");

        let netns_opts = SandboxPolicyOptions {
            isolation_mode: SandboxIsolationMode::StrictKernel,
            proxy_addr: None,
            writable_paths: Vec::new(),
            ..SandboxPolicyOptions::default()
        };
        let (_, netns_posture) = build_command(&config, &netns_opts).unwrap();
        assert_eq!(netns_posture, "kernel-netns");
    }
}
