use dmz_sandbox::agent::{AgentSandbox, LoopbackAllowlistProxy};
#[cfg(target_os = "windows")]
use dmz_sandbox::platform::windows::WindowsJobGuard;
use dmz_sandbox::platform::{
    prepare_agent_command, MetadataReadMode, SandboxConfig, SandboxIsolationMode,
    SandboxPolicyOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::{oneshot, RwLock};
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum AgentRunnerError {
    #[error("Agent with ID '{0}' is already running")]
    AlreadyRunning(String),
    #[error("Agent '{0}' not found")]
    NotFound(String),
    #[error("Failed to allocate agent sandbox: {0}")]
    SandboxAllocation(String),
    #[error("Failed to spawn agent process: {0}")]
    ProcessSpawn(#[source] std::io::Error),
    #[error("Platform error: {0}")]
    Platform(String),
}

impl From<AgentRunnerError> for String {
    fn from(err: AgentRunnerError) -> Self {
        err.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub binary_or_script: String,
    pub args: Vec<String>,
    pub envs: HashMap<String, String>,
    pub allowed_domains: Vec<String>,
    pub scratchpad_bytes: usize,
}

pub enum AgentState {
    Starting,
    Running(Box<ActiveAgent>),
}

pub struct ActiveAgent {
    pub config: AgentConfig,
    pub sandbox: AgentSandbox,
    pub isolation_posture: &'static str,
    _proxy: Option<LoopbackAllowlistProxy>,
    #[cfg(target_os = "windows")]
    _job_guard: Option<WindowsJobGuard>,
    kill_tx: Option<oneshot::Sender<oneshot::Sender<()>>>,
    #[cfg(unix)]
    pgid: Option<i32>,
    reaped: Arc<AtomicBool>,
}

impl Drop for ActiveAgent {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // CRITICAL DEFENSE: Never signal pgid <= 1 (prevents supervisor self-suicide).
            // pgid == 0 signals calling process group; pgid == -1 / 1 signals all host processes.
            if pgid > 1 && !self.reaped.load(Ordering::SeqCst) {
                // SAFETY: Signalling dedicated agent process group on drop with validated positive PGID.
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
        }
        if let Some(kill_tx) = self.kill_tx.take() {
            let (ack_tx, _) = oneshot::channel();
            let _ = kill_tx.send(ack_tx);
        }
    }
}

/// RAII Guard that rolls back the "Starting" reservation if the spawning future
/// fails or is cancelled before the active agent is committed.
struct ReservationGuard {
    agent_id: String,
    agents_arc: Arc<RwLock<HashMap<String, AgentState>>>,
    committed: bool,
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        if !self.committed {
            if let Ok(mut lock) = self.agents_arc.try_write() {
                if let Some(AgentState::Starting) = lock.get(&self.agent_id) {
                    lock.remove(&self.agent_id);
                }
            } else {
                let agents = Arc::clone(&self.agents_arc);
                let id = self.agent_id.clone();
                tokio::spawn(async move {
                    let mut lock = agents.write().await;
                    if let Some(AgentState::Starting) = lock.get(&id) {
                        lock.remove(&id);
                    }
                });
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct AgentRunner {
    active_agents: Arc<RwLock<HashMap<String, AgentState>>>,
}

impl AgentRunner {
    pub fn new() -> Self {
        Self {
            active_agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn new AI agent wrapped in an isolated dmz-sandbox environment (`BestEffort` mode).
    pub async fn spawn_agent(&self, config: AgentConfig) -> Result<(), AgentRunnerError> {
        self.spawn_agent_with_mode(config, SandboxIsolationMode::BestEffort)
            .await
    }

    /// Spawn new AI agent wrapped in an isolated dmz-sandbox environment with explicit [`SandboxIsolationMode`].
    pub async fn spawn_agent_with_mode(
        &self,
        config: AgentConfig,
        isolation_mode: SandboxIsolationMode,
    ) -> Result<(), AgentRunnerError> {
        let agent_id = config.agent_id.clone();

        // Step 1: Atomically check and reserve the agent_id under write lock
        {
            let mut agents = self.active_agents.write().await;
            match agents.entry(agent_id.clone()) {
                Entry::Occupied(_) => {
                    return Err(AgentRunnerError::AlreadyRunning(agent_id));
                }
                Entry::Vacant(vacant) => {
                    vacant.insert(AgentState::Starting);
                }
            }
        }

        // Arm the cancellation rollback guard
        let mut guard = ReservationGuard {
            agent_id: agent_id.clone(),
            agents_arc: Arc::clone(&self.active_agents),
            committed: false,
        };

        info!(
            agent = %agent_id,
            domains = ?config.allowed_domains,
            isolation_mode = ?isolation_mode,
            "Initializing AI agent sandbox"
        );

        // Step 2: Create native AgentSandbox off the async reactor thread
        let id_for_sb = agent_id.clone();
        let scratchpad_bytes = config.scratchpad_bytes;
        let allowed_domains = config.allowed_domains.clone();
        let sandbox = tokio::task::spawn_blocking(move || {
            AgentSandbox::new(&id_for_sb, scratchpad_bytes, allowed_domains)
        })
        .await
        .map_err(|e| AgentRunnerError::SandboxAllocation(format!("Sandbox join error: {}", e)))?
        .map_err(|e| AgentRunnerError::SandboxAllocation(e.to_string()))?;

        let scratchpad_dir = sandbox.scratchpad_path().to_path_buf();

        // Step 3: Determine network posture and start LoopbackAllowlistProxy when domain-restricted
        let allow_all_network = config.allowed_domains.iter().any(|d| d.trim() == "*");
        let no_network = config.allowed_domains.is_empty();
        let use_proxy = !allow_all_network && !no_network;

        let proxy = if use_proxy {
            Some(
                LoopbackAllowlistProxy::start(sandbox.network_policy.clone())
                    .await
                    .map_err(|e| {
                        AgentRunnerError::Platform(format!(
                            "Failed to bind agent allowlist proxy: {}",
                            e
                        ))
                    })?,
            )
        } else {
            None
        };

        // Step 4: Build sanitized environment variables and kernel-confined Command via dmz-sandbox
        let mut envs: Vec<(String, String)> = Vec::new();
        if let Ok(path) = std::env::var("PATH") {
            envs.push(("PATH".into(), path));
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(sysroot) = std::env::var("SYSTEMROOT") {
                envs.push(("SYSTEMROOT".into(), sysroot));
            }
            if let Ok(comspec) = std::env::var("COMSPEC") {
                envs.push(("COMSPEC".into(), comspec));
            }
        }
        envs.push((
            "TMPDIR".into(),
            scratchpad_dir.to_string_lossy().into_owned(),
        ));
        envs.push(("TEMP".into(), scratchpad_dir.to_string_lossy().into_owned()));
        envs.push(("TMP".into(), scratchpad_dir.to_string_lossy().into_owned()));

        for (k, v) in &config.envs {
            envs.push((k.clone(), v.clone()));
        }
        envs.push(("DMZ_AGENT_ID".into(), agent_id.clone()));
        envs.push((
            "DMZ_AGENT_SCRATCHPAD".into(),
            scratchpad_dir.to_string_lossy().into_owned(),
        ));

        if no_network {
            envs.push(("CARGO_NET_OFFLINE".into(), "true".into()));
            envs.push(("PIP_NO_INDEX".into(), "1".into()));
            envs.push(("npm_config_offline".into(), "true".into()));
            envs.push(("GOPROXY".into(), "off".into()));
        } else if let Some(ref active_proxy) = proxy {
            let proxy_url = active_proxy.proxy_url();
            envs.push(("HTTP_PROXY".into(), proxy_url.clone()));
            envs.push(("HTTPS_PROXY".into(), proxy_url.clone()));
            envs.push(("ALL_PROXY".into(), proxy_url.clone()));
            envs.push(("http_proxy".into(), proxy_url.clone()));
            envs.push(("https_proxy".into(), proxy_url.clone()));
            envs.push(("all_proxy".into(), proxy_url));
            envs.push(("NO_PROXY".into(), "localhost,127.0.0.1".into()));
            envs.push(("no_proxy".into(), "localhost,127.0.0.1".into()));
        }

        let sandbox_cfg = SandboxConfig {
            workspace_path: scratchpad_dir.clone(),
            closure_path: scratchpad_dir.clone(),
            command: config.binary_or_script.clone(),
            args: config.args.clone(),
            envs,
            allow_network: allow_all_network,
            ipc_socket_path: None,
        };
        let policy_opts = SandboxPolicyOptions {
            isolation_mode,
            // AI agents execute untrusted code inside ephemeral scratchpads and keep strict
            // ancestor-only Seatbelt metadata rules rather than global stat/lstat visibility.
            metadata_mode: MetadataReadMode::StrictAncestors,
            proxy_addr: proxy.as_ref().map(LoopbackAllowlistProxy::local_addr),
            writable_paths: vec![scratchpad_dir.clone()],
        };

        let (std_cmd, isolation_posture) = prepare_agent_command(&sandbox_cfg, &policy_opts)
            .map_err(|e| AgentRunnerError::Platform(e.to_string()))?;

        let mut cmd = Command::from(std_cmd);
        cmd.stdin(std::process::Stdio::null()).kill_on_drop(false); // Retain lifecycle under supervisor monitor

        #[cfg(target_os = "windows")]
        let job_guard = WindowsJobGuard::create_kill_on_close()
            .map_err(|e| AgentRunnerError::Platform(e.to_string()))?;

        info!(
            agent = %agent_id,
            scratchpad = ?scratchpad_dir,
            posture = %isolation_posture,
            "Launching sandboxed agent process tree"
        );

        let mut child = cmd.spawn().map_err(AgentRunnerError::ProcessSpawn)?;

        #[cfg(target_os = "windows")]
        if let Some(raw_handle) = child.raw_handle() {
            if let Err(err) = job_guard.assign_raw_handle(raw_handle as _) {
                let _ = child.kill().await;
                return Err(AgentRunnerError::Platform(err.to_string()));
            }
        }

        // Validate positive PGID safely without unchecked narrowing cast
        #[cfg(unix)]
        let pgid = child.id().and_then(|id| match i32::try_from(id) {
            Ok(v) if v > 1 => Some(v),
            _ => None,
        });

        let (kill_tx, kill_rx) = oneshot::channel::<oneshot::Sender<()>>();
        let reaped = Arc::new(AtomicBool::new(false));

        let active_agent = ActiveAgent {
            config: config.clone(),
            sandbox,
            isolation_posture,
            _proxy: proxy,
            #[cfg(target_os = "windows")]
            _job_guard: Some(job_guard),
            kill_tx: Some(kill_tx),
            #[cfg(unix)]
            pgid,
            reaped: reaped.clone(),
        };

        // Step 4: Spawn cancel-safe event-driven watcher to monitor exit status or termination signal
        let agents_ref = Arc::clone(&self.active_agents);
        let agent_id_clone = agent_id.clone();
        let reaped_watcher = Arc::clone(&reaped);

        tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    reaped_watcher.store(true, Ordering::SeqCst);
                    let exit_desc = match status {
                        Ok(s) => format!("code {:?}", s.code()),
                        Err(e) => format!("wait error: {}", e),
                    };
                    info!(agent = %agent_id_clone, exit = %exit_desc, "Agent process exited; purging volatile scratchpad");
                    let mut guard = agents_ref.write().await;
                    if let Some(_removed_agent) = guard.remove(&agent_id_clone) {
                        info!(agent = %agent_id_clone, "AI Agent sandbox memory completely wiped");
                    }
                }
                res = kill_rx => {
                    #[cfg(unix)]
                    if let Some(pid) = pgid {
                        // Strictly guard against pgid <= 1 to avoid supervisor suicide
                        if pid > 1 && !reaped_watcher.load(Ordering::SeqCst) {
                            // SAFETY: `pid` is a validated positive process group ID (> 1)
                            // created via `cmd.process_group(0)` for this agent.
                            unsafe {
                                libc::killpg(pid, libc::SIGTERM);
                            }
                        }
                    }

                    // Cooperative wait with timeout before escalating to SIGKILL (Blueprint §4.3)
                    let timed_out = {
                        let wait_fut = child.wait();
                        tokio::pin!(wait_fut);
                        tokio::time::timeout(Duration::from_millis(250), &mut wait_fut)
                            .await
                            .is_err()
                    };

                    if timed_out {
                        warn!(agent = %agent_id_clone, "Agent did not exit within graceful timeout; escalating to SIGKILL");
                        #[cfg(unix)]
                        if let Some(pid) = pgid {
                            if pid > 1 && !reaped_watcher.load(Ordering::SeqCst) {
                                // SAFETY: Force-kill child process group on timeout.
                                unsafe {
                                    libc::killpg(pid, libc::SIGKILL);
                                }
                            }
                        }
                        #[cfg(windows)]
                        let _ = child.start_kill();

                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }

                    reaped_watcher.store(true, Ordering::SeqCst);
                    if let Ok(ack_tx) = res {
                        let _ = ack_tx.send(());
                    }
                }
            }
        });

        // Step 5: Transition state to Running and commit the reservation guard
        {
            let mut agents = self.active_agents.write().await;
            agents.insert(agent_id, AgentState::Running(Box::new(active_agent)));
        }
        guard.committed = true;

        Ok(())
    }

    /// Terminate an agent explicitly with graceful shutdown escalation
    pub async fn terminate_agent(&self, agent_id: &str) -> Result<(), AgentRunnerError> {
        let removed = {
            let mut agents = self.active_agents.write().await;
            agents.remove(agent_id)
        };

        match removed {
            Some(AgentState::Running(mut agent)) => {
                info!(agent = %agent_id, "Killing active AI agent process tree");
                if let Some(kill_tx) = agent.kill_tx.take() {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if kill_tx.send(ack_tx).is_ok() {
                        let _ = ack_rx.await;
                    }
                }
                agent.reaped.store(true, Ordering::SeqCst);
                drop(agent);
                info!(agent = %agent_id, "AI Agent sandbox memory completely wiped");
                Ok(())
            }
            Some(AgentState::Starting) => {
                info!(agent = %agent_id, "Aborting agent in Starting state");
                Ok(())
            }
            None => Err(AgentRunnerError::NotFound(agent_id.to_string())),
        }
    }

    /// Query active agent IDs (both Running and Starting)
    pub async fn list_active_agents(&self) -> Vec<String> {
        let agents = self.active_agents.read().await;
        agents.keys().cloned().collect()
    }

    /// Query active agent isolation postures for telemetry reporting.
    pub async fn list_active_agent_postures(&self) -> HashMap<String, &'static str> {
        let agents = self.active_agents.read().await;
        agents
            .iter()
            .map(|(id, state)| {
                let posture = match state {
                    AgentState::Starting => "starting",
                    AgentState::Running(agent) => agent.isolation_posture,
                };
                (id.clone(), posture)
            })
            .collect()
    }

    /// Query the volatile scratchpad path for an active agent (Law of Demeter §2.7)
    pub async fn get_scratchpad_path(&self, agent_id: &str) -> Option<PathBuf> {
        let agents = self.active_agents.read().await;
        match agents.get(agent_id) {
            Some(AgentState::Running(agent)) => Some(agent.sandbox.scratchpad_path().to_path_buf()),
            _ => None,
        }
    }

    /// Returns true if the agent is actively running
    pub async fn is_running(&self, agent_id: &str) -> bool {
        let agents = self.active_agents.read().await;
        matches!(agents.get(agent_id), Some(AgentState::Running(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_duplicate_agent_id_rejected() {
        let runner = AgentRunner::new();
        let agent_id = "agent-dup-test".to_string();

        let config = AgentConfig {
            agent_id: agent_id.clone(),
            binary_or_script: if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                "sleep".to_string()
            },
            args: if cfg!(windows) {
                vec!["/C".to_string(), "ping 127.0.0.1 -n 5".to_string()]
            } else {
                vec!["5".to_string()]
            },
            envs: HashMap::new(),
            allowed_domains: vec!["*".to_string()],
            scratchpad_bytes: 1024 * 1024,
        };

        let res1 = runner.spawn_agent(config.clone()).await;
        assert!(res1.is_ok());

        // Spawning again with identical ID must fail atomically with AlreadyRunning
        let res2 = runner.spawn_agent(config).await;
        assert!(matches!(res2, Err(AgentRunnerError::AlreadyRunning(_))));

        // Clean up
        let _ = runner.terminate_agent(&agent_id).await;
    }

    #[tokio::test]
    async fn test_terminate_nonexistent_agent_returns_not_found() {
        let runner = AgentRunner::new();
        let res = runner.terminate_agent("ghost-agent").await;
        assert!(matches!(res, Err(AgentRunnerError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_reservation_guard_rollback_on_drop() {
        let runner = AgentRunner::new();
        let agent_id = "reservation-test".to_string();

        {
            let mut agents = runner.active_agents.write().await;
            agents.insert(agent_id.clone(), AgentState::Starting);
        }

        assert_eq!(runner.list_active_agents().await, vec![agent_id.clone()]);

        // Simulating reservation guard drop before commit
        {
            let _guard = ReservationGuard {
                agent_id: agent_id.clone(),
                agents_arc: Arc::clone(&runner.active_agents),
                committed: false,
            };
            // _guard drops here without committed = true
        }

        // Active agents list must now be empty
        assert!(runner.list_active_agents().await.is_empty());
    }

    #[test]
    fn test_pgid_safety_conversion() {
        let validate_pgid = |id: u32| -> Option<i32> {
            match i32::try_from(id) {
                Ok(v) if v > 1 => Some(v),
                _ => None,
            }
        };

        // IDs <= 1 must NEVER be accepted as signal targets (prevents supervisor self-suicide)
        assert_eq!(validate_pgid(0), None);
        assert_eq!(validate_pgid(1), None);
        assert_eq!(validate_pgid(u32::MAX), None);
        assert_eq!(validate_pgid(i32::MAX as u32 + 1), None);

        // Valid positive PIDs > 1 must pass
        assert_eq!(validate_pgid(2), Some(2));
        assert_eq!(validate_pgid(12345), Some(12345));
    }

    #[tokio::test]
    async fn test_agent_posture_reporting_for_no_network_and_proxy() {
        let runner = AgentRunner::new();
        let (binary, args) = if cfg!(windows) {
            (
                "cmd.exe".to_string(),
                vec!["/C".to_string(), "ping 127.0.0.1 -n 5".to_string()],
            )
        } else {
            ("/bin/sleep".to_string(), vec!["5".to_string()])
        };

        let no_net_cfg = AgentConfig {
            agent_id: "posture-no-net".to_string(),
            binary_or_script: binary.clone(),
            args: args.clone(),
            envs: HashMap::new(),
            allowed_domains: Vec::new(),
            scratchpad_bytes: 1024 * 1024,
        };
        let proxy_cfg = AgentConfig {
            agent_id: "posture-proxy".to_string(),
            binary_or_script: binary,
            args,
            envs: HashMap::new(),
            allowed_domains: vec!["example.com".to_string()],
            scratchpad_bytes: 1024 * 1024,
        };

        let mode = if cfg!(windows) {
            SandboxIsolationMode::BestEffort
        } else {
            SandboxIsolationMode::StrictKernel
        };

        runner
            .spawn_agent_with_mode(no_net_cfg, mode)
            .await
            .expect("spawn no-network agent");
        runner
            .spawn_agent_with_mode(proxy_cfg, mode)
            .await
            .expect("spawn proxy agent");

        let postures = runner.list_active_agent_postures().await;
        if cfg!(target_os = "macos") {
            assert_eq!(
                postures.get("posture-no-net").copied(),
                Some("kernel-seatbelt-no-network")
            );
            assert_eq!(
                postures.get("posture-proxy").copied(),
                Some("kernel-seatbelt-loopback-proxy")
            );
        } else if cfg!(target_os = "linux") {
            assert_eq!(
                postures.get("posture-no-net").copied(),
                Some("kernel-netns-no-network")
            );
            assert_eq!(
                postures.get("posture-proxy").copied(),
                Some("kernel-netns-loopback-proxy")
            );
        }

        let _ = runner.terminate_agent("posture-no-net").await;
        let _ = runner.terminate_agent("posture-proxy").await;
    }
}
