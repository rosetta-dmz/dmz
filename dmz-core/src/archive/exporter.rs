use crate::ecosystem::EcosystemRegistry;
use crate::resolver::lockfile::DmzLockfile;
use crate::vcs::{GitProvider, VcsProvenance};
use crate::workspace::{MemberRole, WorkspaceProjectConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::{info, warn};
use zstd::stream::write::Encoder as ZstdEncoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    Unified,  // Workspace source code + dependency store archives
    CodeOnly, // Workspace source code only (no dependencies)
    DepsOnly, // Dependency store archives only (no source code)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaBaseInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_archive_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_closure_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirGapManifest {
    pub dmz_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_base: Option<DeltaBaseInfo>,
    #[serde(default)]
    pub has_git_history: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolchains: Vec<crate::toolchain::ToolchainManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_project: Option<crate::workspace::WorkspaceProjectConfig>,
    pub closure_signature: String,
    pub package_count: usize,
    pub created_at_utc: u64,
    pub archive_sha256: String,
}

#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub workspace_path: PathBuf,
    pub lockfile_path: PathBuf,
    pub store_dir: PathBuf,
    pub output_path: PathBuf,
    pub zstd_level: i32,
    pub mode: ExportMode,
    pub target_platform: Option<String>,
    pub with_git_history: bool,
    pub since: Option<String>,
    pub with_toolchain: bool,
    pub multi_workspace: Option<crate::workspace::WorkspaceProjectConfig>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            lockfile_path: PathBuf::from("dmz.lock"),
            store_dir: PathBuf::from(".dmz/store"),
            output_path: PathBuf::from("dist/airgap_bundle.tar.zst"),
            zstd_level: 3,
            mode: ExportMode::Unified,
            target_platform: None,
            with_git_history: false,
            since: None,
            with_toolchain: false,
            multi_workspace: None,
        }
    }
}

#[derive(Debug)]
pub enum ExportError {
    IoError(String),
    LockfileError(String),
    ArchiveError(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Export IO Error: {}", msg),
            Self::LockfileError(msg) => write!(f, "Export Lockfile Error: {}", msg),
            Self::ArchiveError(msg) => write!(f, "Export Archive Error: {}", msg),
        }
    }
}

impl std::error::Error for ExportError {}

pub struct AirGapExporter;

impl AirGapExporter {
    /// Bundles store packages, dmz.lock, and discovered ecosystem caches into a cryptographically verifiable air-gap archive
    pub fn export(config: &ExportConfig) -> Result<AirGapManifest, ExportError> {
        let workspace_root = if config.workspace_path.is_relative() {
            std::fs::canonicalize(&config.workspace_path)
                .unwrap_or_else(|_| config.workspace_path.clone())
        } else {
            config.workspace_path.clone()
        };

        let effective_lockfile = if config.lockfile_path.is_relative() {
            if workspace_root.join(&config.lockfile_path).exists() {
                workspace_root.join(&config.lockfile_path)
            } else if config.lockfile_path.exists() {
                std::fs::canonicalize(&config.lockfile_path)
                    .unwrap_or_else(|_| config.lockfile_path.clone())
            } else {
                workspace_root.join(&config.lockfile_path)
            }
        } else {
            config.lockfile_path.clone()
        };

        let effective_store = if config.store_dir.is_relative() {
            if workspace_root.join(&config.store_dir).exists() {
                workspace_root.join(&config.store_dir)
            } else if config.store_dir.exists() {
                std::fs::canonicalize(&config.store_dir)
                    .unwrap_or_else(|_| config.store_dir.clone())
            } else {
                workspace_root.join(&config.store_dir)
            }
        } else {
            config.store_dir.clone()
        };

        let effective_output = if config.output_path.is_relative() {
            if config.output_path.exists() {
                std::fs::canonicalize(&config.output_path)
                    .unwrap_or_else(|_| workspace_root.join(&config.output_path))
            } else {
                workspace_root.join(&config.output_path)
            }
        } else {
            config.output_path.clone()
        };

        info!(
            workspace = ?workspace_root,
            lockfile = ?effective_lockfile,
            store = ?effective_store,
            output = ?effective_output,
            with_git = config.with_git_history,
            since = ?config.since,
            "Initiating air-gap package export"
        );

        // Detect VCS Provenance
        let vcs_provenance = GitProvider::get_provenance(&workspace_root).ok().flatten();
        if let Some(ref prov) = vcs_provenance {
            info!(commit = %prov.commit, branch = ?prov.branch, dirty = prov.dirty, "Detected workspace VCS provenance");
        }

        // Parse Delta Baseline if `--since` was requested
        let mut delta_base = None;
        let mut changed_files_filter: Option<HashSet<PathBuf>> = None;

        if let Some(since_spec) = &config.since {
            let since_path = Path::new(since_spec);
            if since_path.exists() {
                if since_spec.ends_with(".zst") || since_spec.ends_with(".tar.zst") {
                    if let Ok(prev_manifest) = Self::read_manifest_from_archive(since_path) {
                        let base_commit = prev_manifest.vcs.as_ref().map(|v| v.commit.clone());
                        delta_base = Some(DeltaBaseInfo {
                            base_commit: base_commit.clone(),
                            base_archive_sha256: Some(prev_manifest.archive_sha256),
                            base_closure_signature: Some(prev_manifest.closure_signature),
                        });
                        if let Some(ref commit) = base_commit {
                            if let Ok(files) =
                                GitProvider::get_changed_files_since(&workspace_root, commit)
                            {
                                changed_files_filter = Some(files.into_iter().collect());
                            }
                        }
                    }
                } else if since_path
                    .file_name()
                    .map(|n| n == "airgap_manifest.json")
                    .unwrap_or(false)
                {
                    if let Ok(content) = fs::read_to_string(since_path) {
                        if let Ok(prev_manifest) = serde_json::from_str::<AirGapManifest>(&content)
                        {
                            let base_commit = prev_manifest.vcs.as_ref().map(|v| v.commit.clone());
                            delta_base = Some(DeltaBaseInfo {
                                base_commit: base_commit.clone(),
                                base_archive_sha256: Some(prev_manifest.archive_sha256),
                                base_closure_signature: Some(prev_manifest.closure_signature),
                            });
                            if let Some(ref commit) = base_commit {
                                if let Ok(files) =
                                    GitProvider::get_changed_files_since(&workspace_root, commit)
                                {
                                    changed_files_filter = Some(files.into_iter().collect());
                                }
                            }
                        }
                    }
                }
            }

            if delta_base.is_none() {
                // Assume since_spec is a git commit hash or ref
                let base_commit = since_spec.trim().to_string();
                if let Ok(files) =
                    GitProvider::get_changed_files_since(&workspace_root, &base_commit)
                {
                    info!(base = %base_commit, count = files.len(), "Computed changed files since git ref");
                    changed_files_filter = Some(files.into_iter().collect());
                }
                delta_base = Some(DeltaBaseInfo {
                    base_commit: Some(base_commit),
                    base_archive_sha256: None,
                    base_closure_signature: None,
                });
            }
        }

        // Automatically discover active workspace ecosystems & gather generalized caches
        let registry = EcosystemRegistry::new();

        let detected_stacks = registry
            .process_workspace(&workspace_root)
            .map_err(|e| ExportError::ArchiveError(format!("Ecosystem discovery failed: {}", e)))?;

        info!(stacks = ?detected_stacks, "Discovered active workspace ecosystems");

        let cache_paths = registry.gather_all_caches(&workspace_root);
        info!(caches = ?cache_paths, "Gathered generalized ecosystem cache paths for archiving");

        // Verify dmz.lock integrity before exporting
        let mut lockfile = DmzLockfile::load_and_verify(&effective_lockfile)
            .map_err(|e| ExportError::LockfileError(e.to_string()))?;

        // Synchronize lockfile VCS provenance if not already set
        if lockfile.vcs.is_none() && vcs_provenance.is_some() {
            lockfile.vcs = vcs_provenance.clone();
            let _ = lockfile.save(&effective_lockfile);
        }

        // Ensure parent output directory exists
        if let Some(parent) = effective_output.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    ExportError::IoError(format!("Failed to create parent dir: {}", e))
                })?;
            }
        }

        // Prepare archive output stream
        let file = File::create(&effective_output)
            .map_err(|e| ExportError::IoError(format!("Failed to create export archive: {}", e)))?;

        let zstd_encoder = ZstdEncoder::new(file, config.zstd_level)
            .map_err(|e| ExportError::ArchiveError(format!("Failed to initialize zstd: {}", e)))?;

        let mut tar_builder = TarBuilder::new(zstd_encoder);

        // Append lockfile
        tar_builder
            .append_path_with_name(&effective_lockfile, "dmz.lock")
            .map_err(|e| ExportError::ArchiveError(format!("Failed to pack dmz.lock: {}", e)))?;

        // Append store items (if mode is Unified or DepsOnly)
        let mut packed_count = 0;
        if config.mode == ExportMode::Unified || config.mode == ExportMode::DepsOnly {
            for pkg_info in lockfile.packages.values() {
                // Skip local path dependencies/workspace members since they are in the workspace source
                if pkg_info.version.starts_with("path:") || pkg_info.version.contains("path") {
                    continue;
                }

                let safe_pkg_name = pkg_info.name.replace('/', "__");

                let pkg_store_path = effective_store.join(&safe_pkg_name);
                if pkg_store_path.exists() {
                    let archive_dest = Path::new("store").join(&safe_pkg_name);
                    if pkg_store_path.is_dir() {
                        Self::append_dir_all(
                            &mut tar_builder,
                            &pkg_store_path,
                            &archive_dest,
                            false,
                        )?;
                    } else {
                        tar_builder
                            .append_path_with_name(&pkg_store_path, &archive_dest)
                            .map_err(|e| {
                                ExportError::ArchiveError(format!(
                                    "Failed to pack package {}: {}",
                                    pkg_info.name, e
                                ))
                            })?;
                    }
                    packed_count += 1;
                } else {
                    return Err(ExportError::ArchiveError(format!(
                        "Missing store artifact for locked package '{}@{}' at {:?}. Run 'dmz fetch' before exporting.",
                        pkg_info.name, pkg_info.version, pkg_store_path
                    )));
                }
            }
        }

        // Detect Multi-Workspace Configuration
        let (ws_project_opt, manifest_path_opt) = match config.multi_workspace.clone() {
            Some(cfg) => (Some(cfg), None),
            None => WorkspaceProjectConfig::discover_workspace_manifest(&workspace_root)
                .map(|(cfg, p)| (Some(cfg), Some(p)))
                .unwrap_or((None, None)),
        };

        let mut final_ws_project = None;

        // Append workspace source code (if mode is Unified or CodeOnly)
        if config.mode == ExportMode::Unified || config.mode == ExportMode::CodeOnly {
            if let Some(ref ws_project) = ws_project_opt {
                // --- MULTI-WORKSPACE UMBRELLA EXPORT ---
                let primary_folder_name = workspace_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("primary")
                    .to_string();

                let mut member_folder_names = HashMap::new();
                let mut member_host_paths = HashMap::new();

                for member in &ws_project.members {
                    if member.role == MemberRole::Primary {
                        member_folder_names.insert(member.id.clone(), primary_folder_name.clone());
                        member_host_paths.insert(member.id.clone(), workspace_root.clone());
                    } else {
                        let host_path = if member.path.is_relative() {
                            if let Some(ref m_path) = manifest_path_opt {
                                m_path
                                    .parent()
                                    .unwrap_or(&workspace_root)
                                    .join(&member.path)
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
                        let folder_name = if raw_name == primary_folder_name {
                            format!("{}-sidecar", raw_name)
                        } else {
                            raw_name
                        };
                        member_folder_names.insert(member.id.clone(), folder_name);
                        member_host_paths.insert(member.id.clone(), host_path);
                    }
                }

                // 1. Package Primary Repository into its dedicated umbrella subfolder
                info!(
                    folder = %primary_folder_name,
                    path = ?workspace_root,
                    "Packaging primary workspace files into umbrella subfolder"
                );
                if let Some(ref changed_files) = changed_files_filter {
                    Self::append_filtered_files(&mut tar_builder, &workspace_root, changed_files)?;
                } else {
                    Self::append_dir_all(
                        &mut tar_builder,
                        &workspace_root,
                        Path::new(&primary_folder_name),
                        true,
                    )?;
                }

                // 2. Package each Sidecar / Service Member into its dedicated umbrella subfolder
                for member in &ws_project.members {
                    if member.role != MemberRole::Primary {
                        if let Some(host_path) = member_host_paths.get(&member.id) {
                            if let Some(folder_name) = member_folder_names.get(&member.id) {
                                if host_path.is_dir() {
                                    info!(
                                        member = %member.id,
                                        src = ?host_path,
                                        dest = %folder_name,
                                        "Packaging sidecar workspace files into umbrella subfolder"
                                    );
                                    Self::append_dir_all(
                                        &mut tar_builder,
                                        host_path,
                                        Path::new(folder_name),
                                        true,
                                    )?;
                                }
                            }
                        }
                    }
                }

                // 3. Package Documentation folder if present
                let mut has_docs = false;
                let doc_candidates = [
                    workspace_root.join("documentation"),
                    workspace_root.join("docs"),
                    workspace_root
                        .parent()
                        .map(|p| p.join("documentation"))
                        .unwrap_or_default(),
                    workspace_root
                        .parent()
                        .map(|p| p.join("docs"))
                        .unwrap_or_default(),
                ];
                for candidate in &doc_candidates {
                    if candidate.is_dir() {
                        info!(path = ?candidate, "Packaging project documentation into umbrella archive");
                        let _ = Self::append_dir_all(
                            &mut tar_builder,
                            candidate,
                            Path::new("documentation"),
                            false,
                        );
                        has_docs = true;
                        break;
                    }
                }

                // 4. Package Dist folder if present
                let dist_candidates = [
                    workspace_root.join("dist"),
                    workspace_root
                        .parent()
                        .map(|p| p.join("dist"))
                        .unwrap_or_default(),
                ];
                for candidate in &dist_candidates {
                    if candidate.is_dir() {
                        let has_files = fs::read_dir(candidate)
                            .map(|entries| {
                                entries.flatten().any(|e| {
                                    let name = e.file_name().to_string_lossy().to_string();
                                    !name.ends_with(".zst") && !name.ends_with(".sha256")
                                })
                            })
                            .unwrap_or(false);
                        if has_files {
                            info!(path = ?candidate, "Packaging distribution assets into umbrella archive");
                            let _ = Self::append_dir_all(
                                &mut tar_builder,
                                candidate,
                                Path::new("dist"),
                                false,
                            );
                            break;
                        }
                    }
                }

                // 5. Serialize and embed normalized dmz-workspace.toml pointing to umbrella subfolders
                let mut normalized_project = ws_project.clone();
                for m in &mut normalized_project.members {
                    if let Some(folder_name) = member_folder_names.get(&m.id) {
                        m.path = PathBuf::from(folder_name);
                    }
                }
                if let Ok(toml_str) = normalized_project.to_toml_string() {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(toml_str.len() as u64);
                    header.set_mode(0o644);
                    header.set_cksum();
                    let _ = tar_builder.append_data(
                        &mut header,
                        "dmz-workspace.toml",
                        toml_str.as_bytes(),
                    );
                }

                // 6. Generate and embed VS Code Multi-Root Workspace (.code-workspace)
                let mut folders = Vec::new();
                let mut linked_rust_projects = Vec::new();
                let mut node_bin_unix = Vec::new();
                let mut node_bin_win = Vec::new();

                for m in &normalized_project.members {
                    let folder_name = m.path.to_string_lossy().to_string();
                    let host_dir = member_host_paths.get(&m.id).unwrap_or(&workspace_root);

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
                            linked_rust_projects.push(format!("{}/Cargo.toml", folder_name));
                        }
                        if host_dir.join("src-tauri/Cargo.toml").exists() {
                            linked_rust_projects
                                .push(format!("{}/src-tauri/Cargo.toml", folder_name));
                        }
                    }

                    if is_node {
                        node_bin_unix.push(format!(
                            "${{workspaceFolder}}/{}/node_modules/.bin",
                            folder_name
                        ));
                        node_bin_win.push(format!(
                            "${{workspaceFolder}}\\{}\\node_modules\\.bin",
                            folder_name
                        ));
                    }

                    let label = if m.role == MemberRole::Primary {
                        format!("Primary: {}", folder_name)
                    } else {
                        format!("Sidecar: {}", m.id)
                    };

                    let mut folder_entry = serde_json::json!({
                        "name": label,
                        "path": folder_name
                    });

                    if is_python {
                        folder_entry["settings"] = serde_json::json!({
                            "python.defaultInterpreterPath": "${workspaceFolder}/.dmz/toolchains/python/bin/python3",
                            "python.terminal.activateEnvironment": true
                        });
                    }

                    folders.push(folder_entry);
                }

                if has_docs {
                    folders.push(serde_json::json!({
                        "name": "Documentation",
                        "path": "documentation"
                    }));
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
                    header.set_cksum();
                    let _ = tar_builder.append_data(
                        &mut header,
                        format!("{}.code-workspace", workspace_display_name),
                        cw_bytes.as_slice(),
                    );
                }

                // 7. Generate and embed self-contained air-gap quickstart README.md
                let members_list = normalized_project
                    .members
                    .iter()
                    .filter(|m| m.role != MemberRole::Primary)
                    .map(|m| format!("- `{}/`: Sidecar member (`{}`)\n", m.path.display(), m.id))
                    .collect::<Vec<_>>()
                    .join("");

                let readme_content = format!(
                    "# {} Air-Gap Workspace Bundle\n\n\
                    This directory is a self-contained, hermetic multi-workspace bundle generated by DMZ.\n\n\
                    ## Preserved Project Layout\n\
                    - `{}/`: Primary application repository.\n\
                    {}\
                    - `documentation/`: Project architecture guides and documentation (if packaged).\n\
                    - `dist/`: Pre-built deliverables and release assets (if packaged).\n\
                    - `.dmz/`: Offline package store (`.dmz/store`) and hermetic toolchains (`.dmz/toolchains`).\n\
                    - `dmz-workspace.toml`: Multi-project workspace orchestrator manifest.\n\
                    - `{}.code-workspace`: VS Code Multi-Root Workspace configuration.\n\n\
                    ## Opening the Workspace\n\
                    Open the multi-root workspace file directly in VS Code, Cursor, or your IDE:\n\
                    ```bash\n\
                    code {}.code-workspace\n\
                    ```\n\
                    Or in VS Code: **File > Open Workspace from File...** and select `{}.code-workspace`.\n\n\
                    Both repositories will appear as peer folders in your explorer with properly isolated language servers and interpreter paths.\n\n\
                    ## Running the Application\n\
                    To run the primary application with its sidecar under the DMZ supervisor:\n\
                    ```bash\n\
                    dmz run --workspace .\n\
                    ```\n",
                    workspace_display_name,
                    primary_folder_name,
                    members_list,
                    workspace_display_name,
                    workspace_display_name,
                    workspace_display_name
                );

                let mut header = tar::Header::new_gnu();
                header.set_size(readme_content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                let _ =
                    tar_builder.append_data(&mut header, "README.md", readme_content.as_bytes());

                final_ws_project = Some(normalized_project);
            } else {
                // --- SINGLE-WORKSPACE FLAT EXPORT (Backward Compatibility) ---
                info!(path = ?workspace_root, "Appending workspace source code to archive");
                if workspace_root.is_dir() {
                    if let Some(ref changed_files) = changed_files_filter {
                        info!(
                            changed_count = changed_files.len(),
                            "Packaging incremental delta workspace files"
                        );
                        Self::append_filtered_files(
                            &mut tar_builder,
                            &workspace_root,
                            changed_files,
                        )?;
                    } else {
                        Self::append_dir_all(
                            &mut tar_builder,
                            &workspace_root,
                            Path::new(""),
                            false,
                        )?;
                    }
                }
            }
        }

        // Append Git Bundle if requested (`--with-git-history`)
        let mut has_git_history = false;
        if config.with_git_history {
            let temp_dir = tempfile::tempdir().map_err(|e| ExportError::IoError(e.to_string()))?;
            let temp_bundle = temp_dir.path().join("workspace.bundle");
            let since_commit_for_bundle =
                delta_base.as_ref().and_then(|d| d.base_commit.as_deref());

            match GitProvider::create_bundle(&workspace_root, &temp_bundle, since_commit_for_bundle)
            {
                Ok(binfo) => {
                    info!(
                        objects = binfo.object_count,
                        head = %binfo.head_commit,
                        "Adding deterministic workspace.bundle to air-gap archive"
                    );
                    tar_builder
                        .append_path_with_name(&temp_bundle, "workspace.bundle")
                        .map_err(|e| {
                            ExportError::ArchiveError(format!(
                                "Failed to pack workspace.bundle: {}",
                                e
                            ))
                        })?;
                    has_git_history = true;
                }
                Err(e) => {
                    warn!("Could not create git bundle for workspace: {}", e);
                }
            }
        }

        // Package active host toolchains if requested (`--with-toolchain`)
        let mut toolchain_manifest_entries = Vec::new();
        if config.with_toolchain {
            info!("Initiating host toolchain discovery and packaging");
            match crate::toolchain::ToolchainPackager::package_toolchains_with_lockfile(
                &workspace_root,
                Some(&lockfile),
                &mut tar_builder,
            ) {
                Ok(entries) => {
                    info!(
                        "Captured and packaged language toolchains into bundle count={}",
                        entries.len()
                    );
                    toolchain_manifest_entries = entries;
                }
                Err(e) => {
                    return Err(ExportError::ArchiveError(format!(
                        "Toolchain packaging failed: {}",
                        e
                    )));
                }
            }
        }

        let host_platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let target_platform = config
            .target_platform
            .clone()
            .or_else(|| lockfile.target_platform.clone())
            .or_else(|| config.with_toolchain.then_some(host_platform));

        // Write air-gap manifest header
        let manifest = AirGapManifest {
            dmz_version: env!("CARGO_PKG_VERSION").to_string(),
            target_platform,
            vcs: vcs_provenance,
            delta_base,
            has_git_history,
            toolchains: toolchain_manifest_entries,
            workspace_project: final_ws_project.or_else(|| config.multi_workspace.clone()),
            closure_signature: lockfile.closure_signature.clone(),
            package_count: packed_count,
            created_at_utc: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            archive_sha256: String::new(), // Populated post-compression
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| ExportError::ArchiveError(e.to_string()))?;

        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_json.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        tar_builder
            .append_data(
                &mut header,
                "airgap_manifest.json",
                manifest_json.as_bytes(),
            )
            .map_err(|e| {
                ExportError::ArchiveError(format!("Failed to embed airgap manifest: {}", e))
            })?;

        // Finish streams and ensure file handle is fully closed
        {
            let zstd_encoder = tar_builder.into_inner().map_err(|e| {
                ExportError::ArchiveError(format!("Failed to finalize tar stream: {}", e))
            })?;

            let mut inner_file = zstd_encoder
                .finish()
                .map_err(|e| format!("Failed to finish zstd compression: {}", e))
                .map_err(ExportError::ArchiveError)?;

            inner_file
                .flush()
                .map_err(|e| ExportError::IoError(e.to_string()))?;

            inner_file
        };

        // Compute final archive checksum
        let archive_sha256 = Self::calculate_file_sha256(&effective_output).map_err(|e| {
            ExportError::IoError(format!("Failed to calculate archive hash: {}", e))
        })?;
        let mut final_manifest = manifest;
        final_manifest.archive_sha256 = archive_sha256;

        // Write sidecar checksum file: <output_path>.sha256
        let sidecar_path = PathBuf::from(format!("{}.sha256", effective_output.display()));
        let filename = effective_output
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "bundle.tar.zst".to_string());
        let sidecar_content = format!("{}  {}\n", final_manifest.archive_sha256, filename);
        if let Err(e) = fs::write(&sidecar_path, sidecar_content) {
            warn!(
                sidecar = ?sidecar_path,
                error = %e,
                "Could not write .sha256 sidecar checksum file"
            );
        } else {
            info!(sidecar = ?sidecar_path, "Wrote .sha256 sidecar checksum file");
        }

        info!(
            checksum = %final_manifest.archive_sha256,
            packages = final_manifest.package_count,
            has_git = final_manifest.has_git_history,
            delta = final_manifest.delta_base.is_some(),
            "Air-gap export completed successfully"
        );

        Ok(final_manifest)
    }

    fn append_dir_all<W: Write>(
        tar: &mut TarBuilder<W>,
        src_dir: &Path,
        dest_prefix: &Path,
        skip_manifest: bool,
    ) -> Result<(), ExportError> {
        let entries = fs::read_dir(src_dir).map_err(|e| {
            ExportError::IoError(format!("Directory read failed {:?}: {}", src_dir, e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| ExportError::IoError(e.to_string()))?;
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // --- SKIP HEAVY BUILD ARTIFACTS & CACHES ---
            if path.is_dir()
                && (name_str == "target"
                    || name_str == "node_modules"
                    || name_str == ".git"
                    || name_str == "dist"
                    || name_str == ".dmz")
            {
                continue;
            }

            if skip_manifest && name_str == "dmz-workspace.toml" {
                continue;
            }

            let rel_dest = dest_prefix.join(name);

            if path.is_dir() {
                Self::append_dir_all(tar, &path, &rel_dest, skip_manifest)?;
            } else {
                tar.append_path_with_name(&path, &rel_dest).map_err(|e| {
                    ExportError::ArchiveError(format!("Failed to add file {:?}: {}", path, e))
                })?;
            }
        }

        Ok(())
    }

    fn append_filtered_files<W: Write>(
        tar: &mut TarBuilder<W>,
        workspace_root: &Path,
        files: &HashSet<PathBuf>,
    ) -> Result<(), ExportError> {
        for rel_path in files {
            // Skip .git, target, node_modules, dist, .dmz
            let starts_with_ignored = rel_path
                .components()
                .next()
                .map(|c| {
                    let s = c.as_os_str().to_string_lossy();
                    s == "target"
                        || s == "node_modules"
                        || s == ".git"
                        || s == "dist"
                        || s == ".dmz"
                })
                .unwrap_or(false);

            if starts_with_ignored {
                continue;
            }

            let full_path = workspace_root.join(rel_path);
            if full_path.is_file() {
                tar.append_path_with_name(&full_path, rel_path)
                    .map_err(|e| {
                        ExportError::ArchiveError(format!(
                            "Failed to add delta file {:?}: {}",
                            rel_path, e
                        ))
                    })?;
            }
        }
        Ok(())
    }

    fn read_manifest_from_archive(path: &Path) -> Result<AirGapManifest, String> {
        let file = File::open(path).map_err(|e| format!("Failed to open since archive: {}", e))?;
        let decoder =
            zstd::stream::read::Decoder::new(file).map_err(|e| format!("Zstd error: {}", e))?;
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().map_err(|e| format!("Tar error: {}", e))? {
            let mut entry = entry.map_err(|e| format!("Tar entry error: {}", e))?;
            let entry_path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
            if entry_path == Path::new("airgap_manifest.json") {
                let mut content = String::new();
                entry
                    .read_to_string(&mut content)
                    .map_err(|e| format!("Read error: {}", e))?;
                let manifest: AirGapManifest = serde_json::from_str(&content)
                    .map_err(|e| format!("JSON parse error: {}", e))?;
                return Ok(manifest);
            }
        }
        Err("airgap_manifest.json not found in archive".to_string())
    }

    pub fn calculate_file_sha256(path: &Path) -> Result<String, std::io::Error> {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];

        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }

        Ok(format!("{:x}", hasher.finalize()))
    }
}
