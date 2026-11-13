use super::types::{DispatchContext, IpcRequest, IpcResponse};
use crate::{AgentConfig, SidecarPairConfig};
use dmz_core::workspace::WorkspaceProjectConfig;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tracing::{info, warn};

/// Dispatches an IPC request to the appropriate supervisor subsystem.
pub async fn dispatch_request(req: IpcRequest, ctx: &DispatchContext) -> IpcResponse {
    match req {
        IpcRequest::Ping => IpcResponse::ok("pong"),

        IpcRequest::GetStatus => {
            let active_agents = ctx.agent_runner.list_active_agents().await;
            let sidecar_state = ctx.orchestrator.current_state().await;
            let service_statuses = if let Some(ref sr) = ctx.service_runner {
                Some(sr.get_status_all().await)
            } else {
                None
            };
            let status_data = serde_json::json!({
                "status": "active",
                "uptime_seconds": ctx.started_at.elapsed().as_secs(),
                "active_agents": active_agents.len(),
                "active_agent_ids": active_agents,
                "sidecar_state": format!("{:?}", sidecar_state),
                "service_statuses": service_statuses,
            });
            IpcResponse::ok_with_data("Supervisor active", status_data)
        }

        IpcRequest::StartAgent {
            agent_id,
            allowed_domains,
            scratchpad_bytes,
            command,
            args,
        } => {
            info!(agent = %agent_id, domains = ?allowed_domains, "Spawning agent process via IPC");
            let (binary_or_script, cmd_args) = match (command, args) {
                (Some(cmd), Some(a)) => (cmd, a),
                (Some(cmd), None) => (cmd, Vec::new()),
                _ => {
                    if cfg!(windows) {
                        (
                            "ping.exe".to_string(),
                            vec![
                                "-n".to_string(),
                                "3600".to_string(),
                                "127.0.0.1".to_string(),
                            ],
                        )
                    } else {
                        ("sleep".to_string(), vec!["3600".to_string()])
                    }
                }
            };

            let config = AgentConfig {
                agent_id: agent_id.clone(),
                binary_or_script,
                args: cmd_args,
                envs: HashMap::new(),
                allowed_domains,
                scratchpad_bytes: scratchpad_bytes.max(1024 * 1024),
            };

            match ctx.agent_runner.spawn_agent(config).await {
                Ok(()) => IpcResponse::ok(format!("Agent '{}' spawned successfully", agent_id)),
                Err(e) => IpcResponse::error(e),
            }
        }

        IpcRequest::StopAgent { agent_id } => {
            info!(agent = %agent_id, "Stopping agent process via IPC");
            match ctx.agent_runner.terminate_agent(&agent_id).await {
                Ok(()) => IpcResponse::ok(format!("Agent '{}' terminated", agent_id)),
                Err(e) => IpcResponse::error(e),
            }
        }

        IpcRequest::RunSandbox {
            workspace_path,
            command,
            args,
            allow_network,
        } => execute_sandboxed_request(workspace_path, command, args, allow_network).await,

        IpcRequest::GetSidecarStatus => {
            let state = ctx.orchestrator.current_state().await;
            let socket = ctx
                .orchestrator
                .current_socket_path()
                .await
                .map(|p| p.to_string_lossy().to_string());
            let last_error = ctx.orchestrator.last_error().await;
            let status_data = serde_json::json!({
                "state": format!("{:?}", state),
                "socket": socket,
                "last_error": last_error,
            });
            IpcResponse::ok_with_data("Sidecar status query", status_data)
        }

        IpcRequest::StartSidecarPair {
            workspace_path,
            config_path,
            allow_network,
        } => {
            info!(
                workspace = %workspace_path,
                config_path = ?config_path,
                "StartSidecarPair requested via IPC"
            );
            let raw_ws = PathBuf::from(&workspace_path);
            let ws_canon = std::fs::canonicalize(&raw_ws).unwrap_or(raw_ws);
            let manifest_res = if let Some(ref explicit_cfg) = config_path {
                let cfg_path = Path::new(explicit_cfg);
                WorkspaceProjectConfig::load_from_file(cfg_path).map(|cfg| {
                    let root = cfg_path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| ws_canon.clone());
                    Some((cfg, root))
                })
            } else {
                WorkspaceProjectConfig::discover_workspace_manifest_checked(&ws_canon).map(|opt| {
                    opt.map(|(cfg, manifest_file)| {
                        let root = manifest_file
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|| ws_canon.clone());
                        let root = std::fs::canonicalize(&root).unwrap_or(root);
                        (cfg, root)
                    })
                })
            };

            match manifest_res {
                Ok(Some((project, root))) => {
                    if project.sidecars().is_empty() {
                        IpcResponse::error(format!(
                            "Workspace '{}' has no sidecar or service member defined",
                            project.name
                        ))
                    } else {
                        let pair_config = SidecarPairConfig {
                            workspace_root: root,
                            project,
                            allow_network,
                        };
                        match ctx.orchestrator.start_pair(pair_config).await {
                            Ok(sock) => IpcResponse::ok_with_data(
                                "Sidecar pair started",
                                serde_json::json!({ "socket": sock.to_string_lossy() }),
                            ),
                            Err(e) => IpcResponse::error(e.to_string()),
                        }
                    }
                }
                Ok(None) => IpcResponse::error(format!(
                    "No dmz-workspace.toml found in workspace '{}'",
                    ws_canon.display()
                )),
                Err(e) => IpcResponse::error(format!("Invalid dmz-workspace.toml: {}", e)),
            }
        }

        IpcRequest::StopSidecarPair => {
            info!("StopSidecarPair requested via IPC");
            match ctx.orchestrator.stop_pair().await {
                Ok(()) => IpcResponse::ok("Sidecar pair stopped"),
                Err(e) => IpcResponse::error(e),
            }
        }

        IpcRequest::GetServiceStatus => {
            if let Some(ref sr) = ctx.service_runner {
                let statuses = sr.get_status_all().await;
                match serde_json::to_value(statuses) {
                    Ok(payload) => IpcResponse::ok_with_data("Collateral service status", payload),
                    Err(e) => {
                        warn!(error = %e, "Failed to serialize collateral service statuses");
                        IpcResponse::error(format!("Failed to serialize service statuses: {}", e))
                    }
                }
            } else {
                IpcResponse::error("ServiceRunner not configured on this IPC server")
            }
        }

        IpcRequest::Shutdown => {
            info!("Shutdown request received via IPC");
            ctx.is_running.store(false, Ordering::SeqCst);
            ctx.shutdown_notify.notify_waiters();
            IpcResponse::ok("Supervisor shutting down")
        }
    }
}

/// Dispatches a sandboxed execution task to blocking threadpool with sanitized environment variables.
pub async fn execute_sandboxed_request(
    workspace_path: String,
    command: String,
    args: Vec<String>,
    allow_network: bool,
) -> IpcResponse {
    info!(cmd = %command, workspace = %workspace_path, "Executing sandboxed task via IPC");
    let ws_path = PathBuf::from(&workspace_path);
    let store_path = ws_path.join(".dmz").join("store");

    // Whitelist benign environment keys and strip potential authentication secrets
    let safe_keys = [
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TERM", "TMPDIR", "TEMP",
        "TMP",
    ];
    let sanitized_envs: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            let upper = k.to_uppercase();
            if upper.contains("KEY")
                || upper.contains("SECRET")
                || upper.contains("TOKEN")
                || upper.contains("PASSWORD")
                || upper.contains("AUTH")
                || upper.starts_with("AWS_")
                || upper.starts_with("GITHUB_")
                || upper.starts_with("SSH_")
            {
                return false;
            }
            safe_keys.contains(&k.as_str())
        })
        .collect();

    let config = SandboxConfig {
        workspace_path: ws_path,
        closure_path: store_path,
        command,
        args,
        envs: sanitized_envs,
        allow_network,
        ipc_socket_path: None,
    };

    let exec_res = tokio::task::spawn_blocking(move || run_sandboxed(&config)).await;

    match exec_res {
        Ok(Ok(status)) => {
            let code = status.code().unwrap_or(-1);
            IpcResponse::ok_with_data(
                format!("Sandbox command completed (exit code {})", code),
                serde_json::json!({
                    "success": status.success(),
                    "exit_code": code,
                }),
            )
        }
        Ok(Err(e)) => IpcResponse::error(format!("Sandbox execution failed: {}", e)),
        Err(e) => IpcResponse::error(format!("Sandbox task join error: {}", e)),
    }
}
