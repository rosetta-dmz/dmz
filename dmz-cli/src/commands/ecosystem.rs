use crate::cli::{EcosystemAction, DEFAULT_STORE_PATH};
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use tracing::{error, info, warn};

/// Inspect and manage multi-ecosystem workspace stacks (Rust, Node, Go, Python, etc.)
pub fn run_ecosystem(action: EcosystemAction) -> anyhow::Result<i32> {
    match action {
        EcosystemAction::Discover { workspace } => {
            info!(
                "Scanning workspace for active ecosystems at {:?}",
                workspace
            );
            let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
            let registry = EcosystemRegistry::new();
            match registry.process_workspace(&ws_canon) {
                Ok(stacks) => {
                    info!(active_stacks = ?stacks, "Ecosystem discovery completed successfully");
                    Ok(0)
                }
                Err(e) => {
                    error!("Ecosystem discovery failed: {}", e);
                    Ok(1)
                }
            }
        }
        EcosystemAction::Verify {
            workspace,
            no_sandbox,
        } => {
            info!("Verifying workspace ecosystems at {:?}", workspace);
            let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
            let registry = EcosystemRegistry::new();
            let active = registry.discover_active(&ws_canon);
            if active.is_empty() {
                warn!(
                    "No active ecosystems discovered in workspace at {:?}",
                    ws_canon
                );
                return Ok(0);
            }

            let mut verified_count = 0;
            for eco in active {
                if let Some(cmd) = eco.verification_command(&ws_canon) {
                    info!(
                        "Running verification for '{}': {} {}",
                        eco.name(),
                        cmd.program,
                        cmd.args.join(" ")
                    );

                    let mut toolchain_envs: std::collections::HashMap<String, String> =
                        std::env::vars().collect();
                    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(
                        &ws_canon,
                        &mut toolchain_envs,
                    );

                    let status_res = if no_sandbox {
                        info!("Executing verification unsandboxed (--no-sandbox requested)");
                        let mut proc_cmd = std::process::Command::new(&cmd.program);
                        proc_cmd.args(&cmd.args).current_dir(&ws_canon);
                        for (k, v) in &toolchain_envs {
                            proc_cmd.env(k, v);
                        }
                        proc_cmd.status().map_err(|e| e.to_string())
                    } else {
                        info!("Executing verification inside zero-network sandbox");
                        let closure_path = ws_canon.join(DEFAULT_STORE_PATH);
                        let config = SandboxConfig {
                            workspace_path: ws_canon.clone(),
                            closure_path,
                            command: cmd.program.clone(),
                            args: cmd.args.clone(),
                            envs: toolchain_envs.clone().into_iter().collect(),
                            allow_network: false,
                            ipc_socket_path: None,
                        };
                        run_sandboxed(&config).map_err(|e| e.to_string())
                    };

                    match status_res {
                        Ok(status) => {
                            if !status.success() {
                                error!("Ecosystem verification failed for '{}'", eco.name());
                                return Ok(1);
                            }
                            verified_count += 1;
                        }
                        Err(e) => {
                            error!(
                                "Failed to execute sandboxed verification command for '{}': {}. Pass --no-sandbox to bypass sandbox containment if required by your environment.",
                                eco.name(),
                                e
                            );
                            return Ok(1);
                        }
                    }
                }
            }

            if verified_count > 0 {
                info!(
                    "All {} ecosystem verifications passed successfully!",
                    verified_count
                );
            } else {
                warn!("Active ecosystems detected but none provided verification commands.");
            }
            Ok(0)
        }
    }
}
