use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMemberLayout {
    pub id: String,
    pub role: MemberRole,
    pub host_path: PathBuf,
    pub folder_name: String,
    pub archive_path: PathBuf,
    pub member: WorkspaceMember,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceProjectConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub members: Vec<WorkspaceMember>,
    #[serde(default)]
    pub ipc: IpcBridgeConfig,
}

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

    /// Maps workspace member to its canonical virtual sandbox/archive mount path
    /// (`workspaces/<id>`, decoupling from host sibling paths like `../sidecar`).
    pub fn canonical_virtual_path(&self) -> PathBuf {
        let folder = if self.id.is_empty() || self.id == "default" {
            match self.role {
                MemberRole::Primary => "primary",
                MemberRole::Sidecar | MemberRole::Service => "sidecar",
                MemberRole::Member => "member",
            }
        } else {
            self.id.as_str()
        };
        PathBuf::from(format!("workspaces/{}", folder))
    }

    /// Detects the default command, arguments, and ecosystem for a workspace member directory
    /// strictly from standard project manifests and entrypoints.
    pub fn detect_default_command(
        member_dir: &Path,
        workspace_root: &Path,
    ) -> Option<(String, Vec<String>, &'static str)> {
        let pkg_json_path = member_dir.join("package.json");
        let cargo_toml_path = member_dir.join("Cargo.toml");
        let tauri_cargo_path = member_dir.join("src-tauri/Cargo.toml");
        let py_req_path = member_dir.join("requirements.txt");
        let pyproject_path = member_dir.join("pyproject.toml");
        let setup_py_path = member_dir.join("setup.py");
        let pipfile_path = member_dir.join("Pipfile");
        let go_mod_path = member_dir.join("go.mod");

        // 1. Node / TypeScript / Tauri (package.json)
        if pkg_json_path.is_file() {
            let pm = if member_dir.join("pnpm-lock.yaml").exists()
                || workspace_root.join("pnpm-lock.yaml").exists()
                || member_dir.join("pnpm-workspace.yaml").exists()
                || member_dir.join("node_modules/.pnpm").is_dir()
            {
                "pnpm".to_string()
            } else if member_dir.join("yarn.lock").exists()
                || workspace_root.join("yarn.lock").exists()
            {
                "yarn".to_string()
            } else if member_dir.join("bun.lockb").exists() || member_dir.join("bun.lock").exists()
            {
                "bun".to_string()
            } else {
                "npm".to_string()
            };

            if let Ok(content) = fs::read_to_string(&pkg_json_path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(sc) = val.get("scripts").and_then(|s| s.as_object()) {
                        if sc.contains_key("tauri") {
                            return Some((
                                pm,
                                vec!["run".to_string(), "tauri".to_string(), "dev".to_string()],
                                "node",
                            ));
                        } else if sc.contains_key("dev") {
                            return Some((pm, vec!["run".to_string(), "dev".to_string()], "node"));
                        } else if sc.contains_key("start") {
                            return Some((
                                pm,
                                vec!["run".to_string(), "start".to_string()],
                                "node",
                            ));
                        } else if sc.contains_key("serve") {
                            return Some((
                                pm,
                                vec!["run".to_string(), "serve".to_string()],
                                "node",
                            ));
                        }
                    }

                    if let Some(main_file) = val.get("main").and_then(|m| m.as_str()) {
                        if member_dir.join(main_file).is_file() {
                            return Some(("node".to_string(), vec![main_file.to_string()], "node"));
                        }
                    }
                }
            }

            for candidate in &[
                "index.js",
                "server.js",
                "main.js",
                "app.js",
                "src/index.js",
                "src/server.js",
            ] {
                if member_dir.join(candidate).is_file() {
                    return Some(("node".to_string(), vec![candidate.to_string()], "node"));
                }
            }
        }

        // 2. Rust / Cargo (Cargo.toml or src-tauri/Cargo.toml)
        if tauri_cargo_path.is_file() || cargo_toml_path.is_file() {
            return Some(("cargo".to_string(), vec!["run".to_string()], "rust"));
        }

        // 3. Python (pyproject.toml, requirements.txt, setup.py, Pipfile)
        if py_req_path.is_file()
            || pyproject_path.is_file()
            || setup_py_path.is_file()
            || pipfile_path.is_file()
        {
            let py_cmd = if cfg!(windows) {
                "python".to_string()
            } else {
                "python3".to_string()
            };

            let candidates = [
                "server.py",
                "main.py",
                "app.py",
                "backend.py",
                "run.py",
                "manage.py",
                "src/server.py",
                "src/main.py",
                "src/app.py",
            ];
            for candidate in &candidates {
                if member_dir.join(candidate).is_file() {
                    return Some((py_cmd, vec![candidate.to_string()], "python"));
                }
            }

            // Check pyproject.toml [project.scripts] / [tool.poetry.scripts]
            if pyproject_path.is_file() {
                if let Ok(content) = fs::read_to_string(&pyproject_path) {
                    if let Ok(parsed) = content.parse::<toml::Value>() {
                        let scripts_table = parsed
                            .get("project")
                            .and_then(|p| p.get("scripts"))
                            .and_then(|s| s.as_table())
                            .or_else(|| {
                                parsed
                                    .get("tool")
                                    .and_then(|t| t.get("poetry"))
                                    .and_then(|p| p.get("scripts"))
                                    .and_then(|s| s.as_table())
                            });
                        if let Some(scripts) = scripts_table {
                            for (_script_name, target_val) in scripts {
                                if let Some(target_str) = target_val.as_str() {
                                    if let Some((mod_name, fn_name)) = target_str.split_once(':') {
                                        let mod_name = mod_name.trim();
                                        let fn_name = fn_name.trim();
                                        if !mod_name.is_empty() && !fn_name.is_empty() {
                                            return Some((
                                                py_cmd,
                                                vec![
                                                    "-c".to_string(),
                                                    format!(
                                                        "import sys, {}; sys.exit({}.{}())",
                                                        mod_name, mod_name, fn_name
                                                    ),
                                                ],
                                                "python",
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Check 1-level package subdirectories for main.py / server.py / app.py
            if let Ok(entries) = fs::read_dir(member_dir) {
                let mut subdirs: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.is_dir()
                            && p.file_name().and_then(|n| n.to_str()).is_some_and(|name| {
                                !name.starts_with('.')
                                    && !matches!(
                                        name,
                                        "venv"
                                            | "node_modules"
                                            | "target"
                                            | "dist"
                                            | "build"
                                            | "tests"
                                            | "test"
                                            | "__pycache__"
                                    )
                            })
                    })
                    .collect();
                subdirs.sort();
                for sub in subdirs {
                    if let Some(sub_name) = sub.file_name().and_then(|n| n.to_str()) {
                        for entry_script in &["main.py", "server.py", "app.py"] {
                            if sub.join(entry_script).is_file() {
                                return Some((
                                    py_cmd,
                                    vec![format!("{}/{}", sub_name, entry_script)],
                                    "python",
                                ));
                            }
                        }
                    }
                }
            }
        }

        // 4. Go (go.mod)
        if go_mod_path.is_file() {
            return Some((
                "go".to_string(),
                vec!["run".to_string(), ".".to_string()],
                "go",
            ));
        }

        None
    }

    /// Resolves the executable command and arguments for this workspace member, failing fast with an
    /// actionable diagnostic error if no command is configured or detectable.
    pub fn resolve_command(
        &self,
        member_dir: &Path,
        workspace_root: &Path,
    ) -> Result<(String, Vec<String>), String> {
        if let Some(ref cmd) = self.exec_command {
            let trimmed = cmd.trim();
            if !trimmed.is_empty() {
                return Ok((
                    trimmed.to_string(),
                    self.exec_args.clone().unwrap_or_default(),
                ));
            }
        }

        if let Some((detected_cmd, detected_args, _eco)) =
            Self::detect_default_command(member_dir, workspace_root)
        {
            let args = self.exec_args.clone().unwrap_or(detected_args);
            return Ok((detected_cmd, args));
        }

        Err(format!(
            "Unable to determine execution command for workspace member '{}' at {:?}. Specify `exec_command` and `exec_args` in dmz-workspace.toml.",
            self.id, member_dir
        ))
    }

    /// Populates `ecosystem`, `exec_command`, and `exec_args` if not already set and detectable on disk.
    pub fn populate_detected_defaults(&mut self, member_dir: &Path, workspace_root: &Path) {
        let member_dir = fs::canonicalize(member_dir).unwrap_or_else(|_| member_dir.to_path_buf());
        if !member_dir.is_dir() {
            return;
        }

        if let Some((cmd, args, eco)) = Self::detect_default_command(&member_dir, workspace_root) {
            if self.ecosystem.is_none() {
                self.ecosystem = Some(eco.to_string());
            }
            if self.exec_command.is_none() {
                self.exec_command = Some(cmd);
            }
            if self.exec_args.is_none() && !args.is_empty() {
                self.exec_args = Some(args);
            }
        } else if self.ecosystem.is_none() {
            if member_dir.join("package.json").is_file() {
                self.ecosystem = Some("node".to_string());
            } else if member_dir.join("Cargo.toml").is_file()
                || member_dir.join("src-tauri/Cargo.toml").is_file()
            {
                self.ecosystem = Some("rust".to_string());
            } else if member_dir.join("pyproject.toml").is_file()
                || member_dir.join("requirements.txt").is_file()
            {
                self.ecosystem = Some("python".to_string());
            } else if member_dir.join("go.mod").is_file() {
                self.ecosystem = Some("go".to_string());
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

    /// Resolves canonical host directories and normalized `workspaces/<folder>` names for all members.
    pub fn resolve_member_layouts(
        &self,
        workspace_root: &Path,
        manifest_path: Option<&Path>,
    ) -> Vec<ResolvedMemberLayout> {
        let primary_raw_name = workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .unwrap_or("primary")
            .to_string();

        let base_dir = manifest_path
            .and_then(|p| p.parent())
            .unwrap_or(workspace_root);

        let mut layouts = Vec::with_capacity(self.members.len());

        for member in &self.members {
            let (host_path, folder_name) = if member.role == MemberRole::Primary
                && (member.path == Path::new(".") || member.path.as_os_str().is_empty())
            {
                let folder =
                    if !member.id.is_empty() && member.id != "primary" && member.id != "default" {
                        member.id.clone()
                    } else {
                        primary_raw_name.clone()
                    };
                (workspace_root.to_path_buf(), folder)
            } else {
                let raw_host = if member.path.is_relative() {
                    base_dir.join(&member.path)
                } else {
                    member.path.clone()
                };
                let host_path = fs::canonicalize(&raw_host).unwrap_or(raw_host);
                let raw_name = host_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or(&member.id)
                    .to_string();

                let folder = if !member.id.is_empty()
                    && !matches!(
                        member.id.as_str(),
                        "primary" | "sidecar" | "service" | "default"
                    ) {
                    member.id.clone()
                } else {
                    raw_name
                };
                (host_path, folder)
            };

            layouts.push(ResolvedMemberLayout {
                id: member.id.clone(),
                role: member.role,
                host_path,
                archive_path: PathBuf::from(format!("workspaces/{}", folder_name)),
                folder_name,
                member: member.clone(),
            });
        }

        // Disambiguate if any non-primary member resolves to the same folder name as the primary member
        let primary_id = self.primary_member().map(|m| m.id.clone());
        let primary_folder = layouts
            .iter()
            .find(|l| Some(&l.id) == primary_id.as_ref())
            .map(|l| l.folder_name.clone())
            .unwrap_or(primary_raw_name);

        for layout in &mut layouts {
            if Some(&layout.id) != primary_id.as_ref() && layout.folder_name == primary_folder {
                layout.folder_name = format!("{}-sidecar", layout.folder_name);
                layout.archive_path = PathBuf::from(format!("workspaces/{}", layout.folder_name));
            }
        }

        layouts
    }

    /// Builds a hermetic runtime environment variable map for a workspace member, injecting:
    /// - Bundled toolchains (`.dmz/toolchains/*`) via `ToolchainEnv::inject_toolchain_paths`
    /// - Member and root `node_modules/.bin` and `NODE_PATH`
    /// - Member and root Python virtual environments (`.venv`, `.dmz/venv` -> `VIRTUAL_ENV`, `PYTHONPATH`)
    /// - IPC socket variables (`ipc.env_var` and `DMZ_SIDECAR_SOCK`)
    /// - Offline network isolation environment variables when `!allow_network`
    pub fn build_member_env(
        &self,
        workspace_root: &Path,
        member_dir: &Path,
        sock_path: &Path,
        allow_network: bool,
    ) -> HashMap<String, String> {
        let mut env_map: HashMap<String, String> = std::env::vars().collect();
        let separator = if cfg!(windows) { ";" } else { ":" };

        // 1. Delegate bundled toolchain injection to single source of truth
        crate::toolchain::ToolchainEnv::inject_toolchain_paths(workspace_root, &mut env_map);
        if member_dir != workspace_root {
            crate::toolchain::ToolchainEnv::inject_toolchain_paths(member_dir, &mut env_map);
        }

        // 2. Prepend member and root node_modules/.bin to PATH and configure NODE_PATH
        let mut current_path = env_map
            .get("PATH")
            .cloned()
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());

        for base in [member_dir, workspace_root] {
            let node_bin = base.join("node_modules/.bin");
            if node_bin.is_dir() {
                let nb_str = node_bin.to_string_lossy().to_string();
                if !current_path.contains(&nb_str) {
                    current_path = if current_path.is_empty() {
                        nb_str
                    } else {
                        format!("{}{}{}", nb_str, separator, current_path)
                    };
                }
            }
        }

        let member_nm = member_dir.join("node_modules");
        if member_nm.is_dir() {
            let nm_str = member_nm.to_string_lossy().to_string();
            let existing = env_map.get("NODE_PATH").cloned().unwrap_or_default();
            env_map.insert(
                "NODE_PATH".to_string(),
                if existing.is_empty() {
                    nm_str
                } else {
                    format!("{}{}{}", nm_str, separator, existing)
                },
            );
        }

        // 3. Configure Python .venv / .dmz/venv (VIRTUAL_ENV, PATH, and PYTHONPATH)
        let venv_candidates = [
            member_dir.join(".venv"),
            member_dir.join(".dmz/venv"),
            workspace_root.join(".venv"),
            workspace_root.join(".dmz/venv"),
        ];
        if let Some(venv) = venv_candidates.iter().find(|p| p.is_dir()) {
            env_map.insert(
                "VIRTUAL_ENV".to_string(),
                venv.to_string_lossy().to_string(),
            );
            let venv_bin = if cfg!(windows) {
                venv.join("Scripts")
            } else {
                venv.join("bin")
            };
            if venv_bin.is_dir() {
                let vb_str = venv_bin.to_string_lossy().to_string();
                if !current_path.contains(&vb_str) {
                    current_path = format!("{}{}{}", vb_str, separator, current_path);
                }
            }

            let mut site_packages = None;
            let lib_dir = venv.join("lib");
            if lib_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&lib_dir) {
                    for entry in entries.flatten() {
                        let sp = entry.path().join("site-packages");
                        if sp.is_dir() {
                            site_packages = Some(sp);
                            break;
                        }
                    }
                }
            }
            if site_packages.is_none() {
                let win_sp = venv.join("Lib/site-packages");
                if win_sp.is_dir() {
                    site_packages = Some(win_sp);
                }
            }
            if let Some(sp) = site_packages {
                let sp_str = sp.to_string_lossy().to_string();
                let existing_py = env_map.get("PYTHONPATH").cloned().unwrap_or_default();
                env_map.insert(
                    "PYTHONPATH".to_string(),
                    if existing_py.is_empty() {
                        sp_str
                    } else {
                        format!("{}{}{}", sp_str, separator, existing_py)
                    },
                );
            }
        }

        env_map.insert("PATH".to_string(), current_path);
        env_map.remove("PYTHONHOME");
        env_map.insert(
            "DMZ_WORKSPACE".to_string(),
            workspace_root.display().to_string(),
        );
        env_map.insert(
            "CARGO_HOME".to_string(),
            member_dir.join(".cargo").display().to_string(),
        );

        // 4. Inject IPC socket environment variables when a socket path is provided
        if !sock_path.as_os_str().is_empty() {
            let sock_str = sock_path.to_string_lossy().to_string();
            env_map.insert(self.ipc.env_var.clone(), sock_str.clone());
            env_map.insert("DMZ_SIDECAR_SOCK".to_string(), sock_str);
        }

        // 5. Offline enforcement variables when network is disallowed
        if !allow_network {
            env_map.insert("CARGO_NET_OFFLINE".to_string(), "true".to_string());
            env_map.insert("PIP_NO_INDEX".to_string(), "1".to_string());
            env_map.insert("GOPROXY".to_string(), "off".to_string());
            env_map.insert("npm_config_offline".to_string(), "true".to_string());
            env_map.insert("YARN_OFFLINE".to_string(), "true".to_string());
            env_map.insert("PNPM_OFFLINE".to_string(), "true".to_string());
            env_map.insert("COMPOSER_DISABLE_NETWORK".to_string(), "1".to_string());
        }

        env_map
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
    #[cfg(windows)]
    if socket_path.to_string_lossy().starts_with(r"\\.\pipe\") {
        // Windows named pipes are in-memory kernel IPC objects managed by NPFS;
        // they are automatically cleaned up when process handles close, with no disk files.
        return Ok(false);
    }

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
