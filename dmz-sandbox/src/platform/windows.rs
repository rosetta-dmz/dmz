use crate::platform::{PlatformError, SandboxConfig};
use std::fs;
use std::process::{Command, ExitStatus, Stdio};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    let canonical_workspace = fs::canonicalize(&config.workspace_path)
        .unwrap_or_else(|_| config.workspace_path.clone());

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&canonical_workspace);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let status = cmd
        .status()
        .map_err(|err| PlatformError::WindowsError(format!("Failed to execute Windows process: {}", err)))?;

    Ok(status)
}