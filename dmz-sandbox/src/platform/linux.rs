use crate::platform::{PlatformError, SandboxConfig};
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&config.workspace_path);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let status = cmd
        .status()
        .map_err(|err| PlatformError::NamespaceError(format!("Failed to execute sandboxed process: {}", err)))?;

    Ok(status)
}