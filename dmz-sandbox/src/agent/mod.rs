pub mod ephemeral_fs;
pub mod network_proxy;

use ephemeral_fs::{EphemeralError, EphemeralFs};
use network_proxy::DomainWhitelist;
use std::path::Path;

pub struct AgentSandbox {
    pub id: String,
    pub scratchpad: EphemeralFs,
    pub network_policy: DomainWhitelist,
}

impl AgentSandbox {
    /// Instantiate a complete AI agent sub-sandbox with volatile storage and network guardrails
    pub fn new(
        agent_id: impl Into<String>,
        scratchpad_size_bytes: usize,
        allowed_domains: Vec<String>,
    ) -> Result<Self, EphemeralError> {
        let id = agent_id.into();
        let scratchpad = EphemeralFs::new(&id, scratchpad_size_bytes)?;
        let network_policy = DomainWhitelist::new(allowed_domains);

        Ok(Self {
            id,
            scratchpad,
            network_policy,
        })
    }

    pub fn scratchpad_path(&self) -> &Path {
        self.scratchpad.path()
    }

    pub fn can_access_host(&self, host: &str) -> bool {
        self.network_policy.is_allowed(host)
    }
}
