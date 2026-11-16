use super::backup::{create_backup_snapshot, restore_backup_snapshot};
use super::hydration::{
    hydrate_offline_dependencies, hydrate_offline_dependencies_with_options, relative_path_between,
};
use super::security::{
    is_contained_path, is_safe_relative_path, unpack_tar_entry_contained,
    unpack_tar_entry_contained_bounded,
};
use super::types::{ImportConfig, ImportError, ImportResult};
use crate::archive::exporter::AirGapManifest;
use crate::ecosystem::EcosystemRegistry;
use crate::resolver::lockfile::DmzLockfile;
use crate::vcs::GitProvider;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use tar::Archive as TarArchive;
use tracing::{info, warn};
use zstd::stream::read::Decoder as ZstdDecoder;

pub struct AirGapImporter;

impl AirGapImporter {
    pub const DEFAULT_MAX_FILES: usize = 250_000;
    pub const DEFAULT_MAX_DECOMPRESSED_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GB

    pub fn is_safe_relative_path(path: &Path) -> bool {
        is_safe_relative_path(path)
    }

    pub fn is_contained_path(base_dir: &Path, rel_link: &Path) -> bool {
        is_contained_path(base_dir, rel_link)
    }

    pub fn unpack_tar_entry_contained<R: Read>(
        entry: &mut tar::Entry<R>,
        target_root: &Path,
        rel_path: &Path,
    ) -> Result<(), ImportError> {
        unpack_tar_entry_contained(entry, target_root, rel_path)
    }

    pub fn hydrate_offline_dependencies(
        workspace_root: &Path,
        store_dir: &Path,
        workspace_project: Option<&crate::workspace::WorkspaceProjectConfig>,
    ) {
        hydrate_offline_dependencies(workspace_root, store_dir, workspace_project);
    }

    pub fn relative_path_between(from: &Path, to: &Path) -> String {
        relative_path_between(from, to)
    }

    /// Verifies and unpacks an air-gap bundle into the target store and workspace directory
    pub fn import(config: &ImportConfig) -> Result<ImportResult, ImportError> {
        if !config.target_workspace_dir.exists() {
            let _ = fs::create_dir_all(&config.target_workspace_dir);
        }
        let effective_workspace = if config.target_workspace_dir.is_relative() {
            std::fs::canonicalize(&config.target_workspace_dir)
                .unwrap_or_else(|_| config.target_workspace_dir.clone())
        } else {
            config.target_workspace_dir.clone()
        };

        let effective_store = if config.target_store_dir.is_relative() {
            if effective_workspace.join(&config.target_store_dir).exists() {
                effective_workspace.join(&config.target_store_dir)
            } else if config.target_store_dir.exists() {
                std::fs::canonicalize(&config.target_store_dir)
                    .unwrap_or_else(|_| config.target_store_dir.clone())
            } else {
                effective_workspace.join(&config.target_store_dir)
            }
        } else {
            config.target_store_dir.clone()
        };

        let effective_archive = if config.archive_path.is_relative() {
            if effective_workspace.join(&config.archive_path).exists() {
                effective_workspace.join(&config.archive_path)
            } else if config.archive_path.exists() {
                std::fs::canonicalize(&config.archive_path)
                    .unwrap_or_else(|_| config.archive_path.clone())
            } else {
                config.archive_path.clone()
            }
        } else {
            config.archive_path.clone()
        };

        info!(
            archive = ?effective_archive,
            store = ?effective_store,
            workspace = ?effective_workspace,
            "Starting air-gap archive import"
        );

        if !effective_archive.exists() {
            return Err(ImportError::IoError(format!(
                "Archive file not found: {}",
                effective_archive.display()
            )));
        }

        // Automatic sidecar .sha256 verification (transit integrity check)
        let sidecar_sha256_path = PathBuf::from(format!("{}.sha256", effective_archive.display()));
        if sidecar_sha256_path.exists() {
            info!(
                sidecar = ?sidecar_sha256_path,
                "Found sidecar .sha256 file; verifying physical transport integrity"
            );
            let sidecar_raw = fs::read_to_string(&sidecar_sha256_path).map_err(|e| {
                ImportError::IoError(format!(
                    "Failed to read checksum sidecar {:?}: {}",
                    sidecar_sha256_path, e
                ))
            })?;
            let expected_sha256 = sidecar_raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if !expected_sha256.is_empty() {
                let actual_sha256 =
                    crate::archive::AirGapExporter::calculate_file_sha256(&effective_archive)
                        .map_err(|e| {
                            ImportError::IoError(format!(
                                "Failed to compute archive checksum: {}",
                                e
                            ))
                        })?;
                if actual_sha256.to_lowercase() != expected_sha256 {
                    return Err(ImportError::VerificationFailed(format!(
                        "Archive transport checksum mismatch! Sidecar expected {}, calculated {}",
                        expected_sha256, actual_sha256
                    )));
                }
                info!(
                    checksum = %actual_sha256,
                    "Physical transit archive checksum verified against .sha256 sidecar"
                );
            }
        }

        // Prepare target directories
        if !effective_store.exists() {
            fs::create_dir_all(&effective_store).map_err(|e| {
                ImportError::IoError(format!("Failed to create target store: {}", e))
            })?;
        }

        if !effective_workspace.exists() {
            fs::create_dir_all(&effective_workspace).map_err(|e| {
                ImportError::IoError(format!("Failed to create target workspace: {}", e))
            })?;
        }

        // Pre-import workspace backup snapshot
        let backup_snapshot = create_backup_snapshot(&effective_workspace)?;

        let result = Self::do_import(
            &effective_archive,
            &effective_store,
            &effective_workspace,
            config.verify_closure_hash,
            config.include_dev,
            config.install_dependencies,
        );

        match result {
            Ok(res) => Ok(res),
            Err(e) => {
                if let Some(ref backup_dir) = backup_snapshot {
                    let _ = restore_backup_snapshot(&effective_workspace, backup_dir);
                }
                Err(e)
            }
        }
    }

    fn do_import(
        effective_archive: &Path,
        effective_store: &Path,
        effective_workspace: &Path,
        verify_closure_hash: bool,
        include_dev: bool,
        install_dependencies: bool,
    ) -> Result<ImportResult, ImportError> {
        // Open archive and initialize zstd stream
        let file = File::open(effective_archive)
            .map_err(|e| ImportError::IoError(format!("Failed to open archive: {}", e)))?;

        let zstd_decoder = ZstdDecoder::new(file).map_err(|e| {
            ImportError::ExtractionError(format!("Failed to initialize zstd decoder: {}", e))
        })?;

        let mut archive = TarArchive::new(zstd_decoder);

        let mut closure_signature = String::new();
        let mut imported_count: usize = 0;
        let mut is_delta = false;
        let mut has_git_history = false;
        let mut vcs_provenance = None;
        let mut manifest_toolchains = Vec::new();
        let mut parsed_lockfile: Option<DmzLockfile> = None;
        let mut workspace_project = None;
        let mut extracted_store_files: Vec<(PathBuf, PathBuf)> = Vec::new();

        // Single-pass entry iteration, extraction, and verification
        let mut file_count: usize = 0;
        let mut total_bytes: u64 = 0;

        let entries = archive.entries().map_err(|e| {
            ImportError::ExtractionError(format!("Failed to read tar entries: {}", e))
        })?;

        for entry_res in entries {
            let mut entry = entry_res
                .map_err(|e| ImportError::ExtractionError(format!("Tar entry error: {}", e)))?;

            file_count += 1;
            if file_count > Self::DEFAULT_MAX_FILES {
                return Err(ImportError::ExtractionError(format!(
                    "Decompression quota exceeded: maximum file count limit ({}) reached",
                    Self::DEFAULT_MAX_FILES
                )));
            }
            if total_bytes.saturating_add(entry.size()) > Self::DEFAULT_MAX_DECOMPRESSED_BYTES {
                return Err(ImportError::ExtractionError(format!(
                    "Decompression quota exceeded: maximum byte limit ({} bytes) reached",
                    Self::DEFAULT_MAX_DECOMPRESSED_BYTES
                )));
            }
            let remaining_quota = Self::DEFAULT_MAX_DECOMPRESSED_BYTES.saturating_sub(total_bytes);

            let path = entry
                .path()
                .map_err(|e| ImportError::ExtractionError(e.to_string()))?
                .to_path_buf();

            if path == Path::new("airgap_manifest.json") {
                const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
                let cap = remaining_quota.min(MAX_MANIFEST_BYTES);
                let mut content = String::new();
                let n = (&mut entry)
                    .take(cap.saturating_add(1))
                    .read_to_string(&mut content)
                    .map_err(|e| {
                        ImportError::ExtractionError(format!("Failed to read manifest: {}", e))
                    })?;
                if (n as u64) > cap {
                    return Err(ImportError::ExtractionError(
                        "airgap_manifest.json exceeds maximum permitted size".to_string(),
                    ));
                }
                total_bytes = total_bytes.saturating_add(n as u64);

                let manifest: AirGapManifest = serde_json::from_str(&content).map_err(|e| {
                    ImportError::VerificationFailed(format!("Invalid airgap_manifest.json: {}", e))
                })?;

                closure_signature = manifest.closure_signature;
                is_delta = manifest.delta_base.is_some();
                has_git_history = manifest.has_git_history;
                vcs_provenance = manifest.vcs.clone();
                manifest_toolchains = manifest.toolchains.clone();
                workspace_project = manifest.workspace_project.clone();

                info!(
                    closure = %closure_signature,
                    version = %manifest.dmz_version,
                    target_platform = ?manifest.target_platform,
                    is_delta = is_delta,
                    has_git = has_git_history,
                    "Air-gap manifest verified"
                );

                // Delta Base Verification
                if let Some(ref delta) = manifest.delta_base {
                    info!(
                        base_commit = ?delta.base_commit,
                        base_closure = ?delta.base_closure_signature,
                        "Verifying target workspace matches delta base"
                    );

                    if let Some(ref expected_closure) = delta.base_closure_signature {
                        let ws_lock = effective_workspace.join("dmz.lock");
                        if ws_lock.exists() {
                            if let Ok(lock) = DmzLockfile::load_and_verify(&ws_lock) {
                                if &lock.closure_signature != expected_closure {
                                    return Err(ImportError::VerificationFailed(format!(
                                        "Delta base mismatch! Expected closure signature {}, but workspace has {}",
                                        expected_closure, lock.closure_signature
                                    )));
                                }
                            }
                        }
                    }

                    if let Some(ref expected_commit) = delta.base_commit {
                        if effective_workspace.join(".git").exists() {
                            if let Ok(Some(prov)) = GitProvider::get_provenance(effective_workspace)
                            {
                                if !prov.commit.starts_with(expected_commit)
                                    && !expected_commit.starts_with(&prov.commit)
                                {
                                    return Err(ImportError::VerificationFailed(format!(
                                        "Delta base mismatch! Expected git commit {}, but workspace has {}",
                                        expected_commit, prov.commit
                                    )));
                                }
                            }
                        }
                    }
                }

                if let Some(target) = &manifest.target_platform {
                    let host_platform =
                        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
                    if target != &host_platform {
                        warn!(
                            bundle_target = %target,
                            host_platform = %host_platform,
                            "Bundle target platform does not match current host platform!"
                        );
                    } else {
                        info!(
                            platform = %host_platform,
                            "Bundle target platform matches host platform"
                        );
                    }
                }

                let dest = effective_workspace.join(&path);
                crate::workspace::atomic_write_file(&dest, &content)
                    .map_err(|e| ImportError::IoError(e.to_string()))?;
            } else if path == Path::new("dmz.lock") {
                let cap = remaining_quota.min(crate::resolver::MAX_LOCKFILE_SIZE);
                let mut content = String::new();
                let n = (&mut entry)
                    .take(cap.saturating_add(1))
                    .read_to_string(&mut content)
                    .map_err(|e| {
                        ImportError::ExtractionError(format!("Failed to read lockfile: {}", e))
                    })?;
                if (n as u64) > cap {
                    return Err(ImportError::VerificationFailed(
                        "Embedded dmz.lock exceeds maximum permitted lockfile size".to_string(),
                    ));
                }
                total_bytes = total_bytes.saturating_add(n as u64);

                let lockfile: DmzLockfile = serde_json::from_str(&content).map_err(|e| {
                    ImportError::VerificationFailed(format!("Invalid embedded dmz.lock: {}", e))
                })?;

                if verify_closure_hash {
                    let computed = DmzLockfile::compute_closure_hash(&lockfile.packages);
                    if computed != lockfile.closure_signature {
                        return Err(ImportError::VerificationFailed(format!(
                            "Closure hash mismatch! Lockfile signature: {}, Computed: {}",
                            lockfile.closure_signature, computed
                        )));
                    }
                }

                let dest = effective_workspace.join(&path);
                crate::workspace::atomic_write_file(&dest, &content)
                    .map_err(|e| ImportError::IoError(e.to_string()))?;
                parsed_lockfile = Some(lockfile);
            } else if path == Path::new("workspace.bundle") {
                // Extract Git bundle into workspace and initialize / link repository
                let bundle_dest = effective_workspace.join("workspace.bundle");
                let written = unpack_tar_entry_contained_bounded(
                    &mut entry,
                    effective_workspace,
                    Path::new("workspace.bundle"),
                    remaining_quota,
                )?;
                total_bytes = total_bytes.saturating_add(written);
                info!(path = ?bundle_dest, "Unpacked embedded git bundle");

                if let Err(e) = GitProvider::restore_bundle(effective_workspace, &bundle_dest) {
                    warn!(
                        "Could not automatically restore git repository from bundle: {}",
                        e
                    );
                } else {
                    info!("Successfully restored offline Git repository from bundle");
                }
            } else if path.starts_with("store") {
                let rel_path = path.strip_prefix("store").unwrap_or(&path);
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                let written = unpack_tar_entry_contained_bounded(
                    &mut entry,
                    effective_store,
                    rel_path,
                    remaining_quota,
                )?;
                total_bytes = total_bytes.saturating_add(written);
                let unpacked_abs = effective_store.join(rel_path);
                if unpacked_abs.is_file() {
                    extracted_store_files.push((rel_path.to_path_buf(), unpacked_abs));
                }
                imported_count += 1;
            } else if path.starts_with("toolchains") || path.starts_with(".dmz/toolchains") {
                let rel_path = path
                    .strip_prefix(".dmz/toolchains")
                    .or_else(|_| path.strip_prefix("toolchains"))
                    .unwrap_or(&path);
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                let tc_root = effective_workspace.join(".dmz/toolchains");
                let written = unpack_tar_entry_contained_bounded(
                    &mut entry,
                    &tc_root,
                    rel_path,
                    remaining_quota,
                )?;
                total_bytes = total_bytes.saturating_add(written);
            } else {
                // Workspace source files (Cargo.toml, src/, etc.)
                let written = unpack_tar_entry_contained_bounded(
                    &mut entry,
                    effective_workspace,
                    &path,
                    remaining_quota,
                )?;
                total_bytes = total_bytes.saturating_add(written);
            }
        }

        // Prune any store files extracted from the archive that are not referenced by dmz.lock
        if let Some(ref lockfile) = parsed_lockfile {
            if !extracted_store_files.is_empty() {
                let mut expected_store_files: std::collections::HashSet<PathBuf> =
                    std::collections::HashSet::new();
                for pkg in lockfile.packages.values() {
                    if pkg.is_local_path() {
                        continue;
                    }
                    if let Ok(artifact_path) =
                        crate::install::locate_locked_store_artifact(effective_store, pkg)
                    {
                        if let Ok(rel) = artifact_path.strip_prefix(effective_store) {
                            expected_store_files.insert(rel.to_path_buf());
                        }
                    } else {
                        let safe_name = pkg.name.replace('/', "__");
                        if effective_store.join(&safe_name).is_file() {
                            expected_store_files.insert(PathBuf::from(safe_name));
                        }
                    }
                }

                for (rel_path, abs_path) in extracted_store_files {
                    if !expected_store_files.contains(&rel_path)
                        && abs_path.is_file()
                        && fs::remove_file(&abs_path).is_ok()
                    {
                        imported_count = imported_count.saturating_sub(1);
                        let mut cur = abs_path.parent();
                        while let Some(dir) = cur {
                            if dir == effective_store || !dir.starts_with(effective_store) {
                                break;
                            }
                            if fs::read_dir(dir)
                                .map(|mut it| it.next().is_none())
                                .unwrap_or(false)
                            {
                                let _ = fs::remove_dir(dir);
                                cur = dir.parent();
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Root Manifest Dominance: Clean up or ignore orphaned / unrequested toolchain stubs
        // across all 14 ecosystems that may have been bundled inadvertently or left over from previous imports.
        let declared_ecosystems =
            crate::toolchain::ToolchainProvider::detect_workspace_ecosystems_with_options(
                effective_workspace,
                parsed_lockfile.as_ref(),
                crate::toolchain::ToolchainDetectOptions { include_dev },
            );
        for eco in crate::toolchain::ToolchainEcosystem::all() {
            if !declared_ecosystems.contains(eco) {
                let stray_tc = effective_workspace
                    .join(".dmz/toolchains")
                    .join(eco.as_str());
                if stray_tc.exists() {
                    if let Err(e) = fs::remove_dir_all(&stray_tc) {
                        warn!(
                            "Failed to purge unrequested {} toolchain directory: {}",
                            eco.as_str(),
                            e
                        );
                    } else {
                        info!(
                            "Purged unrequested {} toolchain directory to preserve 1-to-1 workspace isolation",
                            eco.as_str()
                        );
                    }
                }
            }
        }

        let toolchains_dir = effective_workspace.join(".dmz/toolchains");
        if toolchains_dir.exists()
            && fs::read_dir(&toolchains_dir)
                .map(|mut i| i.next().is_none())
                .unwrap_or(false)
        {
            let _ = fs::remove_dir(&toolchains_dir);
        }
        let has_toolchains = toolchains_dir.exists()
            && fs::read_dir(&toolchains_dir)
                .map(|mut i| i.next().is_some())
                .unwrap_or(false);

        // Setup imported toolchain permissions, quarantine flags, activator scripts, and IDE settings.
        // Always invoked so `.vscode/settings.json` is generated even when `has_toolchains` is false.
        if let Err(e) =
            crate::toolchain::ToolchainPackager::setup_imported_toolchains(effective_workspace)
        {
            warn!(
                "Failed to finalize imported toolchains configuration: {}",
                e
            );
        } else if has_toolchains {
            info!("Successfully materialized and activated bundled toolchains in .dmz/toolchains");
        }

        // Verify that every declared toolchain in airgap_manifest.json was extracted with valid binaries
        if verify_closure_hash {
            for tc_entry in &manifest_toolchains {
                if tc_entry.ecosystem.is_host_prerequisite() {
                    continue;
                }
                if declared_ecosystems.contains(&tc_entry.ecosystem) {
                    let tc_root = effective_workspace
                        .join(".dmz/toolchains")
                        .join(tc_entry.ecosystem.as_str());
                    if !tc_root.exists() {
                        return Err(ImportError::VerificationFailed(format!(
                            "Manifest declares bundled '{}' toolchain ({}), but {:?} was not extracted",
                            tc_entry.ecosystem.as_str(),
                            tc_entry.version,
                            tc_root
                        )));
                    }
                    for bin_name in &tc_entry.binaries {
                        let in_bin = tc_root.join("bin").join(bin_name).is_file();
                        let in_root = tc_root.join(bin_name).is_file();
                        let via_finder = crate::toolchain::ToolchainEnv::find_binary(
                            &tc_root,
                            &format!("bin/{}", bin_name),
                        )
                        .is_some();
                        if !in_bin && !in_root && !via_finder {
                            return Err(ImportError::VerificationFailed(format!(
                                "Bundled '{}' toolchain is missing declared binary '{}' in {:?}",
                                tc_entry.ecosystem.as_str(),
                                bin_name,
                                tc_root
                            )));
                        }
                    }
                }
            }
        }

        // Automatically discover workspace project manifest if not already populated from airgap_manifest.json
        if workspace_project.is_none() {
            if let Some((discovered, _)) =
                crate::workspace::WorkspaceProjectConfig::discover_workspace_manifest_scoped(
                    effective_workspace,
                )
            {
                workspace_project = Some(discovered);
            }
        }

        // Materialize offline dependencies (.venv, node_modules, .cargo/vendor) from the local store
        // BEFORE generating final IDE settings, hydrating workspace configurations, or checking optional host SDKs.
        let lockfile_path = effective_workspace.join("dmz.lock");
        let mut installed_packages = 0usize;
        let mut install_warning = None;
        if install_dependencies && lockfile_path.is_file() && effective_store.is_dir() {
            let installable_pkgs: Vec<&crate::resolver::lockfile::LockedPackage> = parsed_lockfile
                .as_ref()
                .map(|l| l.packages.values().filter(|p| !p.is_local_path()).collect())
                .unwrap_or_default();
            let store_has_cached = imported_count > 0
                || installable_pkgs.iter().any(|pkg| {
                    crate::install::locate_locked_store_artifact(effective_store, pkg).is_ok()
                });
            if !installable_pkgs.is_empty() && store_has_cached {
                let install_cfg = crate::install::InstallConfig {
                    workspace_path: effective_workspace.to_path_buf(),
                    lockfile_path,
                    store_dir: effective_store.to_path_buf(),
                };
                match crate::install::InstallerEngine::install(&install_cfg) {
                    Ok(count) => {
                        installed_packages = count;
                    }
                    Err(e) => {
                        if verify_closure_hash {
                            return Err(ImportError::VerificationFailed(format!(
                                "Post-extraction dependency install failed: {}",
                                e
                            )));
                        }
                        let msg = e.to_string();
                        warn!(
                            error = %msg,
                            "Post-extraction store install skipped or deferred (verify_closure_hash=false)"
                        );
                        install_warning = Some(msg);
                    }
                }
            }
        }

        // Hydrate offline dependencies: Tauri/npm CLI shims, Cargo vendored sources, and scoped Python settings
        let resolved_include_dev = parsed_lockfile
            .as_ref()
            .and_then(|l| l.include_dev)
            .unwrap_or(include_dev);
        hydrate_offline_dependencies_with_options(
            effective_workspace,
            effective_store,
            workspace_project.as_ref(),
            resolved_include_dev,
        );

        // Non-blocking macOS host SDK/CLT check (zero GUI popup): import and node_modules/.venv
        // materialization never abort on missing CLT; surface advisory only when native build tools are absent.
        if !crate::toolchain::has_usable_apple_sdk_or_clt(effective_workspace) {
            if let Err(e) =
                crate::toolchain::verify_apple_prereq_for_ecosystems(&declared_ecosystems)
            {
                let note = e.to_string();
                warn!(warning = %note, "macOS host prerequisite notice after import");
                install_warning = Some(match install_warning {
                    Some(prev) => format!("{prev}; {note}"),
                    None => note,
                });
            }
        }

        // Re-synchronize IDE settings and .code-workspace after .venv and node_modules are materialized
        if let Err(e) = crate::toolchain::ToolchainSetup::generate_ide_settings(effective_workspace)
        {
            warn!(error = %e, "Failed to update IDE settings after import");
        }

        // Exported .code-workspace files embed interpreter paths from the source machine; drop or
        // redirect any that don't resolve on this machine (VS Code errors on dangling paths).
        crate::workspace::interpreter::sanitize_code_workspace_interpreters(effective_workspace);
        crate::workspace::interpreter::sanitize_vscode_settings_interpreters(
            effective_workspace,
            effective_workspace,
        );
        if let Some(wp) = workspace_project.as_ref() {
            for member in &wp.members {
                let member_dir = effective_workspace.join(&member.path);
                if member_dir.is_dir() {
                    crate::workspace::interpreter::sanitize_vscode_settings_interpreters(
                        &member_dir,
                        effective_workspace,
                    );
                }
            }
        }

        // Run post-import ecosystem validation using the registry
        let registry = EcosystemRegistry::new();
        match registry.process_workspace(effective_workspace) {
            Ok(stacks) => {
                info!(active_stacks = ?stacks, "Post-import ecosystem discovery complete");
            }
            Err(e) => {
                info!(error = %e, "Ecosystem post-processing check note");
            }
        }

        info!(
            imported_files = imported_count,
            closure = %closure_signature,
            is_delta = is_delta,
            has_git = has_git_history,
            has_toolchains = has_toolchains,
            "Air-gap import completed successfully"
        );

        Ok(ImportResult {
            imported_packages: imported_count,
            installed_packages,
            install_warning,
            closure_signature,
            store_path: effective_store.to_path_buf(),
            workspace_path: effective_workspace.to_path_buf(),
            is_delta,
            has_git_history,
            has_toolchains,
            workspace_project,
            vcs: vcs_provenance,
        })
    }
}
