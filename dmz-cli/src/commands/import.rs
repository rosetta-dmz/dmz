use crate::cli::{ImportArgs, DEFAULT_STORE_PATH};
use dmz_core::archive::{AirGapImporter, ImportConfig, ImportResult};
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::path::Path;
use tracing::{error, info, warn};

fn resolve_code_workspace_filename(result: &ImportResult) -> String {
    let expected = result
        .workspace_project
        .as_ref()
        .filter(|p| !p.name.is_empty())
        .map(|p| format!("{}.code-workspace", p.name));

    if let Some(ref candidate) = expected {
        if result.workspace_path.join(candidate).is_file() {
            return candidate.clone();
        }
    }

    if let Ok(entries) = std::fs::read_dir(&result.workspace_path) {
        let mut matches: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("code-workspace") {
                    p.file_name().map(|n| n.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect();
        matches.sort();
        if let Some(first) = matches.into_iter().next() {
            return first;
        }
    }

    expected.unwrap_or_else(|| "workspace.code-workspace".to_string())
}

/// Formats copy-pasteable post-import next steps for dual-repo vs single-repo workspaces.
pub fn format_import_next_steps(target_workspace: &Path, result: &ImportResult) -> String {
    let target_display = super::quote_shell_arg(&target_workspace.display().to_string());
    let is_dual_repo = result.workspace_project.is_some()
        || result.workspace_path.join("dmz-workspace.toml").is_file();

    if is_dual_repo {
        let code_workspace = super::quote_shell_arg(&resolve_code_workspace_filename(result));
        format!(
            "\nNext steps (dual-repo workspace):\n  cd {}\n  dmz run --workspace .   # run from your host shell (do not run inside `dmz shell`)\n  code {}\n",
            target_display, code_workspace
        )
    } else {
        let env_hint = if cfg!(windows) {
            ".dmz\\env.bat"
        } else {
            "source .dmz/env.sh"
        };
        format!(
            "\nNext steps:\n  cd {}\n  {}   # or: dmz shell\n",
            target_display, env_hint
        )
    }
}

/// Unpack, verify, and restore an air-gap bundle into local store
pub fn run_import(args: ImportArgs) -> anyhow::Result<i32> {
    let target_display = args.workspace.clone();
    if let Err(e) = std::fs::create_dir_all(&args.workspace) {
        error!(
            "Failed to create workspace directory {:?}: {}",
            args.workspace, e
        );
        return Ok(1);
    }
    let ws_canon =
        std::fs::canonicalize(&args.workspace).unwrap_or_else(|_| args.workspace.clone());
    let effective_archive = if args.archive.is_relative() {
        if args.archive.exists() {
            std::fs::canonicalize(&args.archive).unwrap_or(args.archive)
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join(&args.archive)
        }
    } else {
        args.archive
    };

    let effective_store =
        super::resolve_workspace_relative_path(&ws_canon, &args.store, DEFAULT_STORE_PATH);

    let config = ImportConfig {
        archive_path: effective_archive,
        target_store_dir: effective_store.clone(),
        target_workspace_dir: ws_canon.clone(),
        verify_closure_hash: !args.skip_verify,
        include_dev: args.include_dev,
        install_dependencies: !args.skip_install,
    };

    let import_result = match AirGapImporter::import(&config) {
        Ok(result) => {
            info!(
                imported = result.imported_packages,
                installed = result.installed_packages,
                closure = %result.closure_signature,
                is_delta = result.is_delta,
                has_git = result.has_git_history,
                vcs = ?result.vcs,
                "Air-gap import completed successfully"
            );
            result
        }
        Err(e) => {
            error!("Air-gap import failed: {}", e);
            return Ok(1);
        }
    };

    // Report offline installation outcome performed during import (unless --skip-install)
    if !args.skip_install {
        if let Some(ref warn_msg) = import_result.install_warning {
            warn!(
                "Offline dependency installation skipped or deferred: {}",
                warn_msg
            );
        } else {
            info!(
                "Successfully installed {} packages offline into project layouts",
                import_result.installed_packages
            );
        }

        // Automated ecosystem verification (unless --skip-ecosystem-verify)
        if !args.skip_ecosystem_verify {
            info!(
                "Running automated ecosystem verification for workspace {:?}",
                ws_canon
            );
            let registry = EcosystemRegistry::new();
            let active_pairs = registry.discover_active_with_roots(&ws_canon);
            for (eco, member_root) in active_pairs {
                if let Some(cmd) = eco.verification_command(&member_root) {
                    info!(
                        "Running verification for '{}' in {:?}: {} {}",
                        eco.name(),
                        member_root,
                        cmd.program,
                        cmd.args.join(" ")
                    );

                    // Inject bundled toolchain paths if available
                    let mut toolchain_envs: std::collections::HashMap<String, String> =
                        std::env::vars().collect();
                    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(
                        &ws_canon,
                        &mut toolchain_envs,
                    );
                    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(
                        &member_root,
                        &mut toolchain_envs,
                    );

                    let status_res = if args.no_sandbox {
                        info!("Executing verification unsandboxed (--no-sandbox requested)");
                        let mut proc_cmd = std::process::Command::new(&cmd.program);
                        proc_cmd.args(&cmd.args).current_dir(&member_root);
                        for (k, v) in &toolchain_envs {
                            proc_cmd.env(k, v);
                        }
                        proc_cmd.status().map_err(|e| e.to_string())
                    } else {
                        info!("Executing verification inside zero-network sandbox");
                        let config = SandboxConfig {
                            workspace_path: member_root.clone(),
                            closure_path: effective_store.clone(),
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
            info!("All ecosystem verifications passed successfully!");
        }
    }

    print!(
        "{}",
        format_import_next_steps(&target_display, &import_result)
    );

    Ok(0)
}
