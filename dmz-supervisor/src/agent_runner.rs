use dmz_sandbox::agent::AgentSandbox;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{oneshot, RwLock};
use tracing::info;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub agent_id: String,
    pub binary_or_script: String,
    pub args: Vec<String>,
    pub envs: HashMap<String, String>,
    pub allowed_domains: Vec<String>,
    pub scratchpad_bytes: usize,
}

pub struct ActiveAgent {
    pub config: AgentConfig,
    pub sandbox: AgentSandbox,
    kill_tx: Option<oneshot::Sender<oneshot::Sender<()>>>,
    #[cfg(unix)]
    pgid: Option<i32>,
}

impl Drop for ActiveAgent {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pgid {
            // SAFETY: `pid` is a positive process group ID created via `cmd.process_group(0)` for this agent.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
        if let Some(kill_tx) = self.kill_tx.take() {
            let (ack_tx, _) = oneshot::channel();
            let _ = kill_tx.send(ack_tx);
        }
    }
}

#[derive(Clone, Default)]
pub struct AgentRunner {
    active_agents: Arc<RwLock<HashMap<String, ActiveAgent>>>,
}

impl AgentRunner {
    pub fn new() -> Self {
        Self {
            active_agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn new AI agent wrapped in an isolated dmz-sandbox environment
    pub async fn spawn_agent(&self, config: AgentConfig) -> Result<(), String> {
        let agent_id = config.agent_id.clone();

        {
            let agents = self.active_agents.read().await;
            if agents.contains_key(&agent_id) {
                return Err(format!("Agent with ID '{}' is already running", agent_id));
            }
        }

        info!(agent = %agent_id, domains = ?config.allowed_domains, "Initializing AI agent sandbox");

        // Create native AgentSandbox (allocates EphemeralFs + DomainWhitelist proxy)
        let sandbox = AgentSandbox::new(
            &agent_id,
            config.scratchpad_bytes,
            config.allowed_domains.clone(),
        )
        .map_err(|e| format!("Failed to allocate agent sandbox: {}", e))?;

        let scratchpad_dir = sandbox.scratchpad_path().to_path_buf();

        // Prepare command and inject sandbox environment variables
        let mut cmd = Command::new(&config.binary_or_script);
        cmd.args(&config.args)
            .envs(&config.envs)
            .env("DMZ_AGENT_ID", &agent_id)
            .env("DMZ_AGENT_SCRATCHPAD", &scratchpad_dir)
            .current_dir(&scratchpad_dir)
            .kill_on_drop(true);

        #[cfg(unix)]
        cmd.process_group(0);

        info!(agent = %agent_id, scratchpad = ?scratchpad_dir, "Launching agent process tree");

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn agent process: {}", e))?;

        #[cfg(unix)]
        let pgid = child.id().map(|id| id as i32);

        let (kill_tx, kill_rx) = oneshot::channel::<oneshot::Sender<()>>();

        let active_agent = ActiveAgent {
            config: config.clone(),
            sandbox,
            kill_tx: Some(kill_tx),
            #[cfg(unix)]
            pgid,
        };

        {
            let mut agents = self.active_agents.write().await;
            agents.insert(agent_id.clone(), active_agent);
        }

        // Spawn cancel-safe event-driven watcher to monitor exit status or termination signal
        let agents_ref = self.active_agents.clone();
        let agent_id_clone = agent_id.clone();

        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {
                    info!(agent = %agent_id_clone, "Agent process exited; purging volatile scratchpad");
                    let mut guard = agents_ref.write().await;
                    if let Some(_removed_agent) = guard.remove(&agent_id_clone) {
                        info!(agent = %agent_id_clone, "AI Agent sandbox memory completely wiped");
                    }
                }
                res = kill_rx => {
                    #[cfg(unix)]
                    if let Some(pid) = pgid {
                        // SAFETY: `pid` is a positive process group ID created via `cmd.process_group(0)` for this agent.
                        unsafe {
                            libc::killpg(pid, libc::SIGTERM);
                        }
                    }
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    #[cfg(unix)]
                    if let Some(pid) = pgid {
                        // SAFETY: `pid` is a positive process group ID created via `cmd.process_group(0)` for this agent.
                        unsafe {
                            libc::killpg(pid, libc::SIGKILL);
                        }
                    }
                    if let Ok(ack_tx) = res {
                        let _ = ack_tx.send(());
                    }
                }
            }
        });

        Ok(())
    }

    /// Terminate an agent explicitly without lock contention
    pub async fn terminate_agent(&self, agent_id: &str) -> Result<(), String> {
        let removed = {
            let mut agents = self.active_agents.write().await;
            agents.remove(agent_id)
        };

        if let Some(mut agent) = removed {
            info!(agent = %agent_id, "Killing active AI agent process tree");
            if let Some(kill_tx) = agent.kill_tx.take() {
                let (ack_tx, ack_rx) = oneshot::channel();
                if kill_tx.send(ack_tx).is_ok() {
                    let _ = ack_rx.await;
                }
            }
            drop(agent);
            info!(agent = %agent_id, "AI Agent sandbox memory completely wiped");
            Ok(())
        } else {
            Err(format!("Agent '{}' not found", agent_id))
        }
    }

    /// Query active agent IDs
    pub async fn list_active_agents(&self) -> Vec<String> {
        let agents = self.active_agents.read().await;
        agents.keys().cloned().collect()
    }
}
