use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::info;

use super::child::ManagedChild;
use super::types::{SidecarOrchestratorError, SidecarPairConfig};

/// Automatically infers execution command and arguments for a workspace member based on project manifests
pub fn infer_member_execution_in(
    workspace_root: &Path,
    member_dir: &Path,
    member: &WorkspaceMember,
) -> Result<(String, Vec<String>), SidecarOrchestratorError> {
    if let Ok(resolved) = member.resolve_command(member_dir, workspace_root) {
        return Ok(resolved);
    }

    Err(SidecarOrchestratorError::ExecutionInferenceFailed {
        path: member_dir.display().to_string(),
    })
}

/// Automatically infers execution command and arguments for a workspace member based on project manifests.
///
/// Uses `member_dir.parent()` as a fallback workspace root when the workspace root is not passed explicitly;
/// prefer [`infer_member_execution_in`] when `workspace_root` is known.
pub fn infer_member_execution(
    member_dir: &Path,
    member: &WorkspaceMember,
) -> Result<(String, Vec<String>), SidecarOrchestratorError> {
    let workspace_root = member_dir.parent().unwrap_or(member_dir);
    infer_member_execution_in(workspace_root, member_dir, member)
}

pub async fn spawn_member_process(
    workspace_root: &Path,
    member: &WorkspaceMember,
    sock_path: &Path,
    config: &SidecarPairConfig,
) -> Result<ManagedChild, SidecarOrchestratorError> {
    let member_dir = config
        .project
        .resolve_member_layouts(workspace_root, None)
        .into_iter()
        .find(|l| l.member.id == member.id)
        .map(|l| l.host_path)
        .unwrap_or_else(|| {
            if member.path.is_absolute() {
                member.path.clone()
            } else {
                workspace_root.join(&member.path)
            }
        });
    let member_dir = std::fs::canonicalize(&member_dir).unwrap_or(member_dir);
    if !member_dir.exists() {
        return Err(SidecarOrchestratorError::MemberDirectoryNotFound(
            member_dir,
        ));
    }

    // Inject unified hermetic environment (toolchains, .venv, node_modules/.bin, IPC socket, offline flags)
    let envs = config.project.build_member_env(
        workspace_root,
        &member_dir,
        sock_path,
        config.allow_network,
    );
    let path_env = envs.get("PATH").cloned().unwrap_or_default();

    // Execute pre-launch `build_command` if declared on the member (e.g. freezing a sidecar binary)
    if let Some(ref build_cmd_str) = member.build_command {
        let trimmed_build = build_cmd_str.trim();
        if !trimmed_build.is_empty() {
            info!(
                member_id = %member.id,
                build_command = %trimmed_build,
                "Executing member pre-launch build_command"
            );
            let mut build_proc = if cfg!(windows) {
                let mut c = Command::new("cmd.exe");
                c.args(["/C", trimmed_build]);
                c
            } else {
                let mut c = Command::new("/bin/sh");
                c.args(["-c", trimmed_build]);
                c
            };
            build_proc
                .current_dir(&member_dir)
                .envs(&envs)
                .env_remove("PYTHONHOME");
            let build_out = build_proc.output().await.map_err(|e| {
                SidecarOrchestratorError::Generic(format!(
                    "Failed to execute build_command '{}' for '{}': {}",
                    trimmed_build, member.id, e
                ))
            })?;
            if !build_out.status.success() {
                let stderr = String::from_utf8_lossy(&build_out.stderr);
                return Err(SidecarOrchestratorError::BuildCommandFailed {
                    command: trimmed_build.to_string(),
                    member: member.id.clone(),
                    status: build_out.status,
                    stderr: stderr.trim().to_string(),
                });
            }
        }
    }

    let (raw_exec_cmd, default_args) = member
        .resolve_command(&member_dir, workspace_root)
        .or_else(|_| infer_member_execution(&member_dir, member))?;

    let effective_cwd = if (raw_exec_cmd == "cargo" || raw_exec_cmd.ends_with("/cargo"))
        && !member_dir.join("Cargo.toml").is_file()
        && member_dir.join("src-tauri/Cargo.toml").is_file()
    {
        member_dir.join("src-tauri")
    } else {
        member_dir.clone()
    };

    let resolved_exec_cmd =
        WorkspaceProjectConfig::resolve_executable_in_env(&raw_exec_cmd, &path_env, &effective_cwd);

    let mut cmd = Command::new(&resolved_exec_cmd);
    if !default_args.is_empty() {
        cmd.args(&default_args);
    }

    // Put process in its own process group on Unix so children don't become zombies
    #[cfg(unix)]
    cmd.process_group(0);

    cmd.current_dir(&effective_cwd);
    cmd.envs(envs);
    cmd.env_remove("PYTHONHOME");

    // Pipe stdout and stderr for multiplexed logging
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| SidecarOrchestratorError::SpawnFailed {
            member: member.id.clone(),
            source: e,
        })?;

    let member_id = member.id.clone();
    let color_code = match member.role {
        MemberRole::Primary => "\x1b[36m",                       // Cyan
        MemberRole::Sidecar | MemberRole::Service => "\x1b[35m", // Magenta
        MemberRole::Member => "\x1b[33m",                        // Yellow
    };

    if let Some(stdout) = child.stdout.take() {
        let id = member_id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                println!("{}[{}]\x1b[0m\t{}", color_code, id, line);
            }
        });
    }

    let stderr_tail = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    if let Some(stderr) = child.stderr.take() {
        let id = member_id.clone();
        let tail_ref = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("\x1b[31m[{}]\x1b[0m\t{}", id, line);
                if let Ok(mut guard) = tail_ref.lock() {
                    if guard.len() >= 8 {
                        guard.remove(0);
                    }
                    guard.push(line);
                }
            }
        });
    }

    Ok(ManagedChild::new(child, stderr_tail))
}
