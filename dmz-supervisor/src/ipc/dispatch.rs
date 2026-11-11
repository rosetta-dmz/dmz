use super::types::{DispatchContext, IpcRequest, IpcResponse};
use crate::AgentConfig;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tracing::info;

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
                let payload = serde_json::to_value(statuses).unwrap_or_default();
                IpcResponse::ok_with_data("Collateral service status", payload)
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
