use dmz_sandbox::agent::AgentSandbox;
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

    /// Spawn new AI agent wrapped in an isolated dmz-sandbox environment
    pub async fn spawn_agent(&self, config: AgentConfig) -> Result<(), AgentRunnerError> {
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

        info!(agent = %agent_id, domains = ?config.allowed_domains, "Initializing AI agent sandbox");

        // Step 2: Create native AgentSandbox (allocates EphemeralFs + DomainWhitelist proxy)
        // Executed outside the active_agents lock to eliminate thread pool starvation
        let sandbox = AgentSandbox::new(
            &agent_id,
            config.scratchpad_bytes,
            config.allowed_domains.clone(),
        )
        .map_err(|e| AgentRunnerError::SandboxAllocation(e.to_string()))?;

        let scratchpad_dir = sandbox.scratchpad_path().to_path_buf();

        // Step 3: Prepare command with defense-in-depth environment sanitization
        let mut cmd = Command::new(&config.binary_or_script);
        cmd.args(&config.args).env_clear(); // Defense-in-depth: scrub supervisor host secrets/tokens

        // Preserve benign host system execution paths
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(sysroot) = std::env::var("SYSTEMROOT") {
                cmd.env("SYSTEMROOT", sysroot);
            }
            if let Ok(comspec) = std::env::var("COMSPEC") {
                cmd.env("COMSPEC", comspec);
            }
        }
        if let Ok(tmp) = std::env::var("TMPDIR") {
            cmd.env("TMPDIR", tmp);
        }

        // Inject agent environment variables and scratchpad paths
        cmd.envs(&config.envs)
            .env("DMZ_AGENT_ID", &agent_id)
            .env("DMZ_AGENT_SCRATCHPAD", &scratchpad_dir)
            .current_dir(&scratchpad_dir)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(false); // Retain lifecycle under supervisor monitor

        // Enforce network isolation: toolchain offline flags
        let allow_all_network = config.allowed_domains.iter().any(|d| d.trim() == "*");
        if !allow_all_network {
            cmd.env("CARGO_NET_OFFLINE", "true")
                .env("PIP_NO_INDEX", "1")
                .env("npm_config_offline", "true")
                .env("GOPROXY", "off");
        }

        #[cfg(unix)]
        cmd.process_group(0);

        info!(agent = %agent_id, scratchpad = ?scratchpad_dir, "Launching agent process tree");

        let mut child = cmd.spawn().map_err(AgentRunnerError::ProcessSpawn)?;

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
}
