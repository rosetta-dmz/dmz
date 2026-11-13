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
) -> anyhow::Result<i32> {
    info!("Resolving workspace dependencies at {:?}", workspace);
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    let effective_output =
        super::resolve_workspace_relative_path(&ws_canon, &output, DEFAULT_LOCKFILE_PATH);

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

        let prim_lock =
            match ResolverEngine::resolve_workspace_with_target(&ws_canon, target.clone()) {
                Ok(lock) => lock,
                Err(e) => {
                    error!("Primary workspace dependency resolution failed: {}", e);
                    return Ok(1);
                }
            };

        let sidecar_lock = match ResolverEngine::resolve_workspace_with_target(&sc_canon, target) {
            Ok(lock) => lock,
            Err(e) => {
                error!("Sidecar workspace dependency resolution failed: {}", e);
                return Ok(1);
            }
        };

        // Merge locked packages with workspace member IDs
        let mut merged_packages = Vec::new();
        for (_, mut pkg) in prim_lock.packages {
            pkg.member_id = Some("primary".to_string());
            merged_packages.push(pkg);
        }
        for (_, mut pkg) in sidecar_lock.packages {
            pkg.member_id = Some("sidecar".to_string());
            merged_packages.push(pkg);
        }

        let mut unified_lock = dmz_core::resolver::lockfile::DmzLockfile::new(merged_packages);
        unified_lock.add_workspace(dmz_core::resolver::lockfile::WorkspaceMetadata {
            id: "primary".to_string(),
            path: ws_canon.display().to_string(),
            role: "primary".to_string(),
            ecosystem: None,
            closure_signature: None,
        });
        unified_lock.add_workspace(dmz_core::resolver::lockfile::WorkspaceMetadata {
            id: "sidecar".to_string(),
            path: sc_canon.display().to_string(),
            role: "sidecar".to_string(),
            ecosystem: None,
            closure_signature: None,
        });

        if let Err(e) = unified_lock.save(&effective_output) {
            error!("Failed to save lockfile: {}", e);
            return Ok(1);
        }

        // Auto-generate dmz-workspace.toml if absent
        let manifest_path = ws_canon.join("dmz-workspace.toml");
        if !manifest_path.exists() {
            let mut primary_m = dmz_core::workspace::WorkspaceMember::new(
                "primary",
                std::path::Path::new("."),
                dmz_core::workspace::MemberRole::Primary,
            );
            primary_m.populate_detected_defaults(&ws_canon, &ws_canon);

            let normalized_sc_path =
                dmz_core::workspace::normalize_member_path(&ws_canon, &sc_canon);
            let mut sidecar_m = dmz_core::workspace::WorkspaceMember::new(
                "sidecar",
                normalized_sc_path,
                dmz_core::workspace::MemberRole::Sidecar,
            );
            sidecar_m.populate_detected_defaults(&sc_canon, &ws_canon);

            let proj_name = ws_canon
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project");
            let project_cfg = dmz_core::workspace::WorkspaceProjectConfig::new(
                proj_name,
                vec![primary_m, sidecar_m],
            );
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
        match ResolverEngine::resolve_workspace_with_target(&ws_canon, target) {
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
