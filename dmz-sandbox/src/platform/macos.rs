use crate::platform::{PlatformError, SandboxConfig};
use std::fs;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
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

    let status = cmd.status().map_err(|err| {
        PlatformError::SeatbeltError(format!("Failed to execute sandbox-exec: {}", err))
    })?;

    Ok(status)
}

pub fn generate_seatbelt_profile(config: &SandboxConfig) -> String {
    let canonical_workspace =
        fs::canonicalize(&config.workspace_path).unwrap_or_else(|_| config.workspace_path.clone());
    let workspace = canonical_workspace.to_string_lossy();

    let canonical_closure =
        fs::canonicalize(&config.closure_path).unwrap_or_else(|_| config.closure_path.clone());
    let closure = canonical_closure.to_string_lossy();

    let network_rule = if config.allow_network {
        "(allow network*)".to_string()
    } else {
        "(deny network*)\n(allow network* (to unix-socket))\n(allow network-bind (to unix-socket))\n(allow network-outbound (to unix-socket))".to_string()
    };

    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());

    let mut user_toolchain_rules = String::new();
    let toolchain_dirs = [
        ".cargo", ".rustup", ".local", ".nvm", ".pnpm", ".pyenv", ".asdf", ".fnm", ".volta", "go",
        ".cache",
    ];

    for dir in toolchain_dirs {
        let p = format!("{}/{}", home, dir);
        if std::path::Path::new(&p).exists() {
            user_toolchain_rules.push_str(&format!("    (subpath \"{}\")\n", p));
        }
    }

    let mut ipc_socket_rules = String::new();
    if let Some(ref sock_path) = config.ipc_socket_path {
        let raw_str = sock_path.to_string_lossy();
        ipc_socket_rules.push_str(&format!(
            "(allow file-read* file-write* (literal \"{}\"))\n",
            raw_str
        ));

        let sock_canon = fs::canonicalize(sock_path).unwrap_or_else(|_| sock_path.clone());
        let sock_str = sock_canon.to_string_lossy();
        if sock_str != raw_str {
            ipc_socket_rules.push_str(&format!(
                "(allow file-read* file-write* (literal \"{}\"))\n",
                sock_str
            ));
        }

        if let Some(parent) = sock_canon.parent() {
            ipc_socket_rules.push_str(&format!(
                "(allow file-read* file-write* (subpath \"{}\"))\n",
                parent.display()
            ));
        }
        if let Some(parent) = sock_path.parent() {
            let p_str = parent.display().to_string();
            if !p_str.is_empty() && p_str != "." && p_str != "/tmp" && p_str != "/private/tmp" {
                ipc_socket_rules.push_str(&format!(
                    "(allow file-read* file-write* (subpath \"{}\"))\n",
                    p_str
                ));
            }
        }
    }

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
    (subpath "/private/tmp"))

(allow file-read* file-write* (subpath "{}"))
(allow file-read* (subpath "{}"))
{}
"#,
        network_rule, user_toolchain_rules, workspace, closure, ipc_socket_rules
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

        // Strict network denial is enforced
        assert!(profile.contains("(deny network*)"));

        // Unix domain sockets are explicitly permitted under network denial
        assert!(profile.contains("(allow network* (to unix-socket))"));
        assert!(profile.contains("(allow network-bind (to unix-socket))"));
        assert!(profile.contains("(allow network-outbound (to unix-socket))"));

        // Safe socket literal path is mounted
        assert!(profile.contains(&format!("(literal \"{}\")", sock_path.display())));
    }
}
