pub mod ephemeral_fs;
pub mod network_proxy;

pub use ephemeral_fs::{EphemeralError, EphemeralFs};
pub use network_proxy::{DomainWhitelist, ProxyError};
use std::path::Path;

/// High-level AI agent containerless sub-sandbox combining volatile storage and network policy enforcement
#[derive(Debug)]
pub struct AgentSandbox {
    pub id: String,
    pub scratchpad: EphemeralFs,
    pub network_policy: DomainWhitelist,
}

impl AgentSandbox {
    /// Instantiate AI agent sub-sandbox with volatile storage and network guardrails
    pub fn new(
        agent_id: impl Into<String>,
        scratchpad_size_bytes: usize,
        allowed_domains: Vec<String>,
    ) -> Result<Self, EphemeralError> {
        let raw_id = agent_id.into();
        let id = if raw_id.trim().is_empty() {
            "default".to_string()
        } else {
            raw_id.trim().to_string()
        };

        if scratchpad_size_bytes == 0 {
            return Err(EphemeralError::CreationFailed(
                "Scratchpad size limit must be greater than zero bytes".into(),
            ));
        }

        let scratchpad = EphemeralFs::new(&id, scratchpad_size_bytes)?;
        let network_policy = DomainWhitelist::new(allowed_domains);

        Ok(Self {
            id,
            scratchpad,
            network_policy,
        })
    }

    /// Returns the absolute path to the agent's volatile scratchpad directory
    pub fn scratchpad_path(&self) -> &Path {
        self.scratchpad.path()
    }

    /// Evaluates whether an outbound network destination is permitted
    pub fn can_access_host(&self, host: &str) -> bool {
        self.network_policy.is_allowed(host)
    }

    /// Evaluates and authorizes an outbound network destination, returning a typed error if prohibited
    pub fn authorize_host(&self, host: &str) -> Result<(), ProxyError> {
        self.network_policy.authorize_request(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_sandbox_lifecycle_and_policies() {
        let sandbox = AgentSandbox::new(
            "test_worker",
            2 * 1024 * 1024,
            vec![
                "allowed-service.local".to_string(),
                "*.allowed-wildcard.local".to_string(),
            ],
        )
        .expect("Failed to create agent sandbox");

        assert_eq!(sandbox.id, "test_worker");
        assert!(sandbox.scratchpad_path().exists());

        // Policy verification
        assert!(sandbox.can_access_host("allowed-service.local"));
        assert!(sandbox.can_access_host("allowed-service.local:443"));
        assert!(sandbox.can_access_host("sub.allowed-wildcard.local"));
        assert!(!sandbox.can_access_host("blocked-service.local"));

        assert!(sandbox.authorize_host("allowed-service.local").is_ok());
        assert!(sandbox.authorize_host("blocked-service.local").is_err());

        let path = sandbox.scratchpad_path().to_path_buf();
        drop(sandbox);
        // Automatic cleanup verification
        assert!(!path.exists());
    }

    #[test]
    fn test_agent_sandbox_zero_size_rejection() {
        let res = AgentSandbox::new("zero_agent", 0, vec![]);
        assert!(res.is_err());
        match res.unwrap_err() {
            EphemeralError::CreationFailed(msg) => {
                assert!(msg.contains("greater than zero"));
            }
            other => panic!("Expected CreationFailed, got: {:?}", other),
        }
    }

    #[test]
    fn test_agent_sandbox_empty_id_fallback() {
        let sandbox = AgentSandbox::new("   ", 1024 * 1024, vec![])
            .expect("Failed to create agent sandbox with whitespace id");
        assert_eq!(sandbox.id, "default");
    }
}
