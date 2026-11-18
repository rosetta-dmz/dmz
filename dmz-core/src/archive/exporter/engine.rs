use super::hashing::{calculate_file_sha256 as hash_file_sha256, HashingWriter};
use super::multi_workspace::package_multi_workspace_umbrella;
use super::packaging::{
    append_dir_all, append_file_normalized, append_filtered_files, read_manifest_from_archive,
    resolve_deterministic_epoch,
};
use super::types::{AirGapManifest, DeltaBaseInfo, ExportConfig, ExportError, ExportMode};
use crate::resolver::lockfile::DmzLockfile;
use crate::vcs::{GitProvider, VcsProvenance};
use crate::workspace::WorkspaceProjectConfig;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::{info, warn};
use zstd::stream::write::Encoder as ZstdEncoder;

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
            info!(
                commit = %prov.commit,
                branch = ?prov.branch,
                dirty = prov.dirty,
                "Detected workspace VCS provenance"
            );
        }

        // Parse Delta Baseline if `--since` was requested
        let mut delta_base = None;
        let mut changed_files_filter: Option<HashSet<PathBuf>> = None;

        if let Some(since_spec) = &config.since {
            let since_path = Path::new(since_spec);
            if since_path.exists() {
                if since_spec.ends_with(".zst") || since_spec.ends_with(".tar.zst") {
                    if let Ok(prev_manifest) = read_manifest_from_archive(since_path) {
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
                    info!(
                        base = %base_commit,
                        count = files.len(),
                        "Computed changed files since git ref"
                    );
                    changed_files_filter = Some(files.into_iter().collect());
                }
                delta_base = Some(DeltaBaseInfo {
                    base_commit: Some(base_commit),
                    base_archive_sha256: None,
                    base_closure_signature: None,
                });
            }
        }

        // Verify dmz.lock integrity before exporting
        let mut lockfile = DmzLockfile::load_and_verify(&effective_lockfile)
            .map_err(|e| ExportError::LockfileError(e.to_string()))?;

        // Detect Multi-Workspace Configuration early so export can verify lockfile completeness
        // against declared Cargo manifests/Cargo.lock (matching Nix/Bazel completeness invariants).
        let (ws_project_opt, manifest_path_opt) = match config.multi_workspace.clone() {
            Some(cfg) => (Some(cfg), None),
            None => WorkspaceProjectConfig::discover_workspace_manifest_scoped(&workspace_root)
                .map(|(cfg, p)| (Some(cfg), Some(p)))
                .unwrap_or((None, None)),
        };

        if (config.mode == ExportMode::Unified || config.mode == ExportMode::DepsOnly)
            && config.target_platform.is_none()
            && lockfile.target_platform.is_none()
        {
            let effective_include_dev = lockfile.include_dev.unwrap_or(config.include_dev);
            if let Some(missing_msg) = lockfile.find_missing_cargo_requirement(
                &workspace_root,
                None,
                effective_include_dev,
                ws_project_opt.as_ref(),
            ) {
                return Err(ExportError::LockfileError(format!(
                    "{}. Run 'dmz bundle' / 'Scan & Resolve' before exporting.",
                    missing_msg
                )));
            }
        }

        // Synchronize lockfile VCS provenance if not already set
        if lockfile.vcs.is_none() && vcs_provenance.is_some() {
            lockfile.vcs = vcs_provenance.clone();
            if let Err(e) = lockfile.save(&effective_lockfile) {
                warn!(
                    error = %e,
                    path = ?effective_lockfile,
                    "Failed to persist VCS provenance to dmz.lock before export"
                );
            }
        }

        // Ensure parent output directory exists
        if let Some(parent) = effective_output.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    ExportError::IoError(format!("Failed to create parent dir: {}", e))
                })?;
            }
        }

        // Compute reproducible timestamp (SOURCE_DATE_EPOCH -> git commit time -> 1980-01-01 DOS epoch)
        let deterministic_epoch =
            resolve_deterministic_epoch(&workspace_root, vcs_provenance.as_ref());
        info!(
            epoch = deterministic_epoch,
            "Configured reproducible archive timestamp"
        );

        // Prepare archive output stream with buffered I/O, streaming SHA-256 calculation, and multithreaded zstd
        let file = File::create(&effective_output)
            .map_err(|e| ExportError::IoError(format!("Failed to create export archive: {}", e)))?;
        let buf_file = BufWriter::with_capacity(128 * 1024, file);
        let hashing_writer = HashingWriter::new(buf_file);

        let mut zstd_encoder = ZstdEncoder::new(hashing_writer, config.zstd_level)
            .map_err(|e| ExportError::ArchiveError(format!("Failed to initialize zstd: {}", e)))?;

        let workers = std::thread::available_parallelism()
            .map(|n| u32::try_from(n.get()).unwrap_or(16))
            .unwrap_or(4)
            .clamp(1, 16);
        let _ = zstd_encoder.multithread(workers);

        let mut tar_builder = TarBuilder::new(zstd_encoder);

        // Append lockfile with normalized header
        append_file_normalized(
            &mut tar_builder,
            &effective_lockfile,
            "dmz.lock",
            deterministic_epoch,
        )?;

        // Append store items (if mode is Unified or DepsOnly)
        let mut packed_count = 0;
        let mut packed_pypi_artifacts: Vec<(String, PathBuf)> = Vec::new();
        if config.mode == ExportMode::Unified || config.mode == ExportMode::DepsOnly {
            let mut packed_store_rel_paths: HashSet<PathBuf> = HashSet::new();
            for pkg_info in lockfile.packages.values() {
                // Skip local path dependencies/workspace members since they are in the workspace source
                if pkg_info.is_local_path() {
                    continue;
                }

                let safe_pkg_name = pkg_info.name.replace('/', "__");
                let resolved_artifact = match crate::install::locate_locked_store_artifact(
                    &effective_store,
                    pkg_info,
                ) {
                    Ok(path) => path,
                    Err(crate::install::InstallError::ArtifactMissing { path, .. }) => {
                        let flat_fallback = effective_store.join(&safe_pkg_name);
                        if flat_fallback.is_file() {
                            flat_fallback
                        } else {
                            return Err(ExportError::ArchiveError(format!(
                                    "Missing store artifact for locked package '{}@{}' at {:?}. Run 'dmz fetch' before exporting.",
                                    pkg_info.name, pkg_info.version, path
                                )));
                        }
                    }
                    Err(e) => {
                        return Err(ExportError::ArchiveError(e.to_string()));
                    }
                };

                if pkg_info.sha256.len() == 64
                    && pkg_info.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    let computed_sha = hash_file_sha256(&resolved_artifact)
                        .map_err(|e| ExportError::IoError(e.to_string()))?;
                    if computed_sha != pkg_info.sha256 {
                        return Err(ExportError::ArchiveError(format!(
                            "Store artifact checksum mismatch for '{}@{}': expected {}, got {}",
                            pkg_info.name, pkg_info.version, pkg_info.sha256, computed_sha
                        )));
                    }
                }

                let rel_in_store = resolved_artifact
                    .strip_prefix(&effective_store)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| {
                        let fname = resolved_artifact
                            .file_name()
                            .map(|f| f.to_os_string())
                            .unwrap_or_else(|| {
                                format!("{}-{}.archive", safe_pkg_name, pkg_info.version).into()
                            });
                        PathBuf::from(&safe_pkg_name).join(fname)
                    });

                if !packed_store_rel_paths.insert(rel_in_store.clone()) {
                    continue;
                }

                let archive_dest = Path::new("store").join(&rel_in_store);
                let normalized_dest = archive_dest.to_string_lossy().replace('\\', "/");
                append_file_normalized(
                    &mut tar_builder,
                    &resolved_artifact,
                    &normalized_dest,
                    deterministic_epoch,
                )?;
                if pkg_info.ecosystem == crate::resolver::manifest::Ecosystem::PyPI {
                    packed_pypi_artifacts.push((pkg_info.name.clone(), resolved_artifact));
                }
                packed_count += 1;
            }
        }

        let mut final_ws_project = None;

        // Append workspace source code (if mode is Unified or CodeOnly), or embed normalized
        // `dmz-workspace.toml` for DepsOnly multi-workspace archives so imports retain member routing.
        if config.mode == ExportMode::Unified || config.mode == ExportMode::CodeOnly {
            if let Some(ref ws_project) = ws_project_opt {
                // --- MULTI-WORKSPACE UMBRELLA EXPORT ---
                let normalized_project = package_multi_workspace_umbrella(
                    &mut tar_builder,
                    &workspace_root,
                    ws_project,
                    manifest_path_opt.as_deref(),
                    changed_files_filter.as_ref(),
                    &lockfile,
                    deterministic_epoch,
                    config.with_toolchain,
                )?;
                final_ws_project = Some(normalized_project);
            } else {
                // --- SINGLE-WORKSPACE FLAT EXPORT (Backward Compatibility) ---
                info!(
                    path = ?workspace_root,
                    "Appending workspace source code to archive"
                );
                if workspace_root.is_dir() {
                    if let Some(ref changed_files) = changed_files_filter {
                        info!(
                            changed_count = changed_files.len(),
                            "Packaging incremental delta workspace files"
                        );
                        append_filtered_files(
                            &mut tar_builder,
                            &workspace_root,
                            changed_files,
                            deterministic_epoch,
                        )?;
                    } else {
                        append_dir_all(
                            &mut tar_builder,
                            &workspace_root,
                            Path::new(""),
                            false,
                            deterministic_epoch,
                        )?;
                    }
                }
            }
        } else if let Some(ref ws_project) = ws_project_opt {
            let layouts =
                ws_project.resolve_member_layouts(&workspace_root, manifest_path_opt.as_deref());
            let mut normalized_project = ws_project.clone();
            for (idx, m) in normalized_project.members.iter_mut().enumerate() {
                if let Some(layout) = layouts.get(idx) {
                    m.path = layout.archive_path.clone();
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
                        ExportError::ArchiveError(format!(
                            "Failed to embed dmz-workspace.toml in DepsOnly archive: {}",
                            e
                        ))
                    })?;
            }
            final_ws_project = Some(normalized_project);
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
                    append_file_normalized(
                        &mut tar_builder,
                        &temp_bundle,
                        "workspace.bundle",
                        deterministic_epoch,
                    )?;
                    has_git_history = true;
                }
                Err(e) => {
                    warn!("Could not create git bundle for workspace: {}", e);
                }
            }
        }

        let host_platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let target_platform = config
            .target_platform
            .clone()
            .or_else(|| lockfile.target_platform.clone())
            .or_else(|| config.with_toolchain.then_some(host_platform));

        if let Some(note) = crate::toolchain::export_apple_prereq_note(
            &workspace_root,
            Some(&lockfile),
            target_platform.as_deref(),
        ) {
            info!("{}", note);
        }

        // Package active host toolchains if requested (`--with-toolchain`)
        let mut toolchain_manifest_entries = Vec::new();
        if config.with_toolchain {
            info!("Initiating host toolchain discovery and packaging");
            match crate::toolchain::ToolchainPackager::package_toolchains_with_options_and_project_checked(
                &workspace_root,
                Some(&lockfile),
                ws_project_opt.as_ref(),
                &mut tar_builder,
                deterministic_epoch,
                crate::toolchain::ToolchainDetectOptions {
                    include_dev: config.include_dev,
                },
                config.toolchain_profile,
                target_platform.as_deref(),
                config.allow_missing_toolchains,
            ) {
                Ok(entries) => {
                    info!(
                        "Captured and packaged language toolchains into bundle count={}",
                        entries.len()
                    );
                    toolchain_manifest_entries = entries;
                }
                Err(e) => {
                    drop(tar_builder);
                    let _ = fs::remove_file(&effective_output);
                    return Err(ExportError::ArchiveError(format!(
                        "Toolchain packaging failed: {}",
                        e
                    )));
                }
            }
        }

        if let Err(err) = validate_exported_python_closure(
            &workspace_root,
            ws_project_opt.as_ref(),
            &toolchain_manifest_entries,
            &packed_pypi_artifacts,
            target_platform.as_deref(),
        ) {
            drop(tar_builder);
            let _ = fs::remove_file(&effective_output);
            return Err(err);
        }

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
            created_at_utc: deterministic_epoch,
            archive_sha256: String::new(), // Populated post-compression
        };

        let manifest_json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| ExportError::ArchiveError(e.to_string()))?;

        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_json.len() as u64);
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
                "airgap_manifest.json",
                manifest_json.as_bytes(),
            )
            .map_err(|e| {
                ExportError::ArchiveError(format!("Failed to embed airgap manifest: {}", e))
            })?;

        // Finish streams and finalize the single-pass cryptographic archive hash (zero disk re-reading)
        let archive_sha256 = {
            let zstd_encoder = tar_builder.into_inner().map_err(|e| {
                ExportError::ArchiveError(format!("Failed to finalize tar stream: {}", e))
            })?;

            let mut hashing_writer = zstd_encoder
                .finish()
                .map_err(|e| format!("Failed to finish zstd compression: {}", e))
                .map_err(ExportError::ArchiveError)?;

            hashing_writer
                .flush()
                .map_err(|e| ExportError::IoError(e.to_string()))?;

            let (mut inner_buf, computed_hash) = hashing_writer.finalize_hash();
            inner_buf
                .flush()
                .map_err(|e| ExportError::IoError(e.to_string()))?;

            computed_hash
        };

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
            info!(
                sidecar = ?sidecar_path,
                "Wrote .sha256 sidecar checksum file"
            );
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

    /// Resolves reproducible timestamp: SOURCE_DATE_EPOCH env var -> Git commit timestamp -> DOS/Bazel epoch (1980-01-01)
    pub fn resolve_deterministic_epoch(workspace_root: &Path, vcs: Option<&VcsProvenance>) -> u64 {
        resolve_deterministic_epoch(workspace_root, vcs)
    }

    /// Computes streaming SHA-256 for a file
    pub fn calculate_file_sha256(path: &Path) -> Result<String, std::io::Error> {
        hash_file_sha256(path)
    }
}

fn validate_exported_python_closure(
    workspace_root: &Path,
    ws_project_opt: Option<&WorkspaceProjectConfig>,
    toolchain_entries: &[crate::toolchain::ToolchainManifestEntry],
    packed_pypi_artifacts: &[(String, PathBuf)],
    target_platform: Option<&str>,
) -> Result<(), ExportError> {
    if packed_pypi_artifacts.is_empty() {
        return Ok(());
    }

    let bundled_py_abi = toolchain_entries
        .iter()
        .find(|tc| tc.ecosystem == crate::toolchain::ToolchainEcosystem::Python)
        .and_then(|tc| crate::toolchain::PythonTargetAbi::parse_runtime_spec(&tc.version));

    let target_abi = bundled_py_abi.or_else(|| {
        let member_roots = crate::toolchain::ToolchainDetector::collect_workspace_member_dirs(
            workspace_root,
            ws_project_opt,
        );
        crate::toolchain::discovery::python::resolve_workspace_python_minor(
            Some(workspace_root),
            &member_roots,
        )
        .map(|(maj, min)| crate::toolchain::PythonTargetAbi::new(maj, min, false))
    });

    let (target_os, _) = crate::workspace::resolve_target_os_arch(target_platform);
    crate::toolchain::PythonTargetAbi::verify_bundle_python_closure(
        packed_pypi_artifacts
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path())),
        target_abi,
        target_os,
    )
    .map_err(ExportError::ArchiveError)
}
