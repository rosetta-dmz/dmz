use crate::cli::DEFAULT_STORE_PATH;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::path::PathBuf;
use tracing::{error, info};

/// Execute a command inside a native daemonless OS sandbox
#[allow(clippy::too_many_arguments)]
pub fn run_sandbox(
    workspace: PathBuf,
    sidecar: Option<PathBuf>,
    closure: PathBuf,
    allow_network: bool,
    diagnose: bool,
    exec_command: String,
    args: Vec<String>,
) -> anyhow::Result<i32> {
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    let sc_canon = sidecar
        .as_ref()
        .map(|s| std::fs::canonicalize(s).unwrap_or_else(|_| s.clone()));

    if let Some(ref sc) = sc_canon {
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(sc);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }
    }

    let effective_closure =
        super::resolve_workspace_relative_path(&ws_canon, &closure, DEFAULT_STORE_PATH);
    let effective_closure = std::fs::canonicalize(&effective_closure).unwrap_or(effective_closure);

    let ipc_socket_path =
        match dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest_checked(
            &ws_canon,
        ) {
            Ok(Some((project, _))) => Some(dmz_core::workspace::safe_socket_path(
                &ws_canon,
                &project.ipc.socket_id,
            )),
            Ok(None) => sc_canon
                .as_ref()
                .map(|_| dmz_core::workspace::safe_socket_path(&ws_canon, "sidecar")),
            Err(e) => {
                error!("Failed to load dmz-workspace.toml: {}", e);
                return Ok(1);
            }
        };

    if let Some(ref sock) = ipc_socket_path {
        if let Some(posix_warn) = dmz_core::workspace::check_socket_path_length(sock) {
            eprintln!("\x1b[33m[DMZ POSIX ADVISORY]\x1b[0m {}", posix_warn);
        }
    }

    info!(
        workspace = ?ws_canon,
        closure = ?effective_closure,
        command = %exec_command,
        allow_network = allow_network,
        "Launching sandboxed execution"
    );

    let mut envs: std::collections::HashMap<String, String> = std::env::vars().collect();
    envs.insert("DMZ_WORKSPACE".to_string(), ws_canon.display().to_string());
    envs.insert(
        "DMZ_STORE".to_string(),
        effective_closure.display().to_string(),
    );
    envs.insert("DMZ_SANDBOX".to_string(), "1".to_string());
    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&ws_canon, &mut envs);

    let config = SandboxConfig {
        workspace_path: ws_canon,
        closure_path: effective_closure,
        command: exec_command,
        args,
        envs: envs.into_iter().collect(),
        allow_network,
        ipc_socket_path,
    };

    match run_sandboxed(&config) {
        Ok(status) => {
            if let Some(code) = status.code() {
                return Ok(code);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    return Ok(128 + sig);
                }
            }
            if !status.success() {
                return Ok(1);
            }
            Ok(0)
        }
        Err(e) => {
            if diagnose {
                let diag = dmz_core::diagnostics::DiagnosticError::classify(&e);

                match diag.category {
                    dmz_core::diagnostics::ErrorCategory::PlatformFault => {
                        eprintln!("\x1b[31m[DMZ PLATFORM FAULT]\x1b[0m");
                    }
                    dmz_core::diagnostics::ErrorCategory::PermissionDenied => {
                        eprintln!("\x1b[31m[PERMISSION DENIED]\x1b[0m");
                    }
                    dmz_core::diagnostics::ErrorCategory::ToolchainFault => {
                        eprintln!("\x1b[33m[TOOLCHAIN FAULT]\x1b[0m");
                    }
                    dmz_core::diagnostics::ErrorCategory::IoFault => {
                        eprintln!("\x1b[31m[FILESYSTEM / IO FAULT]\x1b[0m");
                    }
                    dmz_core::diagnostics::ErrorCategory::UserCodeFault => {
                        eprintln!("\x1b[33m[USER CODE / DEPENDENCY FAULT]\x1b[0m");
                    }
                }
                eprintln!("Details: {}", diag.message);
                eprintln!("Remediation: \x1b[1m{}\x1b[0m", diag.remediation);
                Ok(1)
            } else {
                error!("Sandbox execution error: {}", e);
                Ok(1)
            }
        }
    }
}
