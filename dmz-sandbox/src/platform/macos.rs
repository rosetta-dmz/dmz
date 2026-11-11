use crate::platform::{PlatformError, SandboxConfig};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    // Validate IPC socket path length on macOS (POSIX sockaddr_un.sun_path strictly 104 bytes)
    if let Some(ref sock_path) = config.ipc_socket_path {
        let path_len = sock_path.as_os_str().as_encoded_bytes().len();
        if path_len >= 104 {
            return Err(PlatformError::SeatbeltError(format!(
                "IPC socket path exceeds macOS sun_path limit ({} >= 104 bytes): {:?}",
                path_len, sock_path
            )));
        }
    }

    // Validate workspace path exists
    if !config.workspace_path.exists() {
        return Err(PlatformError::SeatbeltError(format!(
            "Workspace path does not exist: {:?}",
            config.workspace_path
        )));
    }

    // Ensure private .dmz/tmp directory exists with restrictive 0o700 permissions
    let private_tmp = config.workspace_path.join(".dmz").join("tmp");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(&private_tmp).map_err(|e| {
        PlatformError::PlatformFault(format!(
            "Failed to create secure sandbox temporary directory {:?}: {}",
            private_tmp, e
        ))
    })?;

    let profile_str = generate_seatbelt_profile(config);

    let mut cmd = Command::new("sandbox-exec");
    cmd.arg("-p").arg(profile_str);
    cmd.arg(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&config.workspace_path);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    // Assign dedicated process group to prevent orphaned zombie processes upon termination
    cmd.process_group(0);

    let status = cmd.status().map_err(|err| {
        PlatformError::SeatbeltError(format!("Failed to execute sandbox-exec: {}", err))
    })?;

    Ok(status)
}

/// Escapes backslashes, double quotes, and control/null characters in file paths for safe interpolation into Scheme Seatbelt profiles
pub fn escape_seatbelt_path(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    raw.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r', '\0'], "")
}

pub fn generate_seatbelt_profile(config: &SandboxConfig) -> String {
    let canonical_workspace =
        fs::canonicalize(&config.workspace_path).unwrap_or_else(|_| config.workspace_path.clone());
    let workspace = escape_seatbelt_path(&canonical_workspace);

    // Private workspace temporary directory (.dmz/tmp)
    let private_tmp = canonical_workspace.join(".dmz").join("tmp");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    let _ = builder.create(&private_tmp);
    let private_tmp_escaped = escape_seatbelt_path(&private_tmp);

    let network_rule = if config.allow_network {
        "(allow network*)".to_string()
    } else {
        "(deny network*)\n(allow network* (to unix-socket))\n(allow network-bind (to unix-socket))\n(allow network-outbound (to unix-socket))".to_string()
    };

    let home = std::env::var("HOME").ok().filter(|h| !h.trim().is_empty());

    let has_bundled_toolchains = config.workspace_path.join(".dmz/toolchains").is_dir()
        || config.closure_path.join("toolchains").is_dir();

    let mut user_toolchain_rules = String::new();
    // Whitelist specific toolchain binary directories rather than blanket ~/.cargo or ~/.cache (which contain credentials)
    let toolchain_dirs = [
        ".cargo/bin",
        ".rustup/toolchains",
        ".local/bin",
        ".nvm",
        ".pnpm",
        ".pyenv",
        ".asdf",
        ".fnm",
        ".volta",
        "go",
    ];

    // Only allow ambient user toolchains if bundled toolchains are not present in .dmz/toolchains
    if !has_bundled_toolchains {
        if let Some(ref h) = home {
            for dir in toolchain_dirs {
                let p = format!("{}/{}", h, dir);
                let path_obj = std::path::Path::new(&p);
                if path_obj.exists() {
                    user_toolchain_rules.push_str(&format!(
                        "    (subpath \"{}\")\n",
                        escape_seatbelt_path(path_obj)
                    ));
                }
            }
        }
    }

    let mut ipc_socket_rules = String::new();
    if let Some(ref sock_path) = config.ipc_socket_path {
        let raw_str = escape_seatbelt_path(sock_path);
        ipc_socket_rules.push_str(&format!(
            "(allow file-read* file-write* (literal \"{}\"))\n",
            raw_str
        ));

        let sock_canon = fs::canonicalize(sock_path).unwrap_or_else(|_| sock_path.clone());
        let sock_str = escape_seatbelt_path(&sock_canon);
        if sock_str != raw_str {
            ipc_socket_rules.push_str(&format!(
                "(allow file-read* file-write* (literal \"{}\"))\n",
                sock_str
            ));
        }

        // Only grant subpath write access if the socket is in a dedicated private folder.
        // Always exclude blanket subpath write access to shared system directories
        // (/tmp, /private/tmp, /var, /private/var, /).
        for parent_candidate in [&sock_canon.parent(), &sock_path.parent()]
            .into_iter()
            .flatten()
        {
            let p_str = parent_candidate.display().to_string();
            if !p_str.is_empty()
                && p_str != "."
                && p_str != "/tmp"
                && p_str != "/private/tmp"
                && p_str != "/var"
                && p_str != "/private/var"
                && p_str != "/"
            {
                let escaped = escape_seatbelt_path(parent_candidate);
                let rule = format!("(allow file-read* file-write* (subpath \"{}\"))\n", escaped);
                if !ipc_socket_rules.contains(&rule) {
                    ipc_socket_rules.push_str(&rule);
                }
            }
        }
    }

    // Guard against empty closure paths granting root filesystem read access
    let closure_rule =
        if !config.closure_path.as_os_str().is_empty() && config.closure_path.exists() {
            let canonical_closure = fs::canonicalize(&config.closure_path)
                .unwrap_or_else(|_| config.closure_path.clone());
            format!(
                "(allow file-read* (subpath \"{}\"))\n",
                escape_seatbelt_path(&canonical_closure)
            )
        } else {
            String::new()
        };

    format!(
        r#"
(version 1)
(deny default)

(allow process*)
(allow sysctl-read)
(allow signal (target self))
(allow mach-lookup)
(allow ipc-posix*)
{}

(allow file-read*
    (literal "/")
    (subpath "/bin")
    (subpath "/sbin")
    (subpath "/usr")
    (subpath "/System")
    (subpath "/Library")
    (subpath "/Applications")
    (subpath "/opt/homebrew")
    (subpath "/usr/local")
    (subpath "/private/var/db/dyld")
    (subpath "/private/etc")
    (subpath "/private/tmp")
    (subpath "/dev")
{})

(allow file-write*
    (subpath "/dev/null")
    (subpath "/dev/zero")
    (subpath "/dev/tty")
    (subpath "{}"))

(allow file-read* file-write* (subpath "{}"))
{}{}
"#,
        network_rule,
        user_toolchain_rules,
        private_tmp_escaped,
        workspace,
        closure_rule,
        ipc_socket_rules
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_seatbelt_profile_unix_socket_and_safe_path() {
        let sock_path = PathBuf::from("/tmp/dmz_abc123_sidecar.sock");
        let config = SandboxConfig {
            workspace_path: PathBuf::from("/Users/test/workspace"),
            closure_path: PathBuf::from("/nix/store"),
            command: "/bin/sh".to_string(),
            args: vec![],
            envs: vec![],
            allow_network: false,
            ipc_socket_path: Some(sock_path.clone()),
        };

        let profile = generate_seatbelt_profile(&config);

        // Enforce strict network denial
        assert!(profile.contains("(deny network*)"));

        // Explicitly permit Unix domain sockets
        assert!(profile.contains("(allow network* (to unix-socket))"));
        assert!(profile.contains("(allow network-bind (to unix-socket))"));
        assert!(profile.contains("(allow network-outbound (to unix-socket))"));

        // Safe socket literal path is mounted
        assert!(profile.contains(&format!("(literal \"{}\")", sock_path.display())));

        // Verify /private/tmp is NOT granted subpath write access
        assert!(!profile.contains(r#"(allow file-read* file-write* (subpath "/private/tmp"))"#));
        assert!(!profile.contains(r#"(allow file-write* (subpath "/private/tmp"))"#));
    }

    #[test]
    fn test_run_sandboxed_socket_length_limit() {
        let long_name = "a".repeat(110);
        let long_sock = PathBuf::from(format!("/tmp/{}.sock", long_name));
        let config = SandboxConfig {
            workspace_path: PathBuf::from("/tmp"),
            closure_path: PathBuf::from("/nix/store"),
            command: "/bin/true".to_string(),
            args: vec![],
            envs: vec![],
            allow_network: false,
            ipc_socket_path: Some(long_sock),
        };

        let res = run_sandboxed(&config);
        assert!(res.is_err());
        match res.unwrap_err() {
            PlatformError::SeatbeltError(msg) => {
                assert!(msg.contains("sun_path limit"));
            }
            other => panic!(
                "Expected SeatbeltError with sun_path limit, got: {:?}",
                other
            ),
        }
    }
}
