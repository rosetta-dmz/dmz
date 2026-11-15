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

/// Formats a path or argument safely for copy-pasteable shell snippets and error hints.
pub fn quote_shell_arg(arg: &str) -> String {
    if arg.is_empty() {
        return if cfg!(windows) {
            "\"\"".to_string()
        } else {
            "''".to_string()
        };
    }

    let is_safe_unquoted = arg.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '.' | '_' | '-' | '/' | ':' | '@' | '+' | '=' | ',' | '~')
    });
    if is_safe_unquoted {
        return arg.to_string();
    }

    if cfg!(windows) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Prints target host prerequisite notes (such as macOS Apple CLT/Xcode) after air-gap export.
pub fn notify_export_host_prerequisites(
    workspace_canon: &Path,
    lockfile_path: &Path,
    target_platform: Option<&str>,
) {
    if let Some(note) = dmz_core::toolchain::export_apple_prereq_note_from_lockfile(
        workspace_canon,
        lockfile_path,
        target_platform,
    ) {
        println!("[DMZ PREREQUISITE] {}", note);
    }
}

/// Central entrypoint to dispatch any parsed CLI command to its runner
pub fn dispatch(command: Commands) -> anyhow::Result<i32> {
    match command {
        Commands::Resolve {
            workspace,
            sidecar,
            output,
            target,
            include_dev,
            prod_only,
        } => run_resolve(
            workspace,
            sidecar,
            output,
            target,
            include_dev && !prod_only,
        ),

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
            strict_kernel,
            strict_metadata,
            diagnose,
            exec_command,
            args,
        } => run_sandbox(
            workspace,
            sidecar,
            closure,
            allow_network,
            strict_kernel,
            strict_metadata,
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
            strict_metadata,
            shell,
            command,
            args,
        } => run_shell(
            workspace,
            sidecar,
            closure,
            allow_network,
            no_sandbox,
            strict_metadata,
            shell,
            command,
            args,
        ),

        Commands::Init {
            workspace,
            target,
            include_dev,
            prod_only,
        } => run_init(workspace, target, include_dev && !prod_only),

        Commands::Ecosystem { action } => run_ecosystem(action),

        Commands::Run(args) => run_run(args),
    }
}
