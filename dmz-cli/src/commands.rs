use crate::cli::{AirgapCommands, CliExportMode, Commands, EcosystemAction, TargetMode};
use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_core::packaging::{BuildConfig, PackageBuilder};
use dmz_core::resolver::ResolverEngine;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::path::{Path, PathBuf};
use std::process::exit;
use tracing::{error, info};

/// Central entrypoint to dispatch any parsed CLI command to its runner
pub fn dispatch(command: Commands) {
    match command {
        Commands::Resolve {
            workspace,
            output,
            target,
        } => {
            run_resolve(workspace, output, target);
        }

        Commands::Fetch { lockfile, store } => {
            run_fetch(lockfile, store);
        }

        Commands::Install {
            workspace,
            lockfile,
            store,
        } => {
            run_install(workspace, lockfile, store);
        }

        Commands::Build {
            target,
            workspace,
            lockfile,
            output,
            name,
            zstd_level,
        } => {
            run_build(target, workspace, lockfile, output, name, zstd_level);
        }

        Commands::Sandbox {
            workspace,
            closure,
            allow_network,
            diagnose,
            exec_command,
            args,
        } => {
            run_sandbox(
                workspace,
                closure,
                allow_network,
                diagnose,
                exec_command,
                args,
            );
        }

        Commands::Bundle {
            target,
            workspace,
            lockfile,
            store,
            output,
            zstd_level,
            mode,
            refresh_lock,
        } => {
            run_bundle(BundleArgs {
                target,
                workspace,
                lockfile,
                store,
                output,
                zstd_level,
                mode,
                refresh_lock,
            });
        }

        Commands::Import {
            archive,
            store,
            workspace,
            skip_verify,
            skip_install,
            skip_ecosystem_verify,
        } => {
            run_import(
                archive,
                store,
                workspace,
                skip_verify,
                skip_install,
                skip_ecosystem_verify,
            );
        }

        Commands::Shell {
            workspace,
            closure,
            allow_network,
            no_sandbox,
            shell,
            command,
            args,
        } => {
            run_shell(
                workspace,
                closure,
                allow_network,
                no_sandbox,
                shell,
                command,
                args,
            );
        }

        Commands::Init { workspace, target } => {
            run_init(workspace, target);
        }

        Commands::Airgap { action } => match action {
            AirgapCommands::Bundle {
                target,
                workspace,
                lockfile,
                store,
                output,
                zstd_level,
                mode,
                refresh_lock,
            } => {
                run_bundle(BundleArgs {
                    target,
                    workspace,
                    lockfile,
                    store,
                    output,
                    zstd_level,
                    mode,
                    refresh_lock,
                });
            }

            AirgapCommands::Export {
                target,
                workspace,
                lockfile,
                store,
                output,
                zstd_level,
                mode,
            } => {
                run_export(target, workspace, lockfile, store, output, zstd_level, mode);
            }

            AirgapCommands::Import {
                archive,
                store,
                workspace,
                skip_verify,
                skip_install,
                skip_ecosystem_verify,
            } => {
                run_import(
                    archive,
                    store,
                    workspace,
                    skip_verify,
                    skip_install,
                    skip_ecosystem_verify,
                );
            }
        },

        Commands::Ecosystem { action } => {
            run_ecosystem(action);
        }
    }
}

pub fn run_resolve(workspace: PathBuf, output: PathBuf, target: Option<String>) {
    info!("Resolving workspace dependencies at {:?}", workspace);
    let effective_output = if output.is_relative() && workspace != Path::new(".") {
        workspace.join(&output)
    } else {
        output
    };
    match ResolverEngine::resolve_workspace_with_target(&workspace, target) {
        Ok(lockfile) => {
            if let Err(e) = lockfile.save(&effective_output) {
                error!("Failed to save lockfile: {}", e);
                exit(1);
            }
            info!(
                "Successfully written closure lock to {:?}",
                effective_output
            );
        }
        Err(e) => {
            error!("Dependency resolution failed: {}", e);
            exit(1);
        }
    }
}

pub fn run_fetch(lockfile: PathBuf, store: PathBuf) {
    info!("Starting dependency fetch from lockfile: {:?}", lockfile);
    match ResolverEngine::fetch_dependencies(&lockfile, &store) {
        Ok(count) => {
            info!(
                "Successfully fetched {} dependencies into store {:?}",
                count, store
            );
        }
        Err(e) => {
            error!("Dependency fetch failed: {}", e);
            exit(1);
        }
    }
}

pub fn run_install(workspace: PathBuf, lockfile: PathBuf, store: PathBuf) {
    info!(
        "Starting DMZ offline installation at workspace {:?}",
        workspace
    );
    let config = dmz_core::install::InstallConfig {
        workspace_path: workspace,
        lockfile_path: lockfile,
        store_dir: store,
    };
    match dmz_core::install::InstallerEngine::install(&config) {
        Ok(count) => {
            info!("Successfully installed {} packages offline", count);
        }
        Err(e) => {
            error!("Offline installation failed: {}", e);
            exit(1);
        }
    }
}

pub fn run_build(
    target: TargetMode,
    workspace: PathBuf,
    lockfile: PathBuf,
    output: PathBuf,
    name: String,
    zstd_level: i32,
) {
    let effective_lockfile = if lockfile.is_relative() && workspace != Path::new(".") {
        workspace.join(&lockfile)
    } else {
        lockfile
    };
    let effective_output = if output.is_relative() && workspace != Path::new(".") {
        workspace.join(&output)
    } else {
        output
    };

    let config = BuildConfig {
        workspace_dir: workspace,
        lockfile_path: effective_lockfile,
        output_dir: effective_output,
        target: target.into(),
        archive_name: name,
        zstd_level,
    };

    match PackageBuilder::build(&config) {
        Ok(result) => {
            info!(
                path = ?result.artifact_path,
                sha256 = %result.sha256,
                bytes = result.size_bytes,
                "Build completed successfully"
            );
        }
        Err(e) => {
            error!("Build failed: {}", e);
            exit(1);
        }
    }
}

pub fn run_sandbox(
    workspace: PathBuf,
    closure: PathBuf,
    allow_network: bool,
    diagnose: bool,
    exec_command: String,
    args: Vec<String>,
) {
    let config = SandboxConfig {
        workspace_path: workspace,
        closure_path: closure,
        command: exec_command,
        args,
        envs: std::env::vars().collect(),
        allow_network,
    };

    match run_sandboxed(&config) {
        Ok(status) => {
            if let Some(code) = status.code() {
                exit(code);
            }
        }
        Err(e) => {
            if diagnose {
                let diag = dmz_core::diagnostics::DiagnosticError::classify(&e);

                match diag.category {
                    dmz_core::diagnostics::ErrorCategory::PlatformFault => {
                        eprintln!("\x1b[31m[DMZ PLATFORM FAULT]\x1b[0m");
                    }
                    dmz_core::diagnostics::ErrorCategory::UserCodeFault => {
                        eprintln!("\x1b[33m[USER CODE / DEPENDENCY FAULT]\x1b[0m");
                    }
                }
                eprintln!("Details: {}", diag.message);
                eprintln!("Remediation: \x1b[1m{}\x1b[0m", diag.remediation);
                exit(1);
            } else {
                error!("Sandbox execution error: {}", e);
                exit(1);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BundleArgs {
    pub target: Option<String>,
    pub workspace: PathBuf,
    pub lockfile: PathBuf,
    pub store: PathBuf,
    pub output: PathBuf,
    pub zstd_level: i32,
    pub mode: CliExportMode,
    pub refresh_lock: bool,
}

pub fn run_bundle(args: BundleArgs) {
    info!(
        "Starting streamlined air-gap bundling for workspace {:?}",
        args.workspace
    );

    let effective_lockfile = if (args.lockfile == Path::new("dmz.lock")
        && args.workspace != Path::new("."))
        || (!args.lockfile.exists() && args.workspace.join(&args.lockfile).exists())
    {
        args.workspace.join(&args.lockfile)
    } else {
        args.lockfile
    };

    let effective_store = if args.store == Path::new(".dmz/store")
        && args.workspace != Path::new(".")
        && !args.store.exists()
    {
        args.workspace.join(&args.store)
    } else {
        args.store
    };

    // 1. Resolve manifests if lockfile is missing or refresh_lock is requested
    let should_resolve = args.refresh_lock || !effective_lockfile.exists();
    if should_resolve {
        info!(
            "Resolving workspace dependency manifests at {:?}",
            args.workspace
        );
        match ResolverEngine::resolve_workspace_with_target(&args.workspace, args.target.clone()) {
            Ok(resolved_lockfile) => {
                if let Err(e) = resolved_lockfile.save(&effective_lockfile) {
                    error!("Failed to write lockfile {:?}: {}", effective_lockfile, e);
                    exit(1);
                }
                info!(
                    "Successfully generated closure lock at {:?}",
                    effective_lockfile
                );
            }
            Err(e) => {
                error!("Dependency resolution failed: {}", e);
                exit(1);
            }
        }
    } else {
        info!(
            "Reusing existing closure lockfile at {:?}",
            effective_lockfile
        );
    }

    // 2. Fetch dependencies into store cache
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
            exit(1);
        }
    }

    // 3. Export unified air-gap archive
    let effective_output = if args.output.is_relative() && args.workspace != Path::new(".") {
        args.workspace.join(&args.output)
    } else {
        args.output
    };

    let config = ExportConfig {
        workspace_path: args.workspace,
        lockfile_path: effective_lockfile,
        store_dir: effective_store,
        output_path: effective_output,
        zstd_level: args.zstd_level,
        mode: args.mode.into(),
        target_platform: args.target,
    };

    match AirGapExporter::export(&config) {
        Ok(manifest) => {
            info!(
                checksum = %manifest.archive_sha256,
                packages = manifest.package_count,
                target_platform = ?manifest.target_platform,
                "Air-gap bundle created successfully"
            );
        }
        Err(e) => {
            error!("Air-gap bundle export failed: {}", e);
            exit(1);
        }
    }
}

pub fn run_export(
    target: Option<String>,
    workspace: PathBuf,
    lockfile: PathBuf,
    store: PathBuf,
    output: PathBuf,
    zstd_level: i32,
    mode: CliExportMode,
) {
    let effective_lockfile = if lockfile.is_relative() && workspace != Path::new(".") {
        workspace.join(&lockfile)
    } else {
        lockfile
    };
    let effective_store = if store.is_relative() && workspace != Path::new(".") {
        workspace.join(&store)
    } else {
        store
    };
    let effective_output = if output.is_relative() && workspace != Path::new(".") {
        workspace.join(&output)
    } else {
        output
    };

    let config = ExportConfig {
        workspace_path: workspace,
        lockfile_path: effective_lockfile,
        store_dir: effective_store,
        output_path: effective_output,
        zstd_level,
        mode: mode.into(),
        target_platform: target,
    };

    match AirGapExporter::export(&config) {
        Ok(manifest) => {
            info!(
                checksum = %manifest.archive_sha256,
                packages = manifest.package_count,
                target_platform = ?manifest.target_platform,
                "Air-gap export completed successfully"
            );
        }
        Err(e) => {
            error!("Air-gap export failed: {}", e);
            exit(1);
        }
    }
}

pub fn run_import(
    archive: PathBuf,
    store: PathBuf,
    workspace: PathBuf,
    skip_verify: bool,
    skip_install: bool,
    skip_ecosystem_verify: bool,
) {
    let config = ImportConfig {
        archive_path: archive,
        target_store_dir: store.clone(),
        target_workspace_dir: workspace.clone(),
        verify_closure_hash: !skip_verify,
    };

    match AirGapImporter::import(&config) {
        Ok(result) => {
            info!(
                imported = result.imported_packages,
                closure = %result.closure_signature,
                "Air-gap import completed successfully"
            );
        }
        Err(e) => {
            error!("Air-gap import failed: {}", e);
            exit(1);
        }
    }

    // Automated offline installation into project layouts (unless --skip-install)
    if !skip_install {
        let lockfile_path = workspace.join("dmz.lock");
        info!(
            "Running automated offline dependency installation in workspace {:?}",
            workspace
        );
        let install_config = dmz_core::install::InstallConfig {
            workspace_path: workspace.clone(),
            lockfile_path,
            store_dir: store,
        };

        match dmz_core::install::InstallerEngine::install(&install_config) {
            Ok(count) => {
                info!(
                    "Successfully installed {} packages offline into project layouts",
                    count
                );
            }
            Err(e) => {
                error!("Offline installation failed: {}", e);
                exit(1);
            }
        }

        // Automated ecosystem verification (unless --skip-ecosystem-verify)
        if !skip_ecosystem_verify {
            info!(
                "Running automated ecosystem verification for workspace {:?}",
                workspace
            );
            let registry = EcosystemRegistry::new();
            let active = registry.discover_active(&workspace);
            for eco in active {
                if let Some(cmd) = eco.verification_command(&workspace) {
                    info!(
                        "Running verification for '{}': {} {}",
                        eco.name(),
                        cmd.program,
                        cmd.args.join(" ")
                    );
                    match std::process::Command::new(&cmd.program)
                        .args(&cmd.args)
                        .current_dir(&workspace)
                        .status()
                    {
                        Ok(status) => {
                            if !status.success() {
                                error!("Ecosystem verification failed for '{}'", eco.name());
                                exit(1);
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to execute verification command for '{}': {}",
                                eco.name(),
                                e
                            );
                            exit(1);
                        }
                    }
                }
            }
            info!("All ecosystem verifications passed successfully!");
        }
    }
}

pub fn run_shell(
    workspace: PathBuf,
    closure: PathBuf,
    allow_network: bool,
    no_sandbox: bool,
    shell: Option<String>,
    command: Option<String>,
    args: Vec<String>,
) {
    // 1. Advisory pre-flight check for dmz.lock
    let lockfile_path = workspace.join("dmz.lock");
    let store_path = workspace.join(".dmz").join("store");
    if !lockfile_path.exists() {
        eprintln!(
            "\x1b[33m[DMZ ADVISORY]\x1b[0m No 'dmz.lock' found in workspace {:?}.\n\
             Tip: Run 'dmz init' or 'dmz bundle' to lock and cache dependencies for full air-gap isolation.\n",
            workspace
        );
    }

    // 2. Resolve shell binary: user option -> $SHELL -> /bin/sh
    let shell_cmd =
        shell.unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()));

    // 3. Assemble command arguments (-c string vs interactive)
    let mut final_args = args;
    if let Some(cmd_str) = command {
        final_args.insert(0, "-c".to_string());
        final_args.insert(1, cmd_str);
    }

    // 4. Prepare environment variables
    let mut envs: std::collections::HashMap<String, String> = std::env::vars().collect();
    let abs_workspace = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    envs.insert(
        "DMZ_WORKSPACE".to_string(),
        abs_workspace.display().to_string(),
    );
    envs.insert(
        "DMZ_STORE".to_string(),
        if store_path.exists() {
            std::fs::canonicalize(&store_path)
                .unwrap_or(store_path)
                .display()
                .to_string()
        } else {
            abs_workspace.join(".dmz/store").display().to_string()
        },
    );
    envs.insert(
        "CARGO_HOME".to_string(),
        abs_workspace.join(".cargo").display().to_string(),
    );
    envs.insert(
        "NODE_PATH".to_string(),
        abs_workspace.join("node_modules").display().to_string(),
    );

    let is_sandbox = !no_sandbox;
    envs.insert(
        "DMZ_SANDBOX".to_string(),
        if is_sandbox {
            "1".to_string()
        } else {
            "0".to_string()
        },
    );

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
        cmd.args(&final_args).current_dir(&workspace).envs(env_vec);

        match cmd.status() {
            Ok(status) => {
                if let Some(code) = status.code() {
                    exit(code);
                }
            }
            Err(e) => {
                error!("Failed to launch host shell '{}': {}", shell_cmd, e);
                exit(1);
            }
        }
    } else {
        info!(
            shell = %shell_cmd,
            network_allowed = allow_network,
            "Launching daemonless sandboxed shell"
        );
        let config = SandboxConfig {
            workspace_path: workspace,
            closure_path: closure,
            command: shell_cmd,
            args: final_args,
            envs: env_vec,
            allow_network,
        };
        match run_sandboxed(&config) {
            Ok(status) => {
                if let Some(code) = status.code() {
                    exit(code);
                }
            }
            Err(e) => {
                error!("Sandbox shell execution failed: {}", e);
                exit(1);
            }
        }
    }
}

pub fn run_init(workspace: PathBuf, target: Option<String>) {
    info!("Initializing project workspace at {:?}", workspace);
    let registry = EcosystemRegistry::new();
    match registry.process_workspace(&workspace) {
        Ok(stacks) => {
            info!(active_stacks = ?stacks, "Discovered active ecosystems");
        }
        Err(e) => {
            error!("Ecosystem discovery failed: {}", e);
            exit(1);
        }
    }

    let dmz_store = workspace.join(".dmz").join("store");
    if let Err(e) = std::fs::create_dir_all(&dmz_store) {
        error!(
            "Failed to initialize store directory {:?}: {}",
            dmz_store, e
        );
        exit(1);
    }

    let lockfile_path = workspace.join("dmz.lock");
    info!("Generating initial closure lock at {:?}", lockfile_path);
    match ResolverEngine::resolve_workspace_with_target(&workspace, target) {
        Ok(lockfile) => {
            if let Err(e) = lockfile.save(&lockfile_path) {
                error!("Failed to save lockfile: {}", e);
                exit(1);
            }
            info!("Successfully created closure lock at {:?}", lockfile_path);
        }
        Err(e) => {
            error!("Dependency resolution failed during init: {}", e);
            exit(1);
        }
    }
    info!("DMZ initialization complete. Run `dmz bundle` to package this workspace.");
}

pub fn run_ecosystem(action: EcosystemAction) {
    match action {
        EcosystemAction::Discover { workspace } => {
            info!(
                "Scanning workspace for active ecosystems at {:?}",
                workspace
            );
            let registry = EcosystemRegistry::new();
            match registry.process_workspace(&workspace) {
                Ok(stacks) => {
                    info!(active_stacks = ?stacks, "Ecosystem discovery completed successfully");
                }
                Err(e) => {
                    error!("Ecosystem discovery failed: {}", e);
                    exit(1);
                }
            }
        }
        EcosystemAction::Verify { workspace } => {
            info!("Verifying workspace ecosystems at {:?}", workspace);
            let registry = EcosystemRegistry::new();
            let active = registry.discover_active(&workspace);
            for eco in active {
                if let Some(cmd) = eco.verification_command(&workspace) {
                    info!(
                        "Running verification for '{}': {} {}",
                        eco.name(),
                        cmd.program,
                        cmd.args.join(" ")
                    );
                    match std::process::Command::new(&cmd.program)
                        .args(&cmd.args)
                        .current_dir(&workspace)
                        .status()
                    {
                        Ok(status) => {
                            if !status.success() {
                                error!("Ecosystem verification failed for '{}'", eco.name());
                                exit(1);
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to execute verification command for '{}': {}",
                                eco.name(),
                                e
                            );
                            exit(1);
                        }
                    }
                }
            }
            info!("All ecosystem verifications passed successfully!");
        }
    }
}
