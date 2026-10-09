use std::collections::HashSet;

pub struct NetworkProxy {
    allowed_domains: HashSet<String>,
}

impl NetworkProxy {
    pub fn new(whitelisted_domains: Vec<String>) -> Self {
        Self {
            allowed_domains: whitelisted_domains.into_iter().collect(),
        }
    }

    pub fn is_allowed(&self, domain: &str) -> bool {
        self.allowed_domains.contains(domain) || self.allowed_domains.contains("*")
    }

    pub fn audit_request(&self, uri: &str) -> Result<(), String> {
        // Extract host or domain from URI
        if let Some(domain) = uri.split('/').nth(2) {
            if self.is_allowed(domain) {
                Ok(())
            } else {
                Err(format!("Access denied to unauthorized domain: {}", domain))
            }
        } else {
            Err("Invalid request URI format".into())
        }
    }
}