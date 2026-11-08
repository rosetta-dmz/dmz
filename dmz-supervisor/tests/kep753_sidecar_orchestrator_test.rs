use dmz_supervisor::sidecar_orchestrator::{SidecarLifecycleState, SidecarOrchestrator};
use std::fs;

#[tokio::test]
async fn test_safe_socket_preparation_and_stale_cleanup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let ws_path = temp_dir.path().join("my_project");
    fs::create_dir_all(&ws_path).unwrap();

    let sock_path = SidecarOrchestrator::prepare_safe_socket(&ws_path, "sidecar").unwrap();
    assert!(sock_path.to_string_lossy().contains("dmz_"));

    #[cfg(unix)]
    {
        // Write a mock stale socket file
        fs::write(&sock_path, b"stale socket").unwrap();
        assert!(sock_path.exists());

        // Calling prepare_safe_socket again must clean up the stale socket automatically
        let prepared_again = SidecarOrchestrator::prepare_safe_socket(&ws_path, "sidecar").unwrap();
        assert_eq!(sock_path, prepared_again);
        assert!(!prepared_again.exists());
    }

    #[cfg(windows)]
    {
        assert!(sock_path.to_string_lossy().starts_with(r"\\.\pipe\dmz_"));
        // Calling prepare_safe_socket again works idempotently on Windows named pipes
        let prepared_again = SidecarOrchestrator::prepare_safe_socket(&ws_path, "sidecar").unwrap();
        assert_eq!(sock_path, prepared_again);
    }
}

#[tokio::test]
async fn test_readiness_probe_gate_timeout() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sock_path = temp_dir.path().join("unbound_sidecar.sock");

    // Probing an unbound socket should fail fast after the timeout
    let res = SidecarOrchestrator::poll_socket_readiness(&sock_path, 100).await;
    assert!(res.is_err());
    let err_msg = res.unwrap_err();
    assert!(err_msg.contains("TIMED OUT"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_readiness_probe_gate_success() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sock_path = temp_dir.path().join("active_sidecar.sock");

    // Bind a listener simulating a responsive sidecar process
    let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();

    let res = SidecarOrchestrator::poll_socket_readiness(&sock_path, 2000).await;
    assert!(res.is_ok());

    drop(listener);
    let _ = fs::remove_file(&sock_path);
}

#[tokio::test]
async fn test_orchestrator_initial_state_and_queries() {
    let orchestrator = SidecarOrchestrator::new();
    assert_eq!(
        orchestrator.current_state().await,
        SidecarLifecycleState::Idle
    );
    assert_eq!(orchestrator.current_socket_path().await, None);
}

#[tokio::test]
async fn test_fail_fast_readiness_on_premature_exit() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sock_path = temp_dir.path().join("crashing_service.sock");

    // Spawn a child process that immediately exits with error code 42
    let mut child = if cfg!(windows) {
        tokio::process::Command::new("cmd.exe")
            .args(["/c", "exit 42"])
            .spawn()
            .unwrap()
    } else {
        tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 42"])
            .spawn()
            .unwrap()
    };

    // The probe should fail fast in <100ms instead of waiting for a 10s timeout
    let start = std::time::Instant::now();
    let res =
        SidecarOrchestrator::poll_socket_readiness_with_child(&sock_path, 10000, Some(&mut child))
            .await;
    let elapsed = start.elapsed();

    assert!(res.is_err());
    let err_msg = res.unwrap_err();
    assert!(err_msg.contains("Companion service process exited prematurely"));
    assert!(
        elapsed.as_millis() < 500,
        "Probe took {}ms, expected fail-fast under 500ms",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn test_multi_sidecar_lifecycle_and_cancelled_wait_for_exit_teardown() {
    use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
    use dmz_supervisor::sidecar_orchestrator::SidecarPairConfig;
    use std::time::Duration;

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();
    let prim_dir = root.join("primary");
    let sc1_dir = root.join("sidecar_one");
    let sc2_dir = root.join("sidecar_two");
    fs::create_dir_all(&prim_dir).unwrap();
    fs::create_dir_all(&sc1_dir).unwrap();
    fs::create_dir_all(&sc2_dir).unwrap();

    let mut primary = WorkspaceMember::new("primary", &prim_dir, MemberRole::Primary);
    let mut sidecar1 = WorkspaceMember::new("sidecar-1", &sc1_dir, MemberRole::Sidecar);
    let mut sidecar2 = WorkspaceMember::new("sidecar-2", &sc2_dir, MemberRole::Service);

    if cfg!(windows) {
        primary.exec_command = Some("cmd.exe".to_string());
        primary.exec_args = Some(vec![
            "/c".to_string(),
            "ping -n 30 127.0.0.1 > nul".to_string(),
        ]);
        sidecar1.exec_command = Some("cmd.exe".to_string());
        sidecar1.exec_args = Some(vec![
            "/c".to_string(),
            "ping -n 30 127.0.0.1 > nul".to_string(),
        ]);
        sidecar2.exec_command = Some("cmd.exe".to_string());
        sidecar2.exec_args = Some(vec![
            "/c".to_string(),
            "ping -n 30 127.0.0.1 > nul".to_string(),
        ]);
    } else {
        primary.exec_command = Some("/bin/sh".to_string());
        primary.exec_args = Some(vec!["-c".to_string(), "sleep 30".to_string()]);
        sidecar1.exec_command = Some("/bin/sh".to_string());
        sidecar1.exec_args = Some(vec!["-c".to_string(), "sleep 30".to_string()]);
        sidecar2.exec_command = Some("/bin/sh".to_string());
        sidecar2.exec_args = Some(vec!["-c".to_string(), "sleep 30".to_string()]);
    }

    let mut project = WorkspaceProjectConfig::new("suite", vec![primary, sidecar1, sidecar2]);
    // Skip socket probe (0ms) to test process orchestration across platforms without a socket listener script
    project.ipc.readiness_timeout_ms = 0;
    project.ipc.drain_timeout_ms = 200;

    let orchestrator = SidecarOrchestrator::new();
    let sock = orchestrator
        .start_pair(SidecarPairConfig {
            workspace_root: root.to_path_buf(),
            project,
            allow_network: false,
        })
        .await
        .expect("Failed to start multi-sidecar pair");

    assert_eq!(
        orchestrator.current_state().await,
        SidecarLifecycleState::Running
    );

    // Simulate `dmz run` Ctrl+C: cancel `wait_for_exit()` via tokio::select! and then invoke `stop_pair()`
    tokio::select! {
        _ = orchestrator.wait_for_exit() => {
            panic!("Primary should still be sleeping");
        }
        _ = tokio::time::sleep(Duration::from_millis(120)) => {}
    }

    orchestrator
        .stop_pair()
        .await
        .expect("stop_pair failed after cancelled wait_for_exit");
    assert_eq!(
        orchestrator.current_state().await,
        SidecarLifecycleState::Stopped
    );
    assert!(!sock.exists());
}
