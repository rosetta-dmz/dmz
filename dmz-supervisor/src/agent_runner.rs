use dmz_sandbox::agent::AgentSandbox;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
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
    pub child_process: Arc<Mutex<Child>>,
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
            .current_dir(&scratchpad_dir);

        info!(agent = %agent_id, scratchpad = ?scratchpad_dir, "Launching agent process tree");

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn agent process: {}", e))?;

        let child_arc = Arc::new(Mutex::new(child));

        let active_agent = ActiveAgent {
            config: config.clone(),
            sandbox,
            child_process: child_arc.clone(),
        };

        {
            let mut agents = self.active_agents.write().await;
            agents.insert(agent_id.clone(), active_agent);
        }

        // Spawn asynchronous background watcher to monitor exit status and trigger cleanup
        let agents_ref = self.active_agents.clone();
        let agent_id_clone = agent_id.clone();

        tokio::spawn(async move {
            let mut child_guard = child_arc.lock().await;
            let _ = child_guard.wait().await;
            drop(child_guard);

            info!(agent = %agent_id_clone, "Agent process exited; purging volatile scratchpad");

            let mut guard = agents_ref.write().await;
            if let Some(_removed_agent) = guard.remove(&agent_id_clone) {
                info!(agent = %agent_id_clone, "AI Agent sandbox memory completely wiped");
            }
        });

        Ok(())
    }

    /// Terminate an agent explicitly
    pub async fn terminate_agent(&self, agent_id: &str) -> Result<(), String> {
        let mut agents = self.active_agents.write().await;
        if let Some(agent) = agents.remove(agent_id) {
            info!(agent = %agent_id, "Killing active AI agent process tree");
            let mut child = agent.child_process.lock().await;
            let _ = child.kill().await;
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
