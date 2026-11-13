use dmz_supervisor::sidecar_orchestrator::{
    SidecarLifecycleState, SidecarOrchestrator, SidecarOrchestratorError,
};
use std::fs;

/// RAII cleanup guard to guarantee sidecar and primary process termination on test failure
struct SidecarTestGuard<'a> {
    orchestrator: &'a SidecarOrchestrator,
    active: bool,
}

impl<'a> SidecarTestGuard<'a> {
    fn new(orchestrator: &'a SidecarOrchestrator) -> Self {
        Self {
            orchestrator,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl<'a> Drop for SidecarTestGuard<'a> {
    fn drop(&mut self) {
        if self.active {
            let orch = self.orchestrator.clone();
            let _ = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok();
                if let Some(rt) = rt {
                    let _ = rt.block_on(async { orch.stop_pair().await });
                }
            })
            .join();
        }
    }
}

#[tokio::test]
async fn test_safe_socket_preparation_and_stale_cleanup() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let ws_path = temp_dir.path().join("my_project");
    fs::create_dir_all(&ws_path).expect("create project directory");

    let sock_path = SidecarOrchestrator::prepare_safe_socket(&ws_path, "sidecar")
        .expect("prepare safe socket path");
    assert!(sock_path.to_string_lossy().contains("dmz_"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = sock_path.parent() {
            if parent.exists() {
                let meta = fs::metadata(parent).expect("query socket directory metadata");
                let mode = meta.permissions().mode() & 0o777;
                assert_eq!(
                    mode, 0o700,
                    "Socket directory must enforce strict 0o700 permissions"
                );
            }
        }

        // Write a mock stale socket file
        fs::write(&sock_path, b"stale socket").expect("write mock stale socket");
        assert!(sock_path.exists());

        // Calling prepare_safe_socket again must clean up the stale socket automatically
        let prepared_again = SidecarOrchestrator::prepare_safe_socket(&ws_path, "sidecar")
            .expect("prepare safe socket again");
        assert_eq!(sock_path, prepared_again);
        assert!(!prepared_again.exists());
    }

    #[cfg(windows)]
    {
        assert!(sock_path.to_string_lossy().starts_with(r"\\.\pipe\dmz_"));
        // Calling prepare_safe_socket again works idempotently on Windows named pipes
        let prepared_again = SidecarOrchestrator::prepare_safe_socket(&ws_path, "sidecar")
            .expect("prepare safe socket again");
        assert_eq!(sock_path, prepared_again);
    }
}

#[tokio::test]
async fn test_readiness_probe_gate_timeout() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let sock_path = temp_dir.path().join("unbound_sidecar.sock");

    // Probing an unbound socket should fail fast after the timeout
    let res = SidecarOrchestrator::poll_socket_readiness(&sock_path, 100).await;
    assert!(res.is_err());
    let err = res.expect_err("expected readiness probe to time out");
    assert!(matches!(
        err,
        SidecarOrchestratorError::ReadinessTimeout {
            timeout_ms: 100,
            ..
        }
    ));
    assert!(err.contains("TIMED OUT"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_readiness_probe_gate_success() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let sock_path = temp_dir.path().join("active_sidecar.sock");

    // Bind a listener simulating a responsive sidecar process
    let listener =
        std::os::unix::net::UnixListener::bind(&sock_path).expect("bind mock unix socket listener");

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
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let sock_path = temp_dir.path().join("crashing_service.sock");

    // Spawn a child process that immediately exits with error code 42
    let mut child = if cfg!(windows) {
        tokio::process::Command::new("cmd.exe")
            .args(["/c", "exit 42"])
            .spawn()
            .expect("spawn crashing process")
    } else {
        tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 42"])
            .spawn()
            .expect("spawn crashing process")
    };

    // Fail probe fast in < 500ms
    let start = std::time::Instant::now();
    let res =
        SidecarOrchestrator::poll_socket_readiness_with_child(&sock_path, 10000, Some(&mut child))
            .await;
    let elapsed = start.elapsed();

    assert!(res.is_err());
    let err = res.expect_err("expected companion crash error");
    assert!(matches!(
        err,
        SidecarOrchestratorError::CompanionExitedPrematurely { .. }
    ));
    assert!(err.contains("Companion service process exited prematurely"));
    assert!(
        elapsed.as_millis() < 500,
        "Probe took {}ms, expected fail-fast under 500ms",
        elapsed.as_millis()
    );

    // Explicitly reap child process to eliminate zombie
    let _ = child.wait().await;
}

#[test]
fn test_infer_member_execution_tauri_and_npm() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let ws_path = temp_dir.path().join("tauri_member");
    fs::create_dir_all(&ws_path).expect("create tauri member directory");

    fs::write(
        ws_path.join("package.json"),
        r#"{"name": "desktop-ui", "scripts": {"tauri": "tauri"}}"#,
    )
    .expect("write package.json");

    let member = dmz_core::workspace::WorkspaceMember::new(
        "ui",
        &ws_path,
        dmz_core::workspace::MemberRole::Sidecar,
    );

    let (cmd, args) = SidecarOrchestrator::infer_member_execution(&ws_path, &member)
        .expect("infer member execution for tauri");
    assert_eq!(cmd, "npm");
    assert_eq!(args, vec!["run", "tauri", "dev"]);

    // When pnpm-lock.yaml is present, infer_member_execution must select pnpm instead of npm
    fs::write(ws_path.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n")
        .expect("write pnpm-lock.yaml");
    let (pnpm_cmd, pnpm_args) = SidecarOrchestrator::infer_member_execution(&ws_path, &member)
        .expect("infer member execution for pnpm");
    assert_eq!(pnpm_cmd, "pnpm");
    assert_eq!(pnpm_args, vec!["run", "tauri", "dev"]);

    // With explicit workspace_root, nested members (e.g. apps/web) detect root pnpm-workspace.yaml
    let mono_root = temp_dir.path().join("mono_ws");
    let nested_web = mono_root.join("apps/web");
    fs::create_dir_all(&nested_web).expect("create nested web dir");
    fs::write(
        mono_root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n",
    )
    .expect("write pnpm-workspace.yaml");
    fs::write(
        nested_web.join("package.json"),
        r#"{"name": "web", "scripts": {"dev": "vite"}}"#,
    )
    .expect("write nested package.json");
    let nested_member = dmz_core::workspace::WorkspaceMember::new(
        "web",
        "apps/web",
        dmz_core::workspace::MemberRole::Primary,
    );
    let (mono_cmd, mono_args) =
        SidecarOrchestrator::infer_member_execution_in(&mono_root, &nested_web, &nested_member)
            .expect("infer nested monorepo pnpm command");
    assert_eq!(mono_cmd, "pnpm");
    assert_eq!(mono_args, vec!["run", "dev"]);
}

#[test]
fn test_infer_member_execution_python() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let ws_path = temp_dir.path().join("py_member");
    fs::create_dir_all(&ws_path).expect("create py member directory");

    fs::write(ws_path.join("requirements.txt"), "fastapi\n").expect("write requirements.txt");
    fs::write(ws_path.join("server.py"), "print('server')\n").expect("write server.py");

    let member = dmz_core::workspace::WorkspaceMember::new(
        "backend",
        &ws_path,
        dmz_core::workspace::MemberRole::Sidecar,
    );

    let (cmd, args) = SidecarOrchestrator::infer_member_execution(&ws_path, &member)
        .expect("infer member execution for python");
    if cfg!(windows) {
        assert_eq!(cmd, "python");
    } else {
        assert_eq!(cmd, "python3");
    }
    assert_eq!(args, vec!["server.py"]);
}

#[test]
fn test_infer_member_execution_undetectable_fails_with_clear_message() {
    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let ws_path = temp_dir.path().join("empty_member");
    fs::create_dir_all(&ws_path).expect("create empty member directory");

    let member = dmz_core::workspace::WorkspaceMember::new(
        "unknown",
        &ws_path,
        dmz_core::workspace::MemberRole::Sidecar,
    );

    let res = SidecarOrchestrator::infer_member_execution(&ws_path, &member);
    assert!(res.is_err());
    let err = res.expect_err("expected inference failure");
    assert!(matches!(
        err,
        SidecarOrchestratorError::ExecutionInferenceFailed { .. }
    ));
    assert!(
        err.contains("Unable to determine execution command for workspace member"),
        "Expected error message, got: {}",
        err
    );
    assert!(
        err.contains("Specify \"exec_command\" and \"exec_args\" in dmz-workspace.toml"),
        "Expected mention of dmz-workspace.toml, got: {}",
        err
    );
}

#[tokio::test]
async fn test_multi_sidecar_lifecycle_and_cancelled_wait_for_exit_teardown() {
    use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
    use dmz_supervisor::sidecar_orchestrator::SidecarPairConfig;
    use std::time::Duration;

    let temp_dir = tempfile::tempdir().expect("create tempdir");
    let root = temp_dir.path();
    let prim_dir = root.join("primary");
    let sc1_dir = root.join("sidecar_one");
    let sc2_dir = root.join("sidecar_two");
    fs::create_dir_all(&prim_dir).expect("create primary directory");
    fs::create_dir_all(&sc1_dir).expect("create sidecar 1 directory");
    fs::create_dir_all(&sc2_dir).expect("create sidecar 2 directory");

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

    // Arm RAII cleanup guard to guarantee child process termination on assertion failure
    let mut guard = SidecarTestGuard::new(&orchestrator);

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

    // Disarm guard after successful explicit teardown
    guard.disarm();

    assert_eq!(
        orchestrator.current_state().await,
        SidecarLifecycleState::Stopped
    );
    assert!(!sock.exists());
}
