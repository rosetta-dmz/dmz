use std::collections::HashSet;

pub struct NetworkProxy {
    allowed_domains: HashSet<String>,
    allowed_wildcards: Vec<String>,
    allow_all: bool,
}

impl NetworkProxy {
    pub fn new(whitelisted_domains: Vec<String>) -> Self {
        let mut allowed_domains = HashSet::new();
        let mut allowed_wildcards = Vec::new();
        let mut allow_all = false;

        for rule in whitelisted_domains {
            let trimmed = rule.trim().to_lowercase();
            if trimmed == "*" {
                allow_all = true;
            } else if let Some(wildcard) = trimmed.strip_prefix("*.") {
                allowed_wildcards.push(wildcard.to_string());
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

    pub fn is_allowed(&self, domain: &str) -> bool {
        if self.allow_all {
            return true;
        }

        let clean_host = domain
            .split(':')
            .next()
            .unwrap_or(domain)
            .trim()
            .to_lowercase();

        if self.allowed_domains.contains(&clean_host) {
            return true;
        }

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

    pub fn audit_request(&self, uri: &str) -> Result<(), String> {
        let domain = if uri.contains("://") {
            uri.split("://")
                .nth(1)
                .and_then(|after| after.split('/').next())
                .unwrap_or(uri)
        } else if let Some(slash_idx) = uri.find('/') {
            &uri[..slash_idx]
        } else {
            uri
        };

        if self.is_allowed(domain) {
            Ok(())
        } else {
            Err(format!("Access denied to unauthorized domain: {}", domain))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supervisor_proxy_ports_and_wildcards() {
        let proxy = NetworkProxy::new(vec![
            "api.example.com".to_string(),
            "*.internal.net".to_string(),
        ]);

        assert!(proxy.audit_request("https://api.example.com:8443/v1/resource").is_ok());
        assert!(proxy.audit_request("http://db.internal.net:5432").is_ok());
        assert!(proxy.audit_request("api.example.com").is_ok());
        assert!(proxy.audit_request("https://malicious.org/data").is_err());
    }
}
