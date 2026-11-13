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
#[serde(default)]
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
#[serde(default)]
pub struct WorkspaceProjectConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub members: Vec<WorkspaceMember>,
    #[serde(default)]
    pub ipc: IpcBridgeConfig,
}

/// Normalizes a workspace member path relative to `workspace_root` when possible.
///
/// Produces `"."` for identical directories, nested relative paths (`"backend"`),
/// or sibling relative paths (`"../sidecar"`), preserving unrelated prefixes verbatim.
pub fn normalize_member_path(workspace_root: &Path, target: &Path) -> PathBuf {
    let ws_abs = if workspace_root.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(workspace_root))
            .unwrap_or_else(|_| workspace_root.to_path_buf())
    } else {
        workspace_root.to_path_buf()
    };
    let ws_canon = fs::canonicalize(&ws_abs).unwrap_or_else(|_| clean_lexical_path(&ws_abs));

    let target_joined = if target.is_relative() {
        ws_canon.join(target)
    } else {
        target.to_path_buf()
    };
    let target_canon =
        fs::canonicalize(&target_joined).unwrap_or_else(|_| clean_lexical_path(&target_joined));

    if ws_canon == target_canon {
        return PathBuf::from(".");
    }

    let ws_components: Vec<std::path::Component<'_>> = ws_canon.components().collect();
    let target_components: Vec<std::path::Component<'_>> = target_canon.components().collect();

    // Never emit cross-drive relative paths on Windows
    if matches!(
        (ws_components.first(), target_components.first()),
        (
            Some(std::path::Component::Prefix(a)),
            Some(std::path::Component::Prefix(b))
        ) if a != b
    ) {
        return target_canon;
    }

    let mut common_len = 0;
    while common_len < ws_components.len()
        && common_len < target_components.len()
        && ws_components[common_len] == target_components[common_len]
    {
        common_len += 1;
    }

    if common_len == 0 {
        return target_canon;
    }

    let mut rel = PathBuf::new();
    for comp in &ws_components[common_len..] {
        if matches!(comp, std::path::Component::Normal(_)) {
            rel.push("..");
        }
    }
    for comp in &target_components[common_len..] {
        rel.push(comp.as_os_str());
    }

    if rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rel
    }
}

fn clean_lexical_path(path: &Path) -> PathBuf {
    let mut out: Vec<std::path::Component<'_>> = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(out.last(), Some(std::path::Component::Normal(_))) {
                    out.pop();
                } else if !matches!(
                    out.last(),
                    Some(std::path::Component::RootDir | std::path::Component::Prefix(_))
                ) {
                    out.push(comp);
                }
            }
            _ => out.push(comp),
        }
    }
    out.into_iter().collect()
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

    #[test]
    fn test_partial_ipc_and_minimal_workspace_manifest_defaults() -> Result<(), Box<dyn Error>> {
        let partial_ipc = r#"
[ipc]
socket_id = "api"
"#;
        let cfg = WorkspaceProjectConfig::from_toml_str(partial_ipc)?;
        assert_eq!(cfg.name, "default");
        assert!(cfg.members.is_empty());
        assert_eq!(cfg.ipc.socket_id, "api");
        assert_eq!(cfg.ipc.env_var, "DMZ_SIDECAR_SOCK");
        assert_eq!(cfg.ipc.readiness_timeout_ms, 10000);
        assert_eq!(cfg.ipc.drain_timeout_ms, 3000);

        let named_without_members = r#"
name = "minimal-app"
"#;
        let cfg2 = WorkspaceProjectConfig::from_toml_str(named_without_members)?;
        assert_eq!(cfg2.name, "minimal-app");
        assert!(cfg2.members.is_empty());
        assert_eq!(cfg2.ipc, IpcBridgeConfig::default());
        Ok(())
    }

    #[test]
    fn test_normalize_member_path_relative_layouts() -> Result<(), Box<dyn Error>> {
        let base = tempfile::tempdir()?;
        let ws = base.path().join("primary");
        let nested = ws.join("backend");
        let sibling = base.path().join("sidecar");
        fs::create_dir_all(&nested)?;
        fs::create_dir_all(&sibling)?;

        assert_eq!(normalize_member_path(&ws, &ws), PathBuf::from("."));
        assert_eq!(
            normalize_member_path(&ws, &nested),
            PathBuf::from("backend")
        );
        assert_eq!(
            normalize_member_path(&ws, &sibling),
            PathBuf::from("../sidecar")
        );
        Ok(())
    }
}
