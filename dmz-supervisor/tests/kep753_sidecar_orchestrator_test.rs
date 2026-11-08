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

#[test]
fn test_infer_member_execution_tauri_and_npm() {
    let temp_dir = tempfile::tempdir().unwrap();
    let ws_path = temp_dir.path().join("tauri_member");
    fs::create_dir_all(&ws_path).unwrap();

    fs::write(
        ws_path.join("package.json"),
        r#"{"name": "desktop-ui", "scripts": {"tauri": "tauri"}}"#,
    )
    .unwrap();

    let member = dmz_core::workspace::WorkspaceMember::new(
        "ui",
        &ws_path,
        dmz_core::workspace::MemberRole::Sidecar,
    );

    let (cmd, args) = SidecarOrchestrator::infer_member_execution(&ws_path, &member).unwrap();
    assert_eq!(cmd, "npm");
    assert_eq!(args, vec!["run", "tauri", "dev"]);
}

#[test]
fn test_infer_member_execution_python() {
    let temp_dir = tempfile::tempdir().unwrap();
    let ws_path = temp_dir.path().join("py_member");
    fs::create_dir_all(&ws_path).unwrap();

    fs::write(ws_path.join("requirements.txt"), "fastapi\n").unwrap();
    fs::write(ws_path.join("server.py"), "print('server')\n").unwrap();

    let member = dmz_core::workspace::WorkspaceMember::new(
        "backend",
        &ws_path,
        dmz_core::workspace::MemberRole::Sidecar,
    );

    let (cmd, args) = SidecarOrchestrator::infer_member_execution(&ws_path, &member).unwrap();
    if cfg!(windows) {
        assert_eq!(cmd, "python");
    } else {
        assert_eq!(cmd, "python3");
    }
    assert_eq!(args, vec!["server.py"]);
}

#[test]
fn test_infer_member_execution_undetectable_fails_with_clear_message() {
    let temp_dir = tempfile::tempdir().unwrap();
    let ws_path = temp_dir.path().join("empty_member");
    fs::create_dir_all(&ws_path).unwrap();

    let member = dmz_core::workspace::WorkspaceMember::new(
        "unknown",
        &ws_path,
        dmz_core::workspace::MemberRole::Sidecar,
    );

    let res = SidecarOrchestrator::infer_member_execution(&ws_path, &member);
    assert!(res.is_err());
    let err_msg = res.unwrap_err();
    assert!(
        err_msg.contains("Unable to determine execution command for workspace member"),
        "Expected error message, got: {}",
        err_msg
    );
    assert!(
        err_msg.contains("Specify \"exec_command\" and \"exec_args\" in dmz-workspace.toml"),
        "Expected mention of dmz-workspace.toml, got: {}",
        err_msg
    );
}
