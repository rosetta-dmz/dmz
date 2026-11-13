use crate::cli::{DEFAULT_LOCKFILE_PATH, DEFAULT_STORE_PATH};
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::path::{Path, PathBuf};
use tracing::{error, info};

/// Open an interactive shell inside the workspace (sandboxed by default)
#[allow(clippy::too_many_arguments)]
pub fn run_shell(
    workspace: PathBuf,
    sidecar: Option<PathBuf>,
    closure: PathBuf,
    allow_network: bool,
    no_sandbox: bool,
    shell: Option<String>,
    command: Option<String>,
    args: Vec<String>,
) -> anyhow::Result<i32> {
    let abs_workspace = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
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
        super::resolve_workspace_relative_path(&abs_workspace, &closure, DEFAULT_STORE_PATH);
    let effective_closure = std::fs::canonicalize(&effective_closure).unwrap_or(effective_closure);

    // Advisory pre-flight check for dmz.lock
    let lockfile_path = super::resolve_workspace_relative_path(
        &abs_workspace,
        Path::new(DEFAULT_LOCKFILE_PATH),
        DEFAULT_LOCKFILE_PATH,
    );
    if !lockfile_path.exists() {
        eprintln!(
            "\x1b[33m[DMZ ADVISORY]\x1b[0m No '{}' found in workspace {:?}.\n\
             Tip: Run 'dmz init' or 'dmz bundle' to lock and cache dependencies for full air-gap isolation.\n",
            DEFAULT_LOCKFILE_PATH,
            abs_workspace
        );
    }

    // Resolve shell binary: user option -> $SHELL -> platform default
    let shell_cmd = shell.unwrap_or_else(|| {
        std::env::var("SHELL").unwrap_or_else(|_| {
            if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        })
    });

    // Assemble command arguments (-c string vs interactive)
    let mut final_args = args;
    if let Some(cmd_str) = command {
        final_args.insert(0, "-c".to_string());
        final_args.insert(1, cmd_str);
    }

    // Prepare environment variables
    let mut envs: std::collections::HashMap<String, String> = std::env::vars().collect();
    envs.insert(
        "DMZ_WORKSPACE".to_string(),
        abs_workspace.display().to_string(),
    );
    envs.insert(
        "DMZ_STORE".to_string(),
        effective_closure.display().to_string(),
    );
    envs.insert(
        "CARGO_HOME".to_string(),
        abs_workspace.join(".cargo").display().to_string(),
    );
    envs.insert(
        "NODE_PATH".to_string(),
        abs_workspace.join("node_modules").display().to_string(),
    );

    // Inject bundled toolchain paths if available (.dmz/toolchains/*/bin)
    dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(&abs_workspace, &mut envs);

    let is_sandbox = !no_sandbox;
    envs.insert(
        "DMZ_SANDBOX".to_string(),
        if is_sandbox {
            "1".to_string()
        } else {
            "0".to_string()
        },
    );

    let ipc_socket_path =
        match dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest_checked(
            &abs_workspace,
        ) {
            Ok(Some((project, _))) => Some(dmz_core::workspace::safe_socket_path(
                &abs_workspace,
                &project.ipc.socket_id,
            )),
            Ok(None) => sc_canon
                .as_ref()
                .map(|_| dmz_core::workspace::safe_socket_path(&abs_workspace, "sidecar")),
            Err(e) => {
                error!("Failed to load dmz-workspace.toml: {}", e);
                return Ok(1);
            }
        };

    if let Some(ref sock) = ipc_socket_path {
        if let Some(posix_warn) = dmz_core::workspace::check_socket_path_length(sock) {
            eprintln!("\x1b[33m[DMZ POSIX ADVISORY]\x1b[0m {}", posix_warn);
        }
        envs.insert("DMZ_SIDECAR_SOCK".to_string(), sock.display().to_string());
    }

    // Visual prompt indicator
    let current_ps1 = std::env::var("PS1").unwrap_or_else(|_| "\\u@\\h:\\w\\$ ".to_string());
    let prompt_prefix = if is_sandbox {
        "(dmz-sandbox) "
    } else {
        "(dmz-host) "
    };
    envs.insert(
        "PS1".to_string(),
        format!("{}{}", prompt_prefix, current_ps1),
    );

    let env_vec: Vec<(String, String)> = envs.into_iter().collect();

    if no_sandbox {
        info!(
            shell = %shell_cmd,
            workspace = ?abs_workspace,
            "Launching unsandboxed host shell with DMZ environment"
        );
        let mut cmd = std::process::Command::new(&shell_cmd);
        cmd.args(&final_args)
            .current_dir(&abs_workspace)
            .envs(env_vec);

        match cmd.status() {
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
                error!("Failed to launch host shell '{}': {}", shell_cmd, e);
                Ok(1)
            }
        }
    } else {
        info!(
            shell = %shell_cmd,
            network_allowed = allow_network,
            "Launching daemonless sandboxed shell"
        );

        let config = SandboxConfig {
            workspace_path: abs_workspace,
            closure_path: effective_closure,
            command: shell_cmd,
            args: final_args,
            envs: env_vec,
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
                error!("Sandbox shell execution failed: {}", e);
                Ok(1)
            }
        }
    }
}
