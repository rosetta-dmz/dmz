use crate::platform::{PlatformError, SandboxConfig};
use std::fs;
use std::process::{Command, ExitStatus};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    let profile_str = generate_seatbelt_profile(config);

    let mut cmd = Command::new("sandbox-exec");
    cmd.arg("-p").arg(profile_str);
    cmd.arg(&config.command);
    cmd.args(&config.args);
    cmd.envs(&config.envs);

    let status = cmd
        .status()
        .map_err(|err| PlatformError::SeatbeltError(format!("Failed to execute sandbox-exec: {}", err)))?;

    Ok(status)
}

fn generate_seatbelt_profile(config: &SandboxConfig) -> String {
    let canonical_workspace = fs::canonicalize(&config.workspace_path)
        .unwrap_or_else(|_| config.workspace_path.clone());
    let workspace = canonical_workspace.to_string_lossy();

    let canonical_closure = fs::canonicalize(&config.closure_path)
        .unwrap_or_else(|_| config.closure_path.clone());
    let closure = canonical_closure.to_string_lossy();

    format!(
        r#"
(version 1)
(deny default)

(allow process*)
(allow sysctl-read)
(allow signal (target self))
(allow network*)

(allow file-read*
    (subpath "/bin")
    (subpath "/sbin")
    (subpath "/usr")
    (subpath "/System")
    (subpath "/Library")
    (subpath "/private/var/db/dyld")
    (subpath "/private/etc")
    (subpath "/private/tmp")
    (subpath "/dev"))

(allow file-write*
    (subpath "/dev/null")
    (subpath "/dev/zero")
    (subpath "/dev/tty")
    (subpath "/private/tmp"))

(allow file-read* file-write* (subpath "{}"))
(allow file-read* (subpath "{}"))
"#,
        workspace, closure
    )
}