use dmz_supervisor::{
    AgentConfig, AgentRunner, IpcClient, IpcRequest, IpcResponse, IpcServer, IpcServerConfig,
    ServiceRunner, SidecarOrchestrator,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{sleep, Duration};

/// RAII cleanup guard for child process termination on panic or early exit
struct AgentTestGuard<'a> {
    runner: &'a AgentRunner,
    agent_id: String,
    active: bool,
}

impl<'a> AgentTestGuard<'a> {
    fn new(runner: &'a AgentRunner, agent_id: String) -> Self {
        Self {
            runner,
            agent_id,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl<'a> Drop for AgentTestGuard<'a> {
    fn drop(&mut self) {
        if self.active {
            let runner = self.runner.clone();
            let agent_id = self.agent_id.clone();
            // Best-effort child process reap on test failure/panic
            let _ = std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok();
                if let Some(rt) = rt {
                    let _ = rt.block_on(async { runner.terminate_agent(&agent_id).await });
                }
            })
            .join();
        }
    }
}

#[tokio::test]
async fn test_agent_runner_retains_scratchpad_and_terminates_without_deadlock() {
    let runner = AgentRunner::new();
    let agent_id = "test-agent-lifecycle".to_string();

    // Spawn 30-second blocking process to prove `terminate_agent` never deadlocks on `child.wait()`
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

    // Arm guard to ensure process termination even if subsequent assertions fail
    let mut guard = AgentTestGuard::new(&runner, agent_id.clone());

    // Agent must be registered in active agents
    let active = runner.list_active_agents().await;
    assert_eq!(active, vec![agent_id.clone()]);

    let scratchpad_path = runner
        .get_scratchpad_path(&agent_id)
        .await
        .expect("Scratchpad path must exist for active agent");
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

    // Disarm guard after successful explicit termination
    guard.disarm();

    // Remove agent immediately and wipe scratchpad
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
    let service_runner = ServiceRunner::new();

    let server = Arc::new(IpcServer::with_all_managers(
        IpcServerConfig {
            socket_path: socket_path.clone(),
        },
        runner.clone(),
        orchestrator,
        Some(service_runner),
    ));

    let server_clone = server.clone();
    let server_handle = tokio::spawn(async move { server_clone.start().await });

    let client = IpcClient::new(socket_path.clone());

    // Active readiness poll with bounded backoff (replaces arbitrary sleep anti-pattern)
    let poll_start = Instant::now();
    let mut server_ready = false;
    while poll_start.elapsed() < Duration::from_millis(1500) {
        if client.send_request(&IpcRequest::Ping).await.is_ok() {
            server_ready = true;
            break;
        }
        sleep(Duration::from_millis(15)).await;
    }
    assert!(
        server_ready,
        "IPC server failed to become ready within 1500ms"
    );
    assert!(server.is_running(), "Server must report is_running == true");

    // Ping
    let ping_resp = client
        .send_request(&IpcRequest::Ping)
        .await
        .expect("Ping failed");
    assert!(matches!(ping_resp, IpcResponse::Ok { ref message, .. } if message == "pong"));

    // StartAgent via IPC
    let start_resp = client
        .send_request(&IpcRequest::StartAgent {
            agent_id: "ipc-agent-01".to_string(),
            allowed_domains: vec!["crates.io".to_string()],
            scratchpad_bytes: 2 * 1024 * 1024,
            command: None,
            args: None,
            isolation_mode: dmz_sandbox::platform::SandboxIsolationMode::BestEffort,
        })
        .await
        .expect("StartAgent failed");
    assert!(
        matches!(start_resp, IpcResponse::Ok { ref message, .. } if message.contains("ipc-agent-01"))
    );

    // GetStatus reflects active_agents == 1
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

    // GetServiceStatus via IPC
    let service_resp = client
        .send_request(&IpcRequest::GetServiceStatus)
        .await
        .expect("GetServiceStatus failed");
    assert!(
        matches!(service_resp, IpcResponse::Ok { ref message, .. } if message.contains("status"))
    );

    // RunSandbox via IPC (executes benign echo)
    let (echo_cmd, echo_args) = if cfg!(windows) {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), "echo dmz-test".to_string()],
        )
    } else {
        ("echo".to_string(), vec!["dmz-test".to_string()])
    };
    let sandbox_resp = client
        .send_request(&IpcRequest::RunSandbox {
            workspace_path: tmp.path().to_string_lossy().to_string(),
            command: echo_cmd,
            args: echo_args,
            allow_network: false,
            isolation_mode: dmz_sandbox::platform::SandboxIsolationMode::BestEffort,
        })
        .await
        .expect("RunSandbox failed");
    assert!(
        matches!(sandbox_resp, IpcResponse::Ok { ref message, .. } if message.contains("Sandbox command completed"))
    );

    // StopAgent via IPC
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

    // GetSidecarStatus via IPC
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

    // Shutdown via IPC wakes server immediately via Notify
    let shutdown_start = Instant::now();
    let _ = client
        .send_request(&IpcRequest::Shutdown)
        .await
        .expect("Shutdown failed");
    let join_res = tokio::time::timeout(Duration::from_millis(500), server_handle)
        .await
        .expect("Server did not stop within 500ms of Shutdown");
    assert!(join_res.expect("Server task panicked").is_ok());
    assert!(shutdown_start.elapsed() < Duration::from_millis(500));

    // Assert post-shutdown invariants
    assert!(
        !server.is_running(),
        "Server must report is_running == false after shutdown"
    );
    #[cfg(unix)]
    assert!(
        !socket_path.exists(),
        "Unix domain socket file must be unlinked from filesystem after server shutdown"
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_live_seatbelt_confined_connect_tunnel_and_bypass_eperm() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    // /usr/bin/perl is a direct Mach-O binary on macOS (unlike /usr/bin/python3, which is an
    // xcselect shim reading /var/select/developer_dir).
    let perl = "/usr/bin/perl";
    assert!(
        std::path::Path::new(perl).exists(),
        "/usr/bin/perl must exist on macOS to run Seatbelt E2E network probe"
    );

    // Local TCP echo/handshake server acting as the upstream destination
    let upstream = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream listener");
    let upstream_port = upstream.local_addr().expect("upstream addr").port();

    let (tx_proof, rx_proof) = tokio::sync::oneshot::channel::<String>();
    let upstream_task = tokio::spawn(async move {
        if let Ok((stream, _)) = upstream.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            let _ = buf_reader.read_line(&mut line).await;
            let _ = writer.write_all(b"UPSTREAM_ACK\n").await;
            let _ = writer.shutdown().await;
            let _ = tx_proof.send(line);
        }
    });

    // Perl probe running INSIDE the Seatbelt-confined agent:
    // 1. Verifies raw direct TCP to upstream_port is blocked by Seatbelt with EPERM.
    // 2. Dials HTTP_PROXY (127.0.0.1:<proxy_port>), issues CONNECT 127.0.0.1:<upstream_port>,
    //    asserts 200 Connection Established, exchanges payload with upstream.
    let probe_script = format!(
        concat!(
            "use strict; use warnings; use Errno qw(EPERM); use IO::Socket::INET;\n",
            "my $upstream_port = {upstream_port};\n",
            "my $direct = IO::Socket::INET->new(PeerAddr => '127.0.0.1', PeerPort => $upstream_port, Proto => 'tcp', Timeout => 2);\n",
            "if ($direct) {{ close($direct); die \"ERROR: direct connect bypassed Seatbelt\\n\"; }}\n",
            "die \"ERROR: expected EPERM, got errno=\" . (0 + $!) . \" ($!)\\n\" unless $! == EPERM;\n",
            "my $proxy_env = $ENV{{HTTP_PROXY}} or die \"ERROR: missing HTTP_PROXY\\n\";\n",
            "$proxy_env =~ m{{^http://([^:]+):(\\d+)}} or die \"ERROR: bad HTTP_PROXY=$proxy_env\\n\";\n",
            "my ($phost, $pport) = ($1, $2);\n",
            "my $sock = IO::Socket::INET->new(PeerAddr => $phost, PeerPort => $pport, Proto => 'tcp', Timeout => 5)\n",
            "    or die \"ERROR: failed dialing proxy $phost:$pport: $!\\n\";\n",
            "$sock->autoflush(1);\n",
            "print $sock \"CONNECT 127.0.0.1:$upstream_port HTTP/1.1\\r\\nHost: 127.0.0.1:$upstream_port\\r\\n\\r\\n\";\n",
            "my $headers = '';\n",
            "while ($headers !~ /\\r\\n\\r\\n/) {{\n",
            "    my $buf = '';\n",
            "    my $n = sysread($sock, $buf, 4096);\n",
            "    die \"ERROR: EOF reading CONNECT headers\\n\" unless defined($n) && $n > 0;\n",
            "    $headers .= $buf;\n",
            "}}\n",
            "die \"ERROR: CONNECT rejected: $headers\\n\" unless $headers =~ m{{^HTTP/1\\.[01] 200}};\n",
            "syswrite($sock, \"SEATBELT_EPERM_OK\\n\");\n",
            "my $ack = '';\n",
            "sysread($sock, $ack, 1024);\n",
            "close($sock);\n",
            "die \"ERROR: unexpected upstream ack: $ack\\n\" unless $ack eq \"UPSTREAM_ACK\\n\";\n",
        ),
        upstream_port = upstream_port,
    );

    let runner = AgentRunner::new();
    let agent_id = "seatbelt-live-e2e".to_string();
    let mut guard = AgentTestGuard::new(&runner, agent_id.clone());

    runner
        .spawn_agent_with_mode(
            AgentConfig {
                agent_id: agent_id.clone(),
                binary_or_script: perl.to_string(),
                args: vec!["-e".to_string(), probe_script],
                envs: HashMap::new(),
                allowed_domains: vec!["127.0.0.1".to_string()],
                scratchpad_bytes: 4 * 1024 * 1024,
            },
            dmz_sandbox::platform::SandboxIsolationMode::StrictKernel,
        )
        .await
        .expect("spawn_agent_with_mode(StrictKernel) must succeed on macOS Seatbelt");

    // Wait for upstream proof to arrive via the Seatbelt-confined CONNECT tunnel
    let proof = tokio::time::timeout(Duration::from_secs(10), rx_proof)
        .await
        .expect("timed out waiting for Seatbelt CONNECT tunnel traffic")
        .expect("upstream channel closed without receiving tunneled payload");
    assert_eq!(proof, "SEATBELT_EPERM_OK\n");

    let _ = upstream_task.await;

    // Wait briefly for the watcher to record natural exit cleanup
    for _ in 0..40 {
        if runner.list_active_agents().await.is_empty() {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    guard.disarm();
    assert!(
        runner.list_active_agents().await.is_empty(),
        "Confined Seatbelt agent should exit cleanly after proof exchange"
    );

    // Also prove empty allowed_domains (proxy_addr: None) blocks all outbound connects with EPERM
    let offline_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind offline listener");
    let offline_port = offline_listener.local_addr().expect("offline addr").port();

    let tmp_ws = tempfile::tempdir().expect("temp workspace");
    let tmp_store = tempfile::tempdir().expect("temp store");
    let offline_script = format!(
        concat!(
            "use strict; use warnings; use Errno qw(EPERM); use IO::Socket::INET;\n",
            "my $s = IO::Socket::INET->new(PeerAddr => '127.0.0.1', PeerPort => {port}, Proto => 'tcp', Timeout => 2);\n",
            "exit(2) if $s;\n",
            "exit($! == EPERM ? 0 : 3);\n",
        ),
        port = offline_port,
    );
    let (mut cmd, posture) = dmz_sandbox::platform::prepare_agent_command(
        &dmz_sandbox::platform::SandboxConfig {
            workspace_path: tmp_ws.path().to_path_buf(),
            closure_path: tmp_store.path().to_path_buf(),
            command: perl.to_string(),
            args: vec!["-e".to_string(), offline_script],
            envs: Vec::new(),
            allow_network: false,
            ipc_socket_path: None,
        },
        &dmz_sandbox::platform::SandboxPolicyOptions {
            proxy_addr: None,
            writable_paths: Vec::new(),
            isolation_mode: dmz_sandbox::platform::SandboxIsolationMode::StrictKernel,
        },
    )
    .expect("prepare_agent_command(no_network) must succeed");
    assert_eq!(posture, "kernel-seatbelt-no-network");

    let status = cmd
        .status()
        .expect("execute Seatbelt no_network perl probe");
    assert!(
        status.success(),
        "no_network Seatbelt profile must deny outbound TCP with EPERM (status={status})"
    );
}
