use std::collections::HashSet;
use std::fmt;
use tracing::{error, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyError {
    AccessDenied { host: String },
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessDenied { host } => write!(
                f,
                "Security Exception: Access to host '{}' is prohibited by the DMZ Network Proxy policy.",
                host
            ),
        }
    }
}

impl std::error::Error for ProxyError {}

#[derive(Debug, Clone)]
pub struct DomainWhitelist {
    allowed_domains: HashSet<String>,
    allowed_wildcards: Vec<String>,
    allow_all: bool,
}

impl DomainWhitelist {
    /// Create network rule engine from list of domain strings
    pub fn new(rules: Vec<String>) -> Self {
        let mut allowed_domains = HashSet::new();
        let mut allowed_wildcards = Vec::new();
        let mut allow_all = false;

        for rule in rules {
            let trimmed = rule.trim().to_lowercase();
            // Remove trailing dot if present
            let normalized = trimmed.strip_suffix('.').unwrap_or(&trimmed);

            if normalized == "*" {
                allow_all = true;
            } else if let Some(wildcard) = normalized.strip_prefix("*.") {
                allowed_wildcards.push(wildcard.to_string());
            } else if let Some(wildcard) = normalized.strip_prefix('.') {
                allowed_wildcards.push(wildcard.to_string());
            } else {
                allowed_domains.insert(normalized.to_string());
            }
        }

        Self {
            allowed_domains,
            allowed_wildcards,
            allow_all,
        }
    }

    /// Evaluate whether an outbound host connection is permitted for the AI agent
    pub fn is_allowed(&self, host: &str) -> bool {
        if self.allow_all {
            return true;
        }

        let clean_host = Self::normalize_host(host);

        // Direct exact match (e.g. "api.openai.com")
        if self.allowed_domains.contains(&clean_host) {
            return true;
        }

        // Wildcard suffix match (e.g. "*.github.com" matches "raw.github.com")
        for wildcard in &self.allowed_wildcards {
            if clean_host.ends_with(wildcard) && clean_host.len() > wildcard.len() {
                let prefix_len = clean_host.len() - wildcard.len();
                // Safe slice indexing using .get() to prevent out-of-bounds panics
                if clean_host.as_bytes().get(prefix_len - 1) == Some(&b'.') {
                    return true;
                }
            }
        }

        false
    }

    /// Normalizes host strings, stripping schemes, user credentials, ports, and paths
    fn normalize_host(raw_host: &str) -> String {
        let trimmed = raw_host.trim();

        // Strip URI scheme if present
        let stripped_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);

        // Strip path and query components if present
        let authority = stripped_scheme.split('/').next().unwrap_or(stripped_scheme);

        // Strip userinfo (user:pass@) if present
        let host_part = authority.split('@').next_back().unwrap_or(authority);

        let clean = if let Some(stripped) = host_part.strip_prefix('[') {
            // IPv6 with brackets: e.g. "[::1]:8080" or "[::1]"
            stripped
                .split(']')
                .next()
                .unwrap_or(stripped)
                .trim()
                .to_lowercase()
        } else if host_part.matches(':').count() == 1 {
            // Standard IPv4 or hostname with port: e.g. "api.openai.com:443" or "127.0.0.1:80"
            host_part
                .split(':')
                .next()
                .unwrap_or(host_part)
                .trim()
                .to_lowercase()
        } else {
            // Bare hostname, bare IPv4, or bare IPv6 without brackets
            host_part.trim().to_lowercase()
        };

        // Strip trailing root dot (FQDN root e.g. "crates.io.")
        clean.strip_suffix('.').unwrap_or(&clean).to_string()
    }

    /// Intercept network calls and returns a typed error when host is blocked
    pub fn authorize_request(&self, target_host: &str) -> Result<(), ProxyError> {
        if self.is_allowed(target_host) {
            info!(host = %target_host, "Outbound network request ALLOWED by agent proxy");
            Ok(())
        } else {
            error!(host = %target_host, "Outbound network request BLOCKED by agent proxy");
            Err(ProxyError::AccessDenied {
                host: target_host.to_string(),
            })
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

        assert!(whitelist.is_allowed(""));
        assert!(whitelist.is_allowed(""));
        assert!(whitelist.is_allowed(""));
        assert!(whitelist.is_allowed(""));

        assert!(!whitelist.is_allowed(""));
        assert!(!whitelist.is_allowed(""));
        assert!(!whitelist.is_allowed(""));
    }

    #[test]
    fn test_authorize_request_typed_error() {
        let whitelist = DomainWhitelist::new(vec!["".to_string()]);

        assert!(whitelist.authorize_request("").is_ok());

        let err = whitelist.authorize_request("").unwrap_err();
        assert_eq!(
            err,
            ProxyError::AccessDenied {
                host: "".to_string()
            }
        );
        assert!(err.to_string().contains("Security Exception"));
    }

    #[test]
    fn test_url_scheme_and_path_normalization() {
        let whitelist = DomainWhitelist::new(vec!["".to_string(), "".to_string()]);

        assert!(whitelist.is_allowed(""));
        assert!(whitelist.is_allowed(""));
        assert!(whitelist.is_allowed(""));
        assert!(!whitelist.is_allowed(""));
    }
}
