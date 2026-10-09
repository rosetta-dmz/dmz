use crate::platform::{PlatformError, SandboxConfig};
use std::fs;
use std::process::{Command, ExitStatus};

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    // Canonicalize workspace path for consistent Windows long-path / UNC handling
    let canonical_workspace = fs::canonicalize(&config.workspace_path)
        .unwrap_or_else(|_| config.workspace_path.clone());

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(&config.envs);
    cmd.current_dir(&canonical_workspace);

    let status = cmd
        .status()
        .map_err(|err| PlatformError::WindowsError(format!("Failed to execute Windows process: {}", err)))?;

    Ok(status)
}