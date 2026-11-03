use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceProjectConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub members: Vec<WorkspaceMember>,
    #[serde(default)]
    pub ipc: IpcBridgeConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    Primary,
    Sidecar,
    Service,
}

impl std::fmt::Display for MemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Primary => write!(f, "primary"),
            Self::Sidecar => write!(f, "sidecar"),
            Self::Service => write!(f, "service"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMember {
    pub id: String,
    pub path: PathBuf,
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

    /// Maps workspace member to its canonical virtual sandbox/archive mount path
    /// (the Runfiles pattern: decouples from host sibling paths like `../sidecar`).
    pub fn canonical_virtual_path(&self) -> PathBuf {
        match self.role {
            MemberRole::Primary => PathBuf::from("workspaces").join("primary"),
            MemberRole::Sidecar | MemberRole::Service => {
                PathBuf::from("workspaces").join("sidecars").join(&self.id)
            }
        }
    }
}

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
        self.members.iter().find(|m| m.role == MemberRole::Primary)
    }

    pub fn sidecars(&self) -> Vec<&WorkspaceMember> {
        self.members
            .iter()
            .filter(|m| m.role == MemberRole::Sidecar)
            .collect()
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
        fs::write(path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
        Ok(())
    }

    /// Searches for `dmz-workspace.toml` starting in `start_dir` and traversing parents.
    pub fn discover_workspace_manifest(start_dir: &Path) -> Option<(Self, PathBuf)> {
        let mut curr = if start_dir.is_file() {
            start_dir.parent()?.to_path_buf()
        } else {
            start_dir.to_path_buf()
        };

        loop {
            let candidate = curr.join("dmz-workspace.toml");
            if candidate.is_file() {
                if let Ok(config) = Self::load_from_file(&candidate) {
                    return Some((config, candidate));
                }
            }

            if !curr.pop() {
                break;
            }
        }

        None
    }
}

/// Generates a POSIX-safe short socket path to prevent sockaddr_un.sun_path overflow (104 bytes on macOS/BSD).
pub fn safe_socket_path(workspace: &Path, member_id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let canonical = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    hasher.update(canonical.to_string_lossy().as_bytes());
    let hash = &format!("{:x}", hasher.finalize())[..10];

    #[cfg(unix)]
    {
        std::env::temp_dir().join(format!("dmz_{}_{}.sock", hash, member_id))
    }

    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\dmz_{}_{}", hash, member_id))
    }
}

/// Automatically removes stale socket files from previous unclean shutdowns.
pub fn cleanup_stale_socket(socket_path: &Path) -> Result<bool, std::io::Error> {
    if socket_path.exists() {
        fs::remove_file(socket_path)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_manifest_roundtrip() {
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

        let config = WorkspaceProjectConfig::from_toml_str(toml_data).unwrap();
        assert_eq!(config.name, "hybrid-app");
        assert_eq!(config.version.as_deref(), Some("1.0.0"));
        assert_eq!(config.members.len(), 2);

        let primary = config.primary_member().unwrap();
        assert_eq!(primary.id, "web-client");
        assert_eq!(primary.role, MemberRole::Primary);
        assert_eq!(
            primary.canonical_virtual_path(),
            PathBuf::from("workspaces/primary")
        );

        let sidecars = config.sidecars();
        assert_eq!(sidecars.len(), 1);
        assert_eq!(sidecars[0].id, "ai-engine");
        assert_eq!(
            sidecars[0].canonical_virtual_path(),
            PathBuf::from("workspaces/sidecars/ai-engine")
        );

        assert_eq!(config.ipc.env_var, "DMZ_AI_SOCK");
        assert_eq!(config.ipc.readiness_timeout_ms, 8000);
    }

    #[test]
    fn test_safe_socket_path_length_guarantee() {
        let long_path = PathBuf::from(
            "/Users/some_very_long_developer_username/Development/EnterpriseProjects/ClientServices/Subsystem/Application/WorkspaceDirectory/DeeplyNestedComponent",
        );
        let safe_sock = safe_socket_path(&long_path, "sidecar");
        let path_str = safe_sock.to_string_lossy();

        #[cfg(unix)]
        {
            // POSIX sockaddr_un.sun_path hard limits: 104 on macOS, 108 on Linux
            assert!(
                path_str.len() < 104,
                "Safe socket path '{}' exceeds 104 characters ({})",
                path_str,
                path_str.len()
            );
            assert!(path_str.ends_with("_sidecar.sock"));
        }

        #[cfg(windows)]
        {
            assert!(path_str.starts_with(r"\\.\pipe\dmz_"));
        }
    }
}
