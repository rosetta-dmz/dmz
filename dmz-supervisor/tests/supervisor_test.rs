use dmz_supervisor::{
    AgentConfig, AgentRunner, AgentRunnerError, ServiceConfig, ServiceRunner, ServiceStatus,
    SidecarOrchestrator, SidecarOrchestratorError,
};
#[cfg(unix)]
use dmz_supervisor::{IpcServerConfig, MAX_UNIX_SOCKET_PATH};
use std::collections::HashMap;
use std::time::Instant;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn test_service_runner_supervises_natural_exit_and_reaps_zombie() {
    let runner = ServiceRunner::new();
    let name = "clean-service".to_string();

    let (command, args) = if cfg!(windows) {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), "exit 0".to_string()],
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "exit 0".to_string()],
        )
    };

    let config = ServiceConfig {
        name: name.clone(),
        command,
        args,
        envs: HashMap::new(),
        working_dir: std::env::temp_dir(),
        health_check_addr: None,
        health_check_timeout_secs: 1,
    };

    runner
        .start_service(config)
        .await
        .expect("Failed to start service");

    // Active polling with bounded backoff
    let start = Instant::now();
    loop {
        let statuses = runner.get_status_all().await;
        if statuses.get(&name) == Some(&ServiceStatus::Stopped) {
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            panic!("Service did not transition to Stopped within 3 seconds");
        }
        sleep(Duration::from_millis(15)).await;
    }

    let statuses = runner.get_status_all().await;
    assert_eq!(
        statuses.get(&name),
        Some(&ServiceStatus::Stopped),
        "Expected service status to transition to Stopped on exit code 0"
    );
}

#[tokio::test]
async fn test_service_runner_supervises_crash_and_records_failure() {
    let runner = ServiceRunner::new();
    let name = "crashing-service".to_string();

    let (command, args) = if cfg!(windows) {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), "exit 42".to_string()],
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "exit 42".to_string()],
        )
    };

    let config = ServiceConfig {
        name: name.clone(),
        command,
        args,
        envs: HashMap::new(),
        working_dir: std::env::temp_dir(),
        health_check_addr: None,
        health_check_timeout_secs: 1,
    };

    runner
        .start_service(config)
        .await
        .expect("Failed to start service");

    // Active polling with bounded backoff
    let start = Instant::now();
    let failure_msg = loop {
        let statuses = runner.get_status_all().await;
        if let Some(ServiceStatus::Failed(ref msg)) = statuses.get(&name) {
            break msg.clone();
        }
        if start.elapsed() > Duration::from_secs(3) {
            panic!("Service did not transition to Failed within 3 seconds");
        }
        sleep(Duration::from_millis(15)).await;
    };
    assert!(
        failure_msg.contains("42"),
        "Expected failure message to reference exit code 42, got: {}",
        failure_msg
    );
}

#[tokio::test]
async fn test_agent_runner_reaped_flag_prevents_signal_after_natural_exit() {
    let runner = AgentRunner::new();
    let agent_id = "agent-natural-exit".to_string();

    let (cmd, args) = if cfg!(windows) {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), "exit 0".to_string()],
        )
    } else {
        (
            "/bin/sh".to_string(),
            vec!["-c".to_string(), "exit 0".to_string()],
        )
    };

    let config = AgentConfig {
        agent_id: agent_id.clone(),
        binary_or_script: cmd,
        args,
        envs: HashMap::new(),
        allowed_domains: vec!["api.openai.com".to_string()],
        scratchpad_bytes: 1024 * 1024,
    };

    runner
        .spawn_agent(config)
        .await
        .expect("Failed to spawn agent");

    // Wait for the agent to exit naturally and be removed from the active map
    let start = Instant::now();
    loop {
        if runner.list_active_agents().await.is_empty() {
            break;
        }
        if start.elapsed() > Duration::from_secs(3) {
            panic!("Agent did not exit naturally within 3 seconds");
        }
        sleep(Duration::from_millis(15)).await;
    }

    assert!(
        runner.list_active_agents().await.is_empty(),
        "Active agent list must be empty after natural exit"
    );

    // Ensure terminate_agent call on already-reaped process returns AgentRunnerError::NotFound
    let term_res = runner.terminate_agent(&agent_id).await;
    assert!(
        term_res.is_err(),
        "Terminating a naturally reaped agent must return an error"
    );
    let term_err = term_res.expect_err("expected AgentRunnerError::NotFound");
    assert!(
        matches!(term_err, AgentRunnerError::NotFound { .. }),
        "Expected AgentRunnerError::NotFound, got: {:?}",
        term_err
    );
}

#[tokio::test]
async fn test_readiness_probe_enforces_configured_timeout() {
    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let sock_path = tmp.path().join("non_existent_probe.sock");

    let start = Instant::now();
    let res = SidecarOrchestrator::poll_socket_readiness(&sock_path, 200).await;
    let elapsed = start.elapsed();

    assert!(res.is_err(), "Readiness probe must fail for unbound socket");
    let err = res.expect_err("expected readiness probe to time out");
    assert!(
        matches!(
            err,
            SidecarOrchestratorError::ReadinessTimeout {
                timeout_ms: 200,
                ..
            }
        ),
        "Expected ReadinessTimeout variant, got: {:?}",
        err
    );
    assert!(
        err.contains("TIMED OUT"),
        "Error message must specify TIMED OUT, got: {}",
        err
    );
    assert!(
        elapsed >= Duration::from_millis(150),
        "Probe elapsed {:?}, expected at least ~150ms for a 200ms timeout",
        elapsed
    );
}

#[cfg(unix)]
#[test]
fn test_ipc_server_secure_default_socket_path_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let config = IpcServerConfig::default();
    assert!(
        config.socket_path.to_string_lossy().contains("dmz"),
        "Socket path must contain dmz namespace"
    );
    assert!(
        config.socket_path.as_os_str().len() < MAX_UNIX_SOCKET_PATH,
        "Default socket path length ({} bytes) must not exceed POSIX limit ({} bytes)",
        config.socket_path.as_os_str().len(),
        MAX_UNIX_SOCKET_PATH
    );

    if let Some(parent) = config.socket_path.parent() {
        if parent.exists() {
            let meta = std::fs::metadata(parent).expect("Failed to get parent metadata");
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "Expected parent directory mode 0700, got: {:o}",
                mode
            );
        }
    }
}
