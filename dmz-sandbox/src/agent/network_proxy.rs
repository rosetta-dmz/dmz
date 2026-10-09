use std::collections::HashSet;
use tracing::{error, info};

#[derive(Debug, Clone)]
pub struct DomainWhitelist {
    allowed_domains: HashSet<String>,
    allowed_wildcards: Vec<String>,
    allow_all: bool,
}

impl DomainWhitelist {
    /// Create a network rule engine from a list of domain strings
    pub fn new(rules: Vec<String>) -> Self {
        let mut allowed_domains = HashSet::new();
        let mut allowed_wildcards = Vec::new();
        let mut allow_all = false;

        for rule in rules {
            let trimmed = rule.trim().to_lowercase();
            if trimmed == "*" {
                allow_all = true;
            } else if trimmed.starts_with("*.") {
                allowed_wildcards.push(trimmed[2..].to_string());
            } else {
                allowed_domains.insert(trimmed);
            }
        }

        Self {
            allowed_domains,
            allowed_wildcards,
            allow_all,
        }
    }

    /// Evaluates if an outbound host connection is permitted for the AI agent
    pub fn is_allowed(&self, host: &str) -> bool {
        if self.allow_all {
            return true;
        }

        let clean_host = host.split(':').next().unwrap_or(host).trim().to_lowercase();

        // 1. Direct exact match (e.g. "api.openai.com")
        if self.allowed_domains.contains(&clean_host) {
            return true;
        }

        // 2. Wildcard suffix match (e.g. "*.github.com" matches "raw.github.com")
        for wildcard in &self.allowed_wildcards {
            if clean_host.ends_with(wildcard) && clean_host.len() > wildcard.len() {
                let prefix_len = clean_host.len() - wildcard.len();
                if clean_host.as_bytes()[prefix_len - 1] == b'.' {
                    return true;
                }
            }
        }

        false
    }

    /// Intercepts network calls and returns an error if host is blocked
    pub fn authorize_request(&self, target_host: &str) -> Result<(), String> {
        if self.is_allowed(target_host) {
            info!(host = %target_host, "Outbound network request ALLOWED by agent proxy");
            Ok(())
        } else {
            error!(host = %target_host, "Outbound network request BLOCKED by agent proxy");
            Err(format!(
                "Security Exception: Access to host '{}' is prohibited by the DMZ Network Proxy policy.",
                target_host
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_whitelist_matching() {
        let whitelist = DomainWhitelist::new(vec![
            "api.openai.com".to_string(),
            "*.github.com".to_string(),
            "crates.io".to_string(),
        ]);

        assert!(whitelist.is_allowed("api.openai.com"));
        assert!(whitelist.is_allowed("api.openai.com:443"));
        assert!(whitelist.is_allowed("raw.github.com"));
        assert!(whitelist.is_allowed("gist.github.com"));

        assert!(!whitelist.is_allowed("malicious-site.com"));
        assert!(!whitelist.is_allowed("notgithub.com"));
        assert!(!whitelist.is_allowed("openai.com"));
    }
}