use crate::platform::{PlatformError, SandboxConfig};
use std::process::{Command, ExitStatus};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(&config.envs);
    cmd.current_dir(&config.workspace_path);

    let status = cmd
        .status()
        .map_err(|err| PlatformError::NamespaceError(format!("Failed to execute sandboxed process: {}", err)))?;

    Ok(status)
}