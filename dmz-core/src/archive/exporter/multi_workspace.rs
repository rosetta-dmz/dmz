use super::packaging::{append_dir_all, append_filtered_files};
use super::types::ExportError;
use crate::resolver::lockfile::DmzLockfile;
use crate::workspace::{MemberRole, WorkspaceProjectConfig};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::info;

#[allow(clippy::too_many_arguments)]
pub fn package_multi_workspace_umbrella<W: Write>(
    tar_builder: &mut TarBuilder<W>,
    workspace_root: &Path,
    ws_project: &WorkspaceProjectConfig,
    manifest_path_opt: Option<&Path>,
    changed_files_filter: Option<&HashSet<PathBuf>>,
    lockfile: &DmzLockfile,
    deterministic_epoch: u64,
    with_toolchain: bool,
) -> Result<WorkspaceProjectConfig, ExportError> {
    let primary_raw_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("primary")
        .to_string();

    let mut member_folder_names = HashMap::new();
    let mut member_host_paths = HashMap::new();

    for member in &ws_project.members {
        if member.role == MemberRole::Primary {
            let folder_name =
                if !member.id.is_empty() && member.id != "primary" && member.id != "default" {
                    member.id.clone()
                } else {
                    primary_raw_name.clone()
                };
            member_folder_names.insert(member.id.clone(), folder_name);
            member_host_paths.insert(member.id.clone(), workspace_root.to_path_buf());
        } else {
            let host_path = if member.path.is_relative() {
                if let Some(m_path) = manifest_path_opt {
                    m_path.parent().unwrap_or(workspace_root).join(&member.path)
                } else {
                    workspace_root.join(&member.path)
                }
            } else {
                member.path.clone()
            };
            let host_path = fs::canonicalize(&host_path).unwrap_or(host_path);
            let raw_name = host_path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| !n.is_empty())
                .unwrap_or(&member.id)
                .to_string();
            let folder_name = if !member.id.is_empty()
                && member.id != "sidecar"
                && member.id != "service"
                && member.id != "default"
            {
                member.id.clone()
            } else {
                raw_name
            };
            member_folder_names.insert(member.id.clone(), folder_name);
            member_host_paths.insert(member.id.clone(), host_path);
        }
    }

    // Disambiguate if primary and sidecar resolve to the same folder name
    let primary_folder_name = member_folder_names
        .get(
            ws_project
                .primary_member()
                .map(|m| m.id.as_str())
                .unwrap_or("primary"),
        )
        .cloned()
        .unwrap_or_else(|| primary_raw_name.clone());

    for (id, name) in member_folder_names.iter_mut() {
        let is_primary = ws_project
            .primary_member()
            .map(|m| m.id == *id)
            .unwrap_or(false);
        if !is_primary && *name == primary_folder_name {
            *name = format!("{}-sidecar", name);
        }
    }

    // 1. Package Primary Repository into its dedicated workspaces/<primary_folder> namespace
    let primary_dest = PathBuf::from(format!("workspaces/{}", primary_folder_name));
    info!(
        folder = %primary_dest.display(),
        path = ?workspace_root,
        "Packaging primary workspace files into workspaces namespace"
    );
    if let Some(changed_files) = changed_files_filter {
        append_filtered_files(
            tar_builder,
            workspace_root,
            changed_files,
            deterministic_epoch,
        )?;
    } else {
        append_dir_all(
            tar_builder,
            workspace_root,
            &primary_dest,
            true,
            deterministic_epoch,
        )?;
    }

    // 2. Package each Sidecar / Service Member into its dedicated workspaces/<sidecar_folder> namespace
    for member in &ws_project.members {
        if member.role != MemberRole::Primary {
            if let Some(host_path) = member_host_paths.get(&member.id) {
                if let Some(folder_name) = member_folder_names.get(&member.id) {
                    if host_path.is_dir() {
                        let sidecar_dest = PathBuf::from(format!("workspaces/{}", folder_name));
                        info!(
                            member = %member.id,
                            src = ?host_path,
                            dest = %sidecar_dest.display(),
                            "Packaging sidecar workspace files into workspaces namespace"
                        );
                        append_dir_all(
                            tar_builder,
                            host_path,
                            &sidecar_dest,
                            true,
                            deterministic_epoch,
                        )?;
                    }
                }
            }
        }
    }

    // 3. Serialize and embed normalized dmz-workspace.toml pointing to workspaces/<folder> subfolders
    let mut normalized_project = ws_project.clone();
    for m in &mut normalized_project.members {
        if let Some(folder_name) = member_folder_names.get(&m.id) {
            m.path = PathBuf::from(format!("workspaces/{}", folder_name));
        }
    }
    if let Ok(toml_str) = normalized_project.to_toml_string() {
        let mut header = tar::Header::new_gnu();
        header.set_size(toml_str.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(deterministic_epoch);
        header.set_uid(0);
        header.set_gid(0);
        let _ = header.set_username("");
        let _ = header.set_groupname("");
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "dmz-workspace.toml", toml_str.as_bytes())
            .map_err(|e| {
                ExportError::ArchiveError(format!("Failed to embed dmz-workspace.toml: {}", e))
            })?;
    }

    // 4. Generate and embed VS Code Multi-Root Workspace (.code-workspace)
    let mut folders = Vec::new();
    let mut linked_rust_projects = Vec::new();
    let mut node_bin_unix = Vec::new();
    let mut node_bin_win = Vec::new();

    for m in &normalized_project.members {
        let folder_path = m.path.to_string_lossy().replace('\\', "/");
        let short_folder_name = m.path.file_name().and_then(|n| n.to_str()).unwrap_or(&m.id);
        let host_dir = member_host_paths
            .get(&m.id)
            .map(|p| p.as_path())
            .unwrap_or(workspace_root);

        let is_python = m.ecosystem.as_deref() == Some("python")
            || m.ecosystem.as_deref() == Some("pypi")
            || host_dir.join("requirements.txt").exists()
            || host_dir.join("pyproject.toml").exists();

        let is_rust = m.ecosystem.as_deref() == Some("rust")
            || m.ecosystem.as_deref() == Some("cargo")
            || host_dir.join("Cargo.toml").exists()
            || host_dir.join("src-tauri/Cargo.toml").exists();

        let is_node = m.ecosystem.as_deref() == Some("node")
            || m.ecosystem.as_deref() == Some("npm")
            || host_dir.join("package.json").exists();

        if is_rust {
            if host_dir.join("Cargo.toml").exists() {
                linked_rust_projects.push(format!("{}/Cargo.toml", folder_path));
            }
            if host_dir.join("src-tauri/Cargo.toml").exists() {
                linked_rust_projects.push(format!("{}/src-tauri/Cargo.toml", folder_path));
            }
        }

        let label = if !m.id.is_empty() {
            m.id.clone()
        } else {
            short_folder_name.to_string()
        };

        if is_node {
            node_bin_unix.push(format!("${{workspaceFolder:{}}}/node_modules/.bin", label));
            node_bin_unix.push(format!(
                "${{workspaceFolder}}/{}/node_modules/.bin",
                folder_path
            ));
            node_bin_win.push(format!(
                "${{workspaceFolder:{}}}\\\\node_modules\\\\.bin",
                label
            ));
            node_bin_win.push(format!(
                "${{workspaceFolder}}\\{}\\node_modules\\.bin",
                folder_path.replace('/', "\\")
            ));
        }

        let mut folder_entry = serde_json::json!({
            "name": label,
            "path": folder_path
        });

        if is_python {
            let mut py_settings = serde_json::json!({
                "python.terminal.activateEnvironment": true
            });

            let has_bundled_py =
                with_toolchain || workspace_root.join(".dmz/toolchains/python").exists();
            let member_venv = host_dir.join(".venv");
            let has_locked_pypi = lockfile
                .packages
                .values()
                .any(|p| p.ecosystem == crate::resolver::manifest::Ecosystem::PyPI);

            let depth = m
                .path
                .components()
                .filter(|c| matches!(c, std::path::Component::Normal(_)))
                .count();
            let rel_prefix = if depth == 0 {
                ".".to_string()
            } else {
                vec![".."; depth].join("/")
            };

            let py_bin_exists = member_venv.join("bin/python").exists()
                || member_venv.join("bin/python3").exists()
                || member_venv.join("Scripts/python.exe").exists()
                || member_venv.join("Scripts/python.bat").exists()
                || member_venv.join("Scripts/python.cmd").exists()
                || member_venv.join("pyvenv.cfg").exists();

            if (member_venv.exists() && py_bin_exists) || has_locked_pypi {
                let py_bin = if cfg!(windows) {
                    "${workspaceFolder}/.venv/Scripts/python.exe"
                } else {
                    "${workspaceFolder}/.venv/bin/python"
                };
                if let Some(obj) = py_settings.as_object_mut() {
                    obj.insert(
                        "python.defaultInterpreterPath".to_string(),
                        serde_json::Value::String(py_bin.to_string()),
                    );
                }
            } else if has_bundled_py {
                let py_bin = if rel_prefix == "." {
                    if cfg!(windows) {
                        "${workspaceFolder}/.dmz/toolchains/python/python.exe".to_string()
                    } else {
                        "${workspaceFolder}/.dmz/toolchains/python/bin/python3".to_string()
                    }
                } else if cfg!(windows) {
                    format!(
                        "${{workspaceFolder}}/{}/.dmz/toolchains/python/python.exe",
                        rel_prefix
                    )
                } else {
                    format!(
                        "${{workspaceFolder}}/{}/.dmz/toolchains/python/bin/python3",
                        rel_prefix
                    )
                };
                if let Some(obj) = py_settings.as_object_mut() {
                    obj.insert(
                        "python.defaultInterpreterPath".to_string(),
                        serde_json::Value::String(py_bin),
                    );
                }
            }

            if let Some(obj) = folder_entry.as_object_mut() {
                obj.insert("settings".to_string(), py_settings);
            }
        }

        folders.push(folder_entry);
    }

    let workspace_display_name = if !ws_project.name.is_empty() {
        &ws_project.name
    } else {
        "workspace"
    };

    let unix_node_prefix = if !node_bin_unix.is_empty() {
        format!("{}:", node_bin_unix.join(":"))
    } else {
        String::new()
    };

    let win_node_prefix = if !node_bin_win.is_empty() {
        format!("{};", node_bin_win.join(";"))
    } else {
        String::new()
    };

    let code_workspace_obj = serde_json::json!({
        "folders": folders,
        "settings": {
            "rust-analyzer.linkedProjects": linked_rust_projects,
            "terminal.integrated.env.osx": {
                "PATH": format!("{}${{workspaceFolder}}/.dmz/toolchains/node/bin:${{workspaceFolder}}/.dmz/toolchains/rust/bin:${{workspaceFolder}}/.dmz/toolchains/python/bin:${{env:PATH}}", unix_node_prefix)
            },
            "terminal.integrated.env.linux": {
                "PATH": format!("{}${{workspaceFolder}}/.dmz/toolchains/node/bin:${{workspaceFolder}}/.dmz/toolchains/rust/bin:${{workspaceFolder}}/.dmz/toolchains/python/bin:${{env:PATH}}", unix_node_prefix)
            },
            "terminal.integrated.env.windows": {
                "PATH": format!("{}${{workspaceFolder}}\\.dmz\\toolchains\\node\\bin;${{workspaceFolder}}\\.dmz\\toolchains\\rust\\bin;${{workspaceFolder}}\\.dmz\\toolchains\\python;${{env:PATH}}", win_node_prefix)
            }
        }
    });

    if let Ok(cw_bytes) = serde_json::to_vec_pretty(&code_workspace_obj) {
        let mut header = tar::Header::new_gnu();
        header.set_size(cw_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(deterministic_epoch);
        header.set_uid(0);
        header.set_gid(0);
        let _ = header.set_username("");
        let _ = header.set_groupname("");
        header.set_cksum();
        tar_builder
            .append_data(
                &mut header,
                format!("{}.code-workspace", workspace_display_name),
                cw_bytes.as_slice(),
            )
            .map_err(|e| {
                ExportError::ArchiveError(format!("Failed to embed .code-workspace: {}", e))
            })?;
    }

    // 5. Generate and embed comprehensive self-contained operational README.md
    let members_list = normalized_project
        .members
        .iter()
        .filter(|m| m.role != MemberRole::Primary)
        .map(|m| {
            format!(
                "- `{}/`: Sidecar companion service (`{}`)\n",
                m.path.display().to_string().replace('\\', "/"),
                m.id
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let primary_member_path = normalized_project
        .primary_member()
        .map(|m| m.path.display().to_string().replace('\\', "/"))
        .unwrap_or_else(|| format!("workspaces/{}", primary_folder_name));

    let ipc_env_var = &ws_project.ipc.env_var;
    let ipc_sock_id = &ws_project.ipc.socket_id;
    let readiness_timeout = ws_project.ipc.readiness_timeout_ms;

    let readme_content = format!(
        r#"# {workspace_name} Air-Gap Workspace Bundle

This directory is a self-contained, hermetic multi-workspace bundle generated by DMZ.
It provides an isolated, production-grade development environment with zero external network dependencies.

## System Architecture

The project comprises a companion service architecture managed by the DMZ supervisor:

```text
┌─────────────────────────────────────────────────────────────┐
│                       DMZ SUPERVISOR                        │
│                                                             │
│  ┌────────────────────────┐      ┌────────────────────────┐ │
│  │ {primary_path:<22} │      │ workspaces/<sidecar>   │ │
│  │  (Primary Application) │      │   (Companion Sidecar)  │ │
│  └───────────┬────────────┘      └───────────┬────────────┘ │
│              │                               │              │
│              │     Unix Domain Socket        │              │
│              └───────► {ipc_env:<18} ◄────┘              │
│                   (Zero-Network IPC Gate)                   │
└─────────────────────────────────────────────────────────────┘
```

## Preserved Directory Layout

- `workspaces/`: Dedicated namespace housing all repository source code trees.
  - `{primary_path}/`: Primary application repository.
{members_list}- `.dmz/`: Hermetic offline dependencies and toolchains.
  - `.dmz/store/`: Cryptographically verified package archive store.
  - `.dmz/toolchains/`: Bundled offline compilers and runtimes.
- `dmz-workspace.toml`: Multi-project workspace orchestrator and IPC manifest.
- `dmz.lock`: Cryptographic dependency lockfile verifying complete reproducibility.
- `{workspace_name}.code-workspace`: VS Code Multi-Root Workspace configuration.

## Developer Quickstart

### 1. Opening in VS Code / Cursor
Open the unified multi-root workspace file directly in your IDE:
```bash
code {workspace_name}.code-workspace
```
Or inside your IDE: **File > Open Workspace from File...** and select `{workspace_name}.code-workspace`.
Both repositories appear as peer folders in your explorer with properly isolated language servers, linters, and toolchain paths.

### 2. Running with the DMZ Supervisor
Launch both services under coordinated KEP-753 lifecycle supervision:
```bash
dmz run --workspace .
```
The supervisor orchestrates sequential execution:
1. Spawns the companion sidecar in its dedicated workspace directory.
2. Creates and verifies the IPC socket (`{ipc_sock_id}`) within {readiness_timeout}ms via a non-destructive handshake probe.
3. Injects `{ipc_env}` into the primary application and spawns it.
4. On shutdown, triggers a reverse graceful drain sequence.

### 3. Offline Build & Parity Commands
Each repository is pre-configured to build hermetically from the local `.dmz/store`:
- **Rust / Cargo**: Run `cargo check --offline` inside `{primary_path}/`. Sources are pre-routed via `.cargo/config.toml`.
- **Python**: Dependencies are materialized in `.dmz/venv/` with `.pth` bridge configured. Run `python3 -m py_compile .`.
- **Node / Tauri**: CLI shims and modules are populated in `node_modules/.bin/`.
"#,
        workspace_name = workspace_display_name,
        primary_path = primary_member_path,
        members_list = members_list,
        ipc_env = ipc_env_var,
        ipc_sock_id = ipc_sock_id,
        readiness_timeout = readiness_timeout,
    );

    let mut header = tar::Header::new_gnu();
    header.set_size(readme_content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(deterministic_epoch);
    header.set_uid(0);
    header.set_gid(0);
    let _ = header.set_username("");
    let _ = header.set_groupname("");
    header.set_cksum();
    tar_builder
        .append_data(&mut header, "README.md", readme_content.as_bytes())
        .map_err(|e| ExportError::ArchiveError(format!("Failed to embed README.md: {}", e)))?;

    Ok(normalized_project)
}
