use crate::cli::RunArgs;
use std::path::Path;
use tracing::{error, info, warn};

/// Launch workspace members under coordinated supervisor execution
pub fn run_run(args: RunArgs) -> anyhow::Result<i32> {
    info!(
        workspace = ?args.workspace,
        "Initializing workspace execution under supervisor orchestration"
    );

    if args.no_sandbox {
        warn!("--no-sandbox requested: Workspace members will execute under host user-space supervision.");
    }

    let (mut project_config, effective_workspace_root) =
        match dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest_checked(
            &args.workspace,
        ) {
            Ok(Some((cfg, manifest_file))) => {
                let root = manifest_file
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf();
                let root = std::fs::canonicalize(&root).unwrap_or(root);
                info!(
                    manifest = ?manifest_file,
                    "Loaded workspace project configuration"
                );
                (cfg, root)
            }
            Ok(None) => {
                let ws_canon = std::fs::canonicalize(&args.workspace)
                    .unwrap_or_else(|_| args.workspace.clone());
                let ws_name = ws_canon
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "workspace".to_string());

                let mut members = Vec::new();
                members.push(dmz_core::workspace::WorkspaceMember::new(
                    "primary",
                    Path::new("."),
                    dmz_core::workspace::MemberRole::Primary,
                ));

                if let Some(ref sc) = args.sidecar {
                    let sc_canon = std::fs::canonicalize(sc).unwrap_or_else(|_| sc.clone());
                    let sc_name = sc_canon
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "companion".to_string());
                    let normalized_sc =
                        dmz_core::workspace::normalize_member_path(&ws_canon, &sc_canon);
                    members.push(dmz_core::workspace::WorkspaceMember::new(
                        sc_name,
                        normalized_sc,
                        dmz_core::workspace::MemberRole::Sidecar,
                    ));
                }

                let cfg = dmz_core::workspace::WorkspaceProjectConfig::new(ws_name, members);
                (cfg, ws_canon)
            }
            Err(e) => {
                error!("Failed to load workspace manifest: {}", e);
                return Ok(1);
            }
        };

    // Auto-detect or apply command override for all members without an explicit command
    for member in &mut project_config.members {
        if member.role == dmz_core::workspace::MemberRole::Primary {
            if let Some(ref cmd) = args.command {
                member.exec_command = Some(cmd.clone());
                if !args.args.is_empty() {
                    member.exec_args = Some(args.args.clone());
                }
                continue;
            }
        }

        if member.exec_command.is_none() {
            let member_dir = if member.path.is_absolute() {
                member.path.clone()
            } else {
                effective_workspace_root.join(&member.path)
            };
            let member_dir = std::fs::canonicalize(&member_dir).unwrap_or(member_dir);

            if let Ok((cmd, cmd_args)) =
                dmz_supervisor::SidecarOrchestrator::infer_member_execution_in(
                    &effective_workspace_root,
                    &member_dir,
                    member,
                )
            {
                member.exec_command = Some(cmd);
                if !cmd_args.is_empty() {
                    member.exec_args = Some(cmd_args);
                }
            }
        }
    }

    let orchestrator = dmz_supervisor::SidecarOrchestrator::new();
    let pair_config = dmz_supervisor::SidecarPairConfig {
        workspace_root: effective_workspace_root,
        project: project_config,
        allow_network: args.allow_network,
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            error!("Failed to initialize Tokio runtime: {}", e);
            return Ok(1);
        }
    };

    let exit_code = rt.block_on(async {
        info!("Launching workspace pair with supervisor lifecycle management...");
        match orchestrator.start_pair(pair_config).await {
            Ok(sock) => {
                info!(
                    socket = ?sock,
                    "Workspace pair active; socket ready. Press Ctrl+C to terminate."
                );
            }
            Err(e) => {
                error!("Supervisor startup failed: {}", e);
                return 1;
            }
        }

        let code = tokio::select! {
            exit_res = orchestrator.wait_for_exit() => {
                match exit_res {
                    Ok(status) => {
                        info!("Primary process exited with status: {}", status);
                        if let Some(c) = status.code() {
                            c
                        } else {
                            #[cfg(unix)]
                            {
                                use std::os::unix::process::ExitStatusExt;
                                if let Some(sig) = status.signal() {
                                    128 + sig
                                } else if status.success() {
                                    0
                                } else {
                                    1
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                if status.success() { 0 } else { 1 }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Wait error: {}", e);
                        1
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Interrupt received (SIGINT); draining workspace pair gracefully...");
                130
            }
        };

        if let Err(e) = orchestrator.stop_pair().await {
            warn!("Error during workspace pair shutdown: {}", e);
        }
        info!("Workspace supervision terminated cleanly");
        code
    });

    Ok(exit_code)
}
