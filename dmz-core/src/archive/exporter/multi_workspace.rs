use super::packaging::{append_dir_all, append_dir_all_excluding, append_filtered_files_prefixed};
use super::types::ExportError;
use crate::resolver::lockfile::DmzLockfile;
use crate::workspace::{MemberRole, WorkspaceProjectConfig};
use std::collections::HashSet;
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
    _lockfile: &DmzLockfile,
    deterministic_epoch: u64,
    with_toolchain: bool,
) -> Result<WorkspaceProjectConfig, ExportError> {
    let primary_raw_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("primary")
        .to_string();

    let mut member_entries: Vec<(PathBuf, String)> = Vec::with_capacity(ws_project.members.len());
    let mut used_folder_names: HashSet<String> = HashSet::new();
    let mut primary_folder_name = primary_raw_name.clone();

    // Assign primary folder name first so sidecar disambiguation never collides with it
    if let Some(primary) = ws_project.primary_member() {
        primary_folder_name =
            if !primary.id.is_empty() && primary.id != "primary" && primary.id != "default" {
                primary.id.clone()
            } else {
                primary_raw_name.clone()
            };
    }

    for member in &ws_project.members {
        if member.role == MemberRole::Primary {
            let mut folder_name = primary_folder_name.clone();
            if !used_folder_names.insert(folder_name.clone()) {
                let mut suffix = 2usize;
                while !used_folder_names.insert(format!("{}-{}", folder_name, suffix)) {
                    suffix += 1;
                }
                folder_name = format!("{}-{}", folder_name, suffix);
            }
            member_entries.push((workspace_root.to_path_buf(), folder_name));
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
            let base_name = if !member.id.is_empty()
                && member.id != "sidecar"
                && member.id != "service"
                && member.id != "default"
            {
                member.id.clone()
            } else {
                raw_name
            };
            let mut folder_name = if base_name == primary_folder_name {
                format!("{}-sidecar", base_name)
            } else {
                base_name
            };
            if !used_folder_names.insert(folder_name.clone()) {
                let mut suffix = 2usize;
                while !used_folder_names.insert(format!("{}-{}", folder_name, suffix)) {
                    suffix += 1;
                }
                folder_name = format!("{}-{}", folder_name, suffix);
            }
            member_entries.push((host_path, folder_name));
        }
    }

    let primary_folder_name = ws_project
        .members
        .iter()
        .enumerate()
        .find(|(_, m)| m.role == MemberRole::Primary)
        .and_then(|(idx, _)| member_entries.get(idx).map(|(_, f)| f.clone()))
        .unwrap_or(primary_folder_name);

    // 1. Package Primary Repository into its dedicated workspaces/<primary_folder> namespace
    let primary_dest = PathBuf::from(format!("workspaces/{}", primary_folder_name));
    info!(
        folder = %primary_dest.display(),
        path = ?workspace_root,
        "Packaging primary workspace files into workspaces namespace"
    );
    let root_canon =
        fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let primary_excluded: Vec<PathBuf> = member_entries
        .iter()
        .enumerate()
        .filter(|(idx, _)| {
            ws_project
                .members
                .get(*idx)
                .is_some_and(|m| m.role != MemberRole::Primary)
        })
        .filter_map(|(_, (p, _))| {
            let c = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            (c != root_canon && c.starts_with(&root_canon)).then_some(c)
        })
        .collect();

    if let Some(changed_files) = changed_files_filter {
        append_filtered_files_prefixed(
            tar_builder,
            workspace_root,
            &primary_dest,
            changed_files,
            deterministic_epoch,
        )?;
    } else {
        append_dir_all_excluding(
            tar_builder,
            workspace_root,
            &primary_dest,
            true,
            deterministic_epoch,
            &primary_excluded,
        )?;
    }

    // 2. Package each Sidecar / Service Member into its dedicated workspaces/<sidecar_folder> namespace
    for (idx, member) in ws_project.members.iter().enumerate() {
        if member.role != MemberRole::Primary {
            if let Some((host_path, folder_name)) = member_entries.get(idx) {
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

    // 3. Serialize and embed normalized dmz-workspace.toml pointing to workspaces/<folder> subfolders
    let mut normalized_project = ws_project.clone();
    for (idx, m) in normalized_project.members.iter_mut().enumerate() {
        if let Some((_, folder_name)) = member_entries.get(idx) {
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
    let folder_storage: Vec<(String, String, &Path, &Path, Option<&str>)> = normalized_project
        .members
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let folder_path = m.path.to_string_lossy().replace('\\', "/");
            let short_folder_name = m.path.file_name().and_then(|n| n.to_str()).unwrap_or(&m.id);
            let label = if !m.id.is_empty() {
                m.id.clone()
            } else {
                short_folder_name.to_string()
            };
            let host_dir = member_entries
                .get(idx)
                .map(|(p, _)| p.as_path())
                .unwrap_or(workspace_root);
            (
                label,
                folder_path,
                m.path.as_path(),
                host_dir,
                m.ecosystem.as_deref(),
            )
        })
        .collect();

    let folder_inputs: Vec<crate::toolchain::setup::ide::WorkspaceFolderInput<'_>> = folder_storage
        .iter()
        .map(|(label, folder_path, rel_path, fs_dir, ecosystem)| {
            crate::toolchain::setup::ide::WorkspaceFolderInput {
                label: label.as_str(),
                folder_path: folder_path.as_str(),
                rel_path,
                fs_dir,
                ecosystem: *ecosystem,
            }
        })
        .collect();

    let workspace_display_name = if !ws_project.name.is_empty() {
        &ws_project.name
    } else {
        "workspace"
    };

    let code_workspace_obj = crate::toolchain::setup::ide::build_multi_root_workspace_doc(
        workspace_root,
        &folder_inputs,
        with_toolchain,
        false,
    );

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
