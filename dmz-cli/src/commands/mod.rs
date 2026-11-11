use crate::cli::{AirgapCommands, Commands};
use std::path::{Path, PathBuf};

/// Resolves a candidate path relative to workspace or canonical filesystem root
pub fn resolve_workspace_relative_path(
    workspace_canon: &Path,
    candidate: &Path,
    default_name: &str,
) -> PathBuf {
    if candidate == Path::new(default_name) {
        workspace_canon.join(default_name)
    } else if candidate.is_relative() {
        if workspace_canon.join(candidate).exists() {
            workspace_canon.join(candidate)
        } else if candidate.exists() {
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf())
        } else {
            workspace_canon.join(candidate)
        }
    } else {
        candidate.to_path_buf()
    }
}

pub mod build;
pub mod bundle;
pub mod ecosystem;
pub mod export;
pub mod fetch;
pub mod import;
pub mod init;
pub mod install;
pub mod resolve;
pub mod run;
pub mod sandbox;
pub mod shell;

pub use build::run_build;
pub use bundle::run_bundle;
pub use ecosystem::run_ecosystem;
pub use export::run_export;
pub use fetch::run_fetch;
pub use import::run_import;
pub use init::run_init;
pub use install::run_install;
pub use resolve::run_resolve;
pub use run::run_run;
pub use sandbox::run_sandbox;
pub use shell::run_shell;

/// Central entrypoint to dispatch any parsed CLI command to its runner
pub fn dispatch(command: Commands) -> anyhow::Result<i32> {
    match command {
        Commands::Resolve {
            workspace,
            sidecar,
            output,
            target,
        } => run_resolve(workspace, sidecar, output, target),

        Commands::Fetch { lockfile, store } => run_fetch(lockfile, store),

        Commands::Install {
            workspace,
            lockfile,
            store,
        } => run_install(workspace, lockfile, store),

        Commands::Build {
            target,
            workspace,
            lockfile,
            output,
            name,
            zstd_level,
        } => run_build(target, workspace, lockfile, output, name, zstd_level),

        Commands::Sandbox {
            workspace,
            sidecar,
            closure,
            allow_network,
            diagnose,
            exec_command,
            args,
        } => run_sandbox(
            workspace,
            sidecar,
            closure,
            allow_network,
            diagnose,
            exec_command,
            args,
        ),

        Commands::Airgap { action } => match action {
            AirgapCommands::Bundle(args) => run_bundle(args),
            AirgapCommands::Export(args) => run_export(args),
            AirgapCommands::Import(args) => run_import(args),
        },

        Commands::Bundle(args) => run_bundle(args),

        Commands::Export(args) => run_export(args),

        Commands::Import(args) => run_import(args),

        Commands::Shell {
            workspace,
            sidecar,
            closure,
            allow_network,
            no_sandbox,
            shell,
            command,
            args,
        } => run_shell(
            workspace,
            sidecar,
            closure,
            allow_network,
            no_sandbox,
            shell,
            command,
            args,
        ),

        Commands::Init { workspace, target } => run_init(workspace, target),

        Commands::Ecosystem { action } => run_ecosystem(action),

        Commands::Run(args) => run_run(args),
    }
}
