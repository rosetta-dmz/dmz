// Workspace configuration models, member descriptors, and serialization.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Role of a workspace member within the multi-workspace hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    #[default]
    Member,
    Primary,
    Sidecar,
    Service,
}

impl std::fmt::Display for MemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Member => write!(f, "member"),
            Self::Primary => write!(f, "primary"),
            Self::Sidecar => write!(f, "sidecar"),
            Self::Service => write!(f, "service"),
        }
    }
}

/// Metadata describing an individual workspace member.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMember {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub role: MemberRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_args: Option<Vec<String>>,
    #[serde(default)]
    pub is_static_binary: bool,
}

impl WorkspaceMember {
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>, role: MemberRole) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            role,
            ecosystem: None,
            build_command: None,
            exec_command: None,
            exec_args: None,
            is_static_binary: false,
        }
    }
}

/// IPC bridge timing and socket identification configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcBridgeConfig {
    pub socket_id: String,
    pub env_var: String,
    pub readiness_timeout_ms: u64,
    pub drain_timeout_ms: u64,
}

impl Default for IpcBridgeConfig {
    fn default() -> Self {
        Self {
            socket_id: "sidecar".to_string(),
            env_var: "DMZ_SIDECAR_SOCK".to_string(),
            readiness_timeout_ms: 10000,
            drain_timeout_ms: 3000,
        }
    }
}

/// Resolved host and virtual archive layout for a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMemberLayout {
    pub id: String,
    pub role: MemberRole,
    pub host_path: PathBuf,
    pub folder_name: String,
    pub archive_path: PathBuf,
    pub member: WorkspaceMember,
}

/// Root configuration model representing `dmz-workspace.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceProjectConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub members: Vec<WorkspaceMember>,
    #[serde(default)]
    pub ipc: IpcBridgeConfig,
}

impl Default for WorkspaceProjectConfig {
    fn default() -> Self {
        Self::new("default", Vec::new())
    }
}

impl WorkspaceProjectConfig {
    pub fn new(name: impl Into<String>, members: Vec<WorkspaceMember>) -> Self {
        Self {
            name: name.into(),
            version: None,
            members,
            ipc: IpcBridgeConfig::default(),
        }
    }

    pub fn primary_member(&self) -> Option<&WorkspaceMember> {
        self.members
            .iter()
            .find(|m| m.role == MemberRole::Primary)
            .or_else(|| self.members.first())
    }

    pub fn sidecars(&self) -> Vec<&WorkspaceMember> {
        let sc: Vec<&WorkspaceMember> = self
            .members
            .iter()
            .filter(|m| m.role == MemberRole::Sidecar || m.role == MemberRole::Service)
            .collect();
        if sc.is_empty() && self.members.len() > 1 {
            self.members.iter().skip(1).collect()
        } else {
            sc
        }
    }

    pub fn find_member(&self, id: &str) -> Option<&WorkspaceMember> {
        self.members.iter().find(|m| m.id == id)
    }

    pub fn from_toml_str(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn load_from_file(path: &Path) -> Result<Self, anyhow::Error> {
        let content = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
        let config = Self::from_toml_str(&content).map_err(|e| {
            anyhow::anyhow!("Invalid workspace manifest in {}: {}", path.display(), e)
        })?;
        Ok(config)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), anyhow::Error> {
        let content = self
            .to_toml_string()
            .map_err(|e| anyhow::anyhow!("Failed to serialize workspace config: {}", e))?;
        super::atomic::atomic_write_file(path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::path::PathBuf;

    #[test]
    fn test_workspace_manifest_roundtrip() -> Result<(), Box<dyn Error>> {
        let toml_data = r#"
name = "hybrid-app"
version = "1.0.0"

[[members]]
id = "web-client"
path = "./frontend"
role = "primary"
ecosystem = "node"

[[members]]
id = "ai-engine"
path = "../sidecar-engine"
role = "sidecar"
ecosystem = "python"
is_static_binary = false
exec_command = "python3"
exec_args = ["server.py"]

[ipc]
socket_id = "ai-engine"
env_var = "DMZ_AI_SOCK"
readiness_timeout_ms = 8000
drain_timeout_ms = 2500
"#;

        let config = WorkspaceProjectConfig::from_toml_str(toml_data)?;
        assert_eq!(config.name, "hybrid-app");
        assert_eq!(config.version.as_deref(), Some("1.0.0"));
        assert_eq!(config.members.len(), 2);

        let primary = config.primary_member().ok_or("missing primary member")?;
        assert_eq!(primary.id, "web-client");
        assert_eq!(primary.role, MemberRole::Primary);
        assert_eq!(
            primary.canonical_virtual_path(),
            PathBuf::from("workspaces/web-client")
        );

        let sidecars = config.sidecars();
        assert_eq!(sidecars.len(), 1);
        assert_eq!(sidecars[0].id, "ai-engine");
        assert_eq!(
            sidecars[0].canonical_virtual_path(),
            PathBuf::from("workspaces/ai-engine")
        );

        assert_eq!(config.ipc.env_var, "DMZ_AI_SOCK");
        assert_eq!(config.ipc.readiness_timeout_ms, 8000);
        Ok(())
    }

    #[test]
    fn test_workspace_config_file_roundtrip() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("dmz-workspace.toml");
        let config = WorkspaceProjectConfig::new("roundtrip-app", Vec::new());
        config.save_to_file(&path)?;
        let loaded = WorkspaceProjectConfig::load_from_file(&path)?;
        assert_eq!(loaded.name, "roundtrip-app");
        Ok(())
    }
}
