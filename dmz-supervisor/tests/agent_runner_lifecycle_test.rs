use dmz_supervisor::{
    AgentConfig, AgentRunner, IpcClient, IpcRequest, IpcResponse, IpcServer, IpcServerConfig,
    SidecarOrchestrator,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_agent_runner_retains_scratchpad_and_terminates_without_deadlock() {
    let runner = AgentRunner::new();
    let agent_id = "test-agent-lifecycle".to_string();

    // Spawn a 30-second blocking process to prove `terminate_agent` never deadlocks on `child.wait()`
    let cmd = if cfg!(windows) { "ping.exe" } else { "sleep" };
    let args = if cfg!(windows) {
        vec!["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()]
    } else {
        vec!["30".to_string()]
    };

    let config = AgentConfig {
        agent_id: agent_id.clone(),
        binary_or_script: cmd.to_string(),
        args,
        envs: HashMap::new(),
        allowed_domains: vec!["api.openai.com".to_string()],
        scratchpad_bytes: 1024 * 1024,
    };

    runner
        .spawn_agent(config)
        .await
        .expect("Failed to spawn agent");

    // Agent must be registered in active agents
    let active = runner.list_active_agents().await;
    assert_eq!(active, vec![agent_id.clone()]);

    let scratchpad_path = std::env::temp_dir().join(format!("dmz_agent_scratchpad_{}", agent_id));
    assert!(
        scratchpad_path.exists(),
        "Scratchpad must exist while agent is running"
    );

    // Explicitly terminate the 30s agent and assert it finishes in < 500ms
    let stop_start = Instant::now();
    runner
        .terminate_agent(&agent_id)
        .await
        .expect("Terminate must succeed");
    let stop_elapsed = stop_start.elapsed();
    assert!(
        stop_elapsed < Duration::from_millis(500),
        "terminate_agent took {:?} (expected < 500ms without lock contention)",
        stop_elapsed
    );

    // Agent must immediately be removed and scratchpad wiped
    let active_after = runner.list_active_agents().await;
    assert!(
        active_after.is_empty(),
        "Active agents list should be empty after termination"
    );

    assert!(
        !scratchpad_path.exists(),
        "Scratchpad must be deleted immediately on termination"
    );
}

#[tokio::test]
async fn test_ipc_server_live_dispatch_and_shutdown() {
    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    #[cfg(unix)]
    let socket_path = tmp.path().join("dmz_test_ipc.sock");
    #[cfg(windows)]
    let socket_path =
        std::path::PathBuf::from(format!(r"\\.\pipe\dmz_test_ipc_{}", std::process::id()));

    let runner = AgentRunner::new();
    let orchestrator = SidecarOrchestrator::new();
    let server = Arc::new(IpcServer::with_managers(
        IpcServerConfig {
            socket_path: socket_path.clone(),
        },
        runner.clone(),
        orchestrator,
    ));

    let server_clone = server.clone();
    let server_handle = tokio::spawn(async move { server_clone.start().await });

    // Wait briefly for listener bind
    sleep(Duration::from_millis(50)).await;

    let client = IpcClient::new(socket_path);

    // 1. Ping
    let ping_resp = client
        .send_request(&IpcRequest::Ping)
        .await
        .expect("Ping failed");
    assert!(matches!(ping_resp, IpcResponse::Ok { ref message, .. } if message == "pong"));

    // 2. StartAgent via IPC
    let start_resp = client
        .send_request(&IpcRequest::StartAgent {
            agent_id: "ipc-agent-01".to_string(),
            allowed_domains: vec!["crates.io".to_string()],
            scratchpad_bytes: 2 * 1024 * 1024,
        })
        .await
        .expect("StartAgent failed");
    assert!(
        matches!(start_resp, IpcResponse::Ok { ref message, .. } if message.contains("ipc-agent-01"))
    );

    // 3. GetStatus reflects active_agents == 1
    let status_resp = client
        .send_request(&IpcRequest::GetStatus)
        .await
        .expect("GetStatus failed");
    let active_count = match status_resp {
        IpcResponse::Ok {
            payload: Some(ref d),
            ..
        } => d.get("active_agents").and_then(|v| v.as_u64()).unwrap_or(0),
        _ => 0,
    };
    assert_eq!(active_count, 1);

    // 4. StopAgent via IPC
    let stop_resp = client
        .send_request(&IpcRequest::StopAgent {
            agent_id: "ipc-agent-01".to_string(),
        })
        .await
        .expect("StopAgent failed");
    assert!(
        matches!(stop_resp, IpcResponse::Ok { ref message, .. } if message.contains("terminated"))
    );
    assert!(runner.list_active_agents().await.is_empty());

    // 5. GetSidecarStatus via IPC
    let sidecar_resp = client
        .send_request(&IpcRequest::GetSidecarStatus)
        .await
        .expect("GetSidecarStatus failed");
    let state_str = match sidecar_resp {
        IpcResponse::Ok {
            payload: Some(ref d),
            ..
        } => d.get("state").and_then(|v| v.as_str()).unwrap_or(""),
        _ => "",
    };
    assert_eq!(state_str, "Idle");

    // 6. Shutdown via IPC wakes server immediately via Notify
    let shutdown_start = Instant::now();
    let _ = client
        .send_request(&IpcRequest::Shutdown)
        .await
        .expect("Shutdown failed");
    let join_res = tokio::time::timeout(Duration::from_millis(200), server_handle)
        .await
        .expect("Server did not stop within 200ms of Shutdown");
    assert!(join_res.unwrap().is_ok());
    assert!(shutdown_start.elapsed() < Duration::from_millis(200));
}
