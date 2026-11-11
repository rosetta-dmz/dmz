use crate::cli::{
    BundleArgs, DEFAULT_CLOSURE_ARCHIVE_PATH, DEFAULT_LOCKFILE_PATH, DEFAULT_STORE_PATH,
};
use dmz_core::archive::{AirGapExporter, ExportConfig};
use dmz_core::resolver::ResolverEngine;
use std::path::{Path, PathBuf};
use tracing::{error, info};

/// One-shot resolution, dependency fetching, and export into an air-gap bundle (.tar.zst)
pub fn run_bundle(args: BundleArgs) -> anyhow::Result<i32> {
    info!(
        "Starting streamlined air-gap bundling for workspace {:?}",
        args.workspace
    );

    let with_toolchain = args.toolchain_enabled();

    if let Some(ref sc) = args.sidecar {
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(sc);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }

        // POSIX socket path limit advisory
        let sock_path = dmz_core::workspace::safe_socket_path(&args.workspace, "sidecar");
        if let Some(posix_warn) = dmz_core::workspace::check_socket_path_length(&sock_path) {
            eprintln!("\x1b[33m[DMZ POSIX ADVISORY]\x1b[0m {}", posix_warn);
        }
    }

    let ws_canon =
        std::fs::canonicalize(&args.workspace).unwrap_or_else(|_| args.workspace.clone());

    let effective_lockfile =
        super::resolve_workspace_relative_path(&ws_canon, &args.lockfile, DEFAULT_LOCKFILE_PATH);
    let effective_store =
        super::resolve_workspace_relative_path(&ws_canon, &args.store, DEFAULT_STORE_PATH);

    let multi_workspace =
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&ws_canon)
            .map(|(cfg, _)| cfg)
            .or_else(|| {
                args.sidecar.as_ref().map(|sc| {
                    let primary = dmz_core::workspace::WorkspaceMember::new(
                        "primary",
                        &ws_canon,
                        dmz_core::workspace::MemberRole::Primary,
                    );
                    let sidecar_m = dmz_core::workspace::WorkspaceMember::new(
                        "sidecar",
                        sc,
                        dmz_core::workspace::MemberRole::Sidecar,
                    );
                    let proj_name = ws_canon
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("project");
                    dmz_core::workspace::WorkspaceProjectConfig::new(
                        proj_name,
                        vec![primary, sidecar_m],
                    )
                })
            })
            .or_else(|| {
                dmz_core::workspace::WorkspaceProjectConfig::discover_or_from_lockfile(
                    &ws_canon,
                    &effective_lockfile,
                )
            });

    // Resolve manifests if lockfile is missing or refresh_lock is requested
    let should_resolve = args.refresh_lock || !effective_lockfile.exists();
    if should_resolve {
        info!("Resolving workspace dependency manifests at {:?}", ws_canon);
        let resolve_res = if let Some(ref mws) = multi_workspace {
            ResolverEngine::resolve_multi_workspace_with_target(&ws_canon, mws, args.target.clone())
        } else {
            ResolverEngine::resolve_workspace_with_target(&ws_canon, args.target.clone())
        };

        match resolve_res {
            Ok(resolved_lockfile) => {
                if let Err(e) = resolved_lockfile.save(&effective_lockfile) {
                    error!("Failed to write lockfile {:?}: {}", effective_lockfile, e);
                    return Ok(1);
                }
                info!(
                    "Successfully generated closure lock at {:?}",
                    effective_lockfile
                );
            }
            Err(e) => {
                error!("Dependency resolution failed: {}", e);
                return Ok(1);
            }
        }
    } else {
        info!(
            "Reusing existing closure lockfile at {:?}",
            effective_lockfile
        );
    }

    // Fetch dependencies into store cache
    info!(
        "Fetching and caching dependencies into store {:?}",
        effective_store
    );
    match ResolverEngine::fetch_dependencies(&effective_lockfile, &effective_store) {
        Ok(count) => {
            info!(
                "Successfully fetched and cached {} dependencies in {:?}",
                count, effective_store
            );
        }
        Err(e) => {
            error!("Dependency fetch failed: {}", e);
            return Ok(1);
        }
    }

    // Export unified air-gap archive
    let effective_output = if args.output == Path::new(DEFAULT_CLOSURE_ARCHIVE_PATH) {
        ws_canon.join(DEFAULT_CLOSURE_ARCHIVE_PATH)
    } else if args.output.is_relative() {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&args.output)
    } else {
        args.output
    };

    let config = ExportConfig {
        workspace_path: ws_canon,
        lockfile_path: effective_lockfile,
        store_dir: effective_store,
        output_path: effective_output,
        zstd_level: args.zstd_level,
        mode: args.mode.into(),
        target_platform: args.target,
        with_git_history: args.with_git_history,
        since: args.since,
        with_toolchain,
        multi_workspace,
    };

    match AirGapExporter::export(&config) {
        Ok(manifest) => {
            info!(
                checksum = %manifest.archive_sha256,
                packages = manifest.package_count,
                target_platform = ?manifest.target_platform,
                "Air-gap bundle created successfully"
            );
            Ok(0)
        }
        Err(e) => {
            error!("Air-gap bundle export failed: {}", e);
            Ok(1)
        }
    }
}
