use crate::cli::DEFAULT_LOCKFILE_PATH;
use dmz_core::resolver::ResolverEngine;
use std::path::PathBuf;
use tracing::{error, info, warn};

/// Scan workspace manifests and generate a deterministic dmz.lock closure
pub fn run_resolve(
    workspace: PathBuf,
    sidecar: Option<PathBuf>,
    output: PathBuf,
    target: Option<String>,
    include_dev: bool,
) -> anyhow::Result<i32> {
    info!("Resolving workspace dependencies at {:?}", workspace);
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    let effective_output =
        super::resolve_workspace_relative_path(&ws_canon, &output, DEFAULT_LOCKFILE_PATH);
    let options = dmz_core::resolver::ResolveOptions {
        target_platform: target,
        include_dev,
    };

    if let Some(ref sc_path) = sidecar {
        let sc_canon = std::fs::canonicalize(sc_path).unwrap_or_else(|_| sc_path.clone());
        info!(
            "Dual-repo workspace mode active with sidecar at {:?}",
            sc_canon
        );

        // Run ABI and dynamic library diagnostic check
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(&sc_canon);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }

        // Socket path limit advisory
        let sock_path = dmz_core::workspace::safe_socket_path(&ws_canon, "sidecar");
        if let Some(posix_warn) = dmz_core::workspace::check_socket_path_length(&sock_path) {
            eprintln!("\x1b[33m[DMZ POSIX ADVISORY]\x1b[0m {}", posix_warn);
        }

        let normalized_sc_path = dmz_core::workspace::normalize_member_path(&ws_canon, &sc_canon);
        let proj_name = ws_canon
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");
        let mut primary_m = dmz_core::workspace::WorkspaceMember::new(
            "primary",
            std::path::Path::new("."),
            dmz_core::workspace::MemberRole::Primary,
        );
        primary_m.populate_detected_defaults(&ws_canon, &ws_canon);
        let mut sidecar_m = dmz_core::workspace::WorkspaceMember::new(
            "sidecar",
            normalized_sc_path,
            dmz_core::workspace::MemberRole::Sidecar,
        );
        sidecar_m.populate_detected_defaults(&sc_canon, &ws_canon);
        let project_cfg =
            dmz_core::workspace::WorkspaceProjectConfig::new(proj_name, vec![primary_m, sidecar_m]);

        let unified_lock = match ResolverEngine::resolve_multi_workspace_with_options(
            &ws_canon,
            &project_cfg,
            None,
            &options,
        ) {
            Ok(lock) => lock,
            Err(e) => {
                error!("Dual-workspace dependency resolution failed: {}", e);
                return Ok(1);
            }
        };

        if let Err(e) = unified_lock.save(&effective_output) {
            error!("Failed to save lockfile: {}", e);
            return Ok(1);
        }

        // Auto-generate dmz-workspace.toml if absent
        let manifest_path = ws_canon.join("dmz-workspace.toml");
        if !manifest_path.exists() {
            match project_cfg.save_to_file(&manifest_path) {
                Ok(()) => {
                    info!(
                        "Auto-generated dual-repo project manifest at {:?}",
                        manifest_path
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to auto-generate dual-repo project manifest at {:?}: {}",
                        manifest_path, e
                    );
                }
            }
        }

        info!(
            "Successfully written dual-workspace closure lock ({} packages) to {:?}",
            unified_lock.packages.len(),
            effective_output
        );
    } else {
        match ResolverEngine::resolve_workspace_with_options(&ws_canon, options) {
            Ok(lockfile) => {
                if let Err(e) = lockfile.save(&effective_output) {
                    error!("Failed to save lockfile: {}", e);
                    return Ok(1);
                }
                info!(
                    "Successfully written closure lock to {:?}",
                    effective_output
                );
            }
            Err(e) => {
                error!("Dependency resolution failed: {}", e);
                return Ok(1);
            }
        }
    }

    Ok(0)
}
