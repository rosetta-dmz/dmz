use crate::cli::{AirgapCommands, CliExportMode, Commands, EcosystemAction, TargetMode};
use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_core::packaging::{BuildConfig, PackageBuilder};
use dmz_core::resolver::ResolverEngine;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::path::{Path, PathBuf};
use std::process::exit;
use tracing::{error, info, warn};

/// Central entrypoint to dispatch any parsed CLI command to its runner
pub fn dispatch(command: Commands) {
    match command {
        Commands::Resolve {
            workspace,
            sidecar,
            output,
            target,
        } => {
            run_resolve(workspace, sidecar, output, target);
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
            sidecar,
            closure,
            allow_network,
            diagnose,
            exec_command,
            args,
        } => {
            run_sandbox(
                workspace,
                sidecar,
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
            sidecar,
            lockfile,
            store,
            output,
            zstd_level,
            mode,
            refresh_lock,
            with_git_history,
            since,
            with_toolchain,
            no_toolchain,
        } => {
            run_bundle(BundleArgs {
                target,
                workspace,
                sidecar,
                lockfile,
                store,
                output,
                zstd_level,
                mode,
                refresh_lock,
                with_git_history,
                since,
                with_toolchain: with_toolchain && !no_toolchain,
            });
        }

        Commands::Import {
            archive,
            store,
            workspace,
            skip_verify,
            skip_install,
            skip_ecosystem_verify,
            no_sandbox,
        } => {
            run_import(
                archive,
                store,
                workspace,
                skip_verify,
                skip_install,
                skip_ecosystem_verify,
                no_sandbox,
            );
        }

        Commands::Shell {
            workspace,
            sidecar,
            closure,
            allow_network,
            no_sandbox,
            shell,
            command,
            args,
        } => {
            run_shell(
                workspace,
                sidecar,
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
                sidecar,
                lockfile,
                store,
                output,
                zstd_level,
                mode,
                refresh_lock,
                with_git_history,
                since,
                with_toolchain,
                no_toolchain,
            } => {
                run_bundle(BundleArgs {
                    target,
                    workspace,
                    sidecar,
                    lockfile,
                    store,
                    output,
                    zstd_level,
                    mode,
                    refresh_lock,
                    with_git_history,
                    since,
                    with_toolchain: with_toolchain && !no_toolchain,
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
                with_git_history,
                since,
                with_toolchain,
                no_toolchain,
            } => {
                run_export(ExportArgs {
                    target,
                    workspace,
                    lockfile,
                    store,
                    output,
                    zstd_level,
                    mode,
                    with_git_history,
                    since,
                    with_toolchain: with_toolchain && !no_toolchain,
                });
            }

            AirgapCommands::Import {
                archive,
                store,
                workspace,
                skip_verify,
                skip_install,
                skip_ecosystem_verify,
                no_sandbox,
            } => {
                run_import(
                    archive,
                    store,
                    workspace,
                    skip_verify,
                    skip_install,
                    skip_ecosystem_verify,
                    no_sandbox,
                );
            }
        },

        Commands::Ecosystem { action } => {
            run_ecosystem(action);
        }

        Commands::Run {
            workspace,
            sidecar,
            allow_network,
            no_sandbox,
            command,
            args,
        } => {
            run_run(RunArgs {
                workspace,
                sidecar,
                allow_network,
                no_sandbox,
                command,
                args,
            });
        }
    }
}

pub fn run_resolve(
    workspace: PathBuf,
    sidecar: Option<PathBuf>,
    output: PathBuf,
    target: Option<String>,
) {
    info!("Resolving workspace dependencies at {:?}", workspace);
    let effective_output = if output.is_relative() && workspace != Path::new(".") {
        workspace.join(&output)
    } else {
        output
    };

    if let Some(ref sc_path) = sidecar {
        info!(
            "Dual-repo workspace mode active with sidecar at {:?}",
            sc_path
        );

        // Run ABI and dynamic library diagnostic check
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(sc_path);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }

        // Socket path limit advisory
        let sock_path = dmz_core::workspace::safe_socket_path(&workspace, "sidecar");
        if let Some(posix_warn) = dmz_core::workspace::check_socket_path_length(&sock_path) {
            eprintln!("\x1b[33m[DMZ POSIX ADVISORY]\x1b[0m {}", posix_warn);
        }

        let manifest_path = workspace.join("dmz-workspace.toml");
        let project_cfg = if manifest_path.is_file() {
            match dmz_core::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to parse {:?}: {}", manifest_path, e);
                    exit(1);
                }
            }
        } else {
            let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
            let sc_canon = std::fs::canonicalize(sc_path).unwrap_or_else(|_| sc_path.clone());
            let prim_id = ws_canon
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("primary");
            let sc_id = sc_canon
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|s| !s.is_empty() && *s != prim_id)
                .unwrap_or("sidecar");

            let mut primary_m = dmz_core::workspace::WorkspaceMember::new(
                prim_id,
                Path::new("."),
                dmz_core::workspace::MemberRole::Primary,
            );
            primary_m.populate_detected_defaults(&ws_canon, &ws_canon);

            let mut sidecar_m = dmz_core::workspace::WorkspaceMember::new(
                sc_id,
                sc_path,
                dmz_core::workspace::MemberRole::Sidecar,
            );
            sidecar_m.populate_detected_defaults(&sc_canon, &ws_canon);

            let cfg = dmz_core::workspace::WorkspaceProjectConfig::new(
                prim_id,
                vec![primary_m, sidecar_m],
            );
            if let Ok(()) = cfg.save_to_file(&manifest_path) {
                info!(
                    "Auto-generated dual-repo project manifest at {:?}",
                    manifest_path
                );
            }
            cfg
        };

        let unified_lock = match ResolverEngine::resolve_multi_workspace(
            &workspace,
            &project_cfg,
            Some(&manifest_path),
            target,
        ) {
            Ok(lock) => lock,
            Err(e) => {
                error!("Multi-workspace dependency resolution failed: {}", e);
                exit(1);
            }
        };

        if let Err(e) = unified_lock.save(&effective_output) {
            error!("Failed to save lockfile: {}", e);
            exit(1);
        }

        info!(
            "Successfully written dual-workspace closure lock ({} packages) to {:?}",
            unified_lock.packages.len(),
            effective_output
        );
    } else {
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
    sidecar: Option<PathBuf>,
    closure: PathBuf,
    allow_network: bool,
    diagnose: bool,
    exec_command: String,
    args: Vec<String>,
) {
    if let Some(ref sc) = sidecar {
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(sc);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }
    }

    let ipc_socket_path =
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&workspace)
            .map(|(project, _)| {
                dmz_core::workspace::safe_socket_path(&workspace, &project.ipc.socket_id)
            })
            .or_else(|| {
                sidecar
                    .as_ref()
                    .map(|_| dmz_core::workspace::safe_socket_path(&workspace, "sidecar"))
            });

    if let Some(ref sock) = ipc_socket_path {
        if let Some(posix_warn) = dmz_core::workspace::check_socket_path_length(sock) {
            eprintln!("\x1b[33m[DMZ POSIX ADVISORY]\x1b[0m {}", posix_warn);
        }
    }

    let config = SandboxConfig {
        workspace_path: workspace,
        closure_path: closure,
        command: exec_command,
        args,
        envs: std::env::vars().collect(),
        allow_network,
        ipc_socket_path,
    };

    match run_sandboxed(&config) {
        Ok(status) => {
            if let Some(code) = status.code() {
                exit(code);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    exit(128 + sig);
                }
            }
            if !status.success() {
                exit(1);
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
    pub sidecar: Option<PathBuf>,
    pub lockfile: PathBuf,
    pub store: PathBuf,
    pub output: PathBuf,
    pub zstd_level: i32,
    pub mode: CliExportMode,
    pub refresh_lock: bool,
    pub with_git_history: bool,
    pub since: Option<String>,
    pub with_toolchain: bool,
}

pub fn run_bundle(args: BundleArgs) {
    info!(
        "Starting streamlined air-gap bundling for workspace {:?}",
        args.workspace
    );

    if let Some(ref sc) = args.sidecar {
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(sc);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }
    }

    let ws_canon =
        std::fs::canonicalize(&args.workspace).unwrap_or_else(|_| args.workspace.clone());

    let effective_lockfile = if args.lockfile == Path::new("dmz.lock") {
        ws_canon.join("dmz.lock")
    } else if args.lockfile.is_relative() {
        if ws_canon.join(&args.lockfile).exists() {
            ws_canon.join(&args.lockfile)
        } else if args.lockfile.exists() {
            std::fs::canonicalize(&args.lockfile).unwrap_or(args.lockfile)
        } else {
            ws_canon.join(&args.lockfile)
        }
    } else {
        args.lockfile
    };

    let effective_store = if args.store == Path::new(".dmz/store") {
        ws_canon.join(".dmz/store")
    } else if args.store.is_relative() {
        if ws_canon.join(&args.store).exists() {
            ws_canon.join(&args.store)
        } else if args.store.exists() {
            std::fs::canonicalize(&args.store).unwrap_or(args.store)
        } else {
            ws_canon.join(&args.store)
        }
    } else {
        args.store
    };

    let multi_workspace =
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&ws_canon)
            .map(|(cfg, _)| cfg)
            .or_else(|| {
                args.sidecar.as_ref().map(|sc| {
                    let sc_canon = std::fs::canonicalize(sc).unwrap_or_else(|_| sc.clone());
                    let prim_id = ws_canon
                        .file_name()
                        .and_then(|n| n.to_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("primary");
                    let sc_id = sc_canon
                        .file_name()
                        .and_then(|n| n.to_str())
                        .filter(|s| !s.is_empty() && *s != prim_id)
                        .unwrap_or("sidecar");

                    let mut primary = dmz_core::workspace::WorkspaceMember::new(
                        prim_id,
                        Path::new("."),
                        dmz_core::workspace::MemberRole::Primary,
                    );
                    primary.populate_detected_defaults(&ws_canon, &ws_canon);

                    let mut sidecar_m = dmz_core::workspace::WorkspaceMember::new(
                        sc_id,
                        sc,
                        dmz_core::workspace::MemberRole::Sidecar,
                    );
                    sidecar_m.populate_detected_defaults(&sc_canon, &ws_canon);

                    let cfg = dmz_core::workspace::WorkspaceProjectConfig::new(
                        prim_id,
                        vec![primary, sidecar_m],
                    );
                    let manifest_path = ws_canon.join("dmz-workspace.toml");
                    if !manifest_path.exists() {
                        let _ = cfg.save_to_file(&manifest_path);
                    }
                    cfg
                })
            });

    // Resolve manifests if lockfile is missing or refresh_lock is requested
    let should_resolve = args.refresh_lock || !effective_lockfile.exists();
    if should_resolve {
        info!("Resolving workspace dependency manifests at {:?}", ws_canon);
        let resolve_res = if let Some(ref project_cfg) = multi_workspace {
            let manifest_file = ws_canon.join("dmz-workspace.toml");
            ResolverEngine::resolve_multi_workspace(
                &ws_canon,
                project_cfg,
                manifest_file.is_file().then_some(manifest_file.as_path()),
                args.target.clone(),
            )
        } else {
            ResolverEngine::resolve_workspace_with_target(&ws_canon, args.target.clone())
        };

        match resolve_res {
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
            exit(1);
        }
    }

    // Export unified air-gap archive
    let effective_output = if args.output == Path::new("dist/workspace_closure.tar.zst") {
        ws_canon.join("dist/workspace_closure.tar.zst")
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
        with_toolchain: args.with_toolchain,
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
        }
        Err(e) => {
            error!("Air-gap bundle export failed: {}", e);
            exit(1);
        }
    }
}

pub struct ExportArgs {
    pub target: Option<String>,
    pub workspace: PathBuf,
    pub lockfile: PathBuf,
    pub store: PathBuf,
    pub output: PathBuf,
    pub zstd_level: i32,
    pub mode: CliExportMode,
    pub with_git_history: bool,
    pub since: Option<String>,
    pub with_toolchain: bool,
}

pub fn run_export(args: ExportArgs) {
    let ws_canon =
        std::fs::canonicalize(&args.workspace).unwrap_or_else(|_| args.workspace.clone());

    let effective_lockfile = if args.lockfile == Path::new("dmz.lock") {
        ws_canon.join("dmz.lock")
    } else if args.lockfile.is_relative() {
        if ws_canon.join(&args.lockfile).exists() {
            ws_canon.join(&args.lockfile)
        } else if args.lockfile.exists() {
            std::fs::canonicalize(&args.lockfile).unwrap_or(args.lockfile)
        } else {
            ws_canon.join(&args.lockfile)
        }
    } else {
        args.lockfile
    };

    let effective_store = if args.store == Path::new(".dmz/store") {
        ws_canon.join(".dmz/store")
    } else if args.store.is_relative() {
        if ws_canon.join(&args.store).exists() {
            ws_canon.join(&args.store)
        } else if args.store.exists() {
            std::fs::canonicalize(&args.store).unwrap_or(args.store)
        } else {
            ws_canon.join(&args.store)
        }
    } else {
        args.store
    };

    let effective_output = if args.output == Path::new("dist/airgap_bundle.tar.zst")
        || args.output == Path::new("dist/workspace_closure.tar.zst")
    {
        ws_canon.join("dist/workspace_closure.tar.zst")
    } else if args.output.is_relative() {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&args.output)
    } else {
        args.output
    };

    let multi_workspace =
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&ws_canon)
            .map(|(cfg, _)| cfg);

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
        with_toolchain: args.with_toolchain,
        multi_workspace,
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
    no_sandbox: bool,
) {
    let _ = std::fs::create_dir_all(&workspace);
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    let effective_archive = if archive.is_relative() {
        if archive.exists() {
            std::fs::canonicalize(&archive).unwrap_or(archive)
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&archive)
        }
    } else {
        archive
    };

    let effective_store = if store == Path::new(".dmz/store") {
        ws_canon.join(".dmz/store")
    } else if store.is_relative() {
        if ws_canon.join(&store).exists() {
            ws_canon.join(&store)
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&store)
        }
    } else {
        store
    };

    let config = ImportConfig {
        archive_path: effective_archive,
        target_store_dir: effective_store.clone(),
        target_workspace_dir: ws_canon.clone(),
        verify_closure_hash: !skip_verify,
    };

    match AirGapImporter::import(&config) {
        Ok(result) => {
            info!(
                imported = result.imported_packages,
                closure = %result.closure_signature,
                is_delta = result.is_delta,
                has_git = result.has_git_history,
                vcs = ?result.vcs,
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
        let lockfile_path = ws_canon.join("dmz.lock");
        info!(
            "Running automated offline dependency installation in workspace {:?}",
            ws_canon
        );
        let install_config = dmz_core::install::InstallConfig {
            workspace_path: ws_canon.clone(),
            lockfile_path,
            store_dir: effective_store.clone(),
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
                ws_canon
            );
            verify_workspace_ecosystems(&ws_canon, &effective_store, no_sandbox);
            info!("All ecosystem verifications passed successfully!");
        }
    }
}

fn verify_workspace_ecosystems(ws_canon: &Path, closure_path: &Path, no_sandbox: bool) {
    let registry = EcosystemRegistry::new();
    let mut member_roots = Vec::new();
    if let Some((project, manifest_path)) =
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(ws_canon)
    {
        for layout in project.resolve_member_layouts(ws_canon, Some(&manifest_path)) {
            if layout.host_path.is_dir() && !member_roots.contains(&layout.host_path) {
                member_roots.push(layout.host_path);
            }
        }
    }
    if member_roots.is_empty() {
        member_roots.push(ws_canon.to_path_buf());
    }

    for member_root in member_roots {
        let active = registry.discover_active(&member_root);
        for eco in active {
            if let Some(cmd) = eco.verification_command(&member_root) {
                info!(
                    "Running verification for '{}' in {:?}: {} {}",
                    eco.name(),
                    member_root,
                    cmd.program,
                    cmd.args.join(" ")
                );

                let mut toolchain_envs = std::collections::HashMap::new();
                dmz_core::toolchain::ToolchainPackager::inject_toolchain_paths(
                    ws_canon,
                    &mut toolchain_envs,
                );

                let status_res = if no_sandbox {
                    info!("Executing verification unsandboxed (--no-sandbox requested)");
                    let mut proc_cmd = std::process::Command::new(&cmd.program);
                    proc_cmd.args(&cmd.args).current_dir(&member_root);
                    for (k, v) in toolchain_envs {
                        proc_cmd.env(k, v);
                    }
                    proc_cmd.status().map_err(|e| e.to_string())
                } else {
                    info!("Executing verification inside zero-network sandbox");
                    let config = SandboxConfig {
                        workspace_path: member_root.clone(),
                        closure_path: closure_path.to_path_buf(),
                        command: cmd.program.clone(),
                        args: cmd.args.clone(),
                        envs: toolchain_envs.into_iter().collect(),
                        allow_network: false,
                        ipc_socket_path: None,
                    };
                    run_sandboxed(&config).map_err(|e| e.to_string())
                };

                match status_res {
                    Ok(status) => {
                        if !status.success() {
                            error!("Ecosystem verification failed for '{}'", eco.name());
                            exit(1);
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to execute sandboxed verification command for '{}': {}. Pass --no-sandbox to bypass sandbox containment if required by your environment.",
                            eco.name(),
                            e
                        );
                        exit(1);
                    }
                }
            }
        }
    }
}

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
) {
    if let Some(ref sc) = sidecar {
        let abi_report = dmz_core::workspace::analyze_sidecar_abi(sc);
        for warning in &abi_report.warnings {
            eprintln!("\x1b[33m[DMZ ABI DIAGNOSTIC]\x1b[0m {}", warning);
        }
    }

    // Advisory pre-flight check for dmz.lock
    let lockfile_path = workspace.join("dmz.lock");
    let store_path = workspace.join(".dmz").join("store");
    if !lockfile_path.exists() {
        eprintln!(
            "\x1b[33m[DMZ ADVISORY]\x1b[0m No 'dmz.lock' found in workspace {:?}.\n\
             Tip: Run 'dmz init' or 'dmz bundle' to lock and cache dependencies for full air-gap isolation.\n",
            workspace
        );
    }

    // Resolve shell binary cross-platform: user option -> $SHELL/$COMSPEC -> /bin/sh or cmd.exe
    let shell_cmd = shell.unwrap_or_else(|| {
        if cfg!(windows) {
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        }
    });

    // Assemble command arguments (/C vs -c string vs interactive)
    let mut final_args = args;
    if let Some(cmd_str) = command {
        let exec_flag = if cfg!(windows) && shell_cmd.to_lowercase().contains("cmd") {
            "/C"
        } else {
            "-c"
        };
        final_args.insert(0, exec_flag.to_string());
        final_args.insert(1, cmd_str);
    }

    // Prepare environment variables
    let abs_workspace = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    let mut envs = dmz_core::workspace::WorkspaceProjectConfig::default().build_member_env(
        &abs_workspace,
        &abs_workspace,
        std::path::Path::new(""),
        allow_network,
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
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&workspace)
            .map(|(project, _)| {
                dmz_core::workspace::safe_socket_path(&workspace, &project.ipc.socket_id)
            })
            .or_else(|| {
                sidecar
                    .as_ref()
                    .map(|_| dmz_core::workspace::safe_socket_path(&workspace, "sidecar"))
            });

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
            ipc_socket_path,
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
        EcosystemAction::Verify {
            workspace,
            no_sandbox,
        } => {
            info!("Verifying workspace ecosystems at {:?}", workspace);
            let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
            let closure_path = ws_canon.join(".dmz/store");
            verify_workspace_ecosystems(&ws_canon, &closure_path, no_sandbox);
            info!("All ecosystem verifications passed successfully!");
        }
    }
}

pub struct RunArgs {
    pub workspace: PathBuf,
    pub sidecar: Option<PathBuf>,
    pub allow_network: bool,
    pub no_sandbox: bool,
    pub command: Option<String>,
    pub args: Vec<String>,
}

pub fn run_run(args: RunArgs) {
    info!(
        workspace = ?args.workspace,
        "Initializing workspace execution under supervisor orchestration"
    );

    let (mut project_config, effective_workspace_root) = if let Some((cfg, manifest_file)) =
        dmz_core::workspace::WorkspaceProjectConfig::discover_workspace_manifest(&args.workspace)
    {
        let root = manifest_file
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        info!(
            manifest = ?manifest_file,
            "Loaded workspace project configuration"
        );
        (cfg, root)
    } else {
        let ws_name = args
            .workspace
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
            let sc_name = sc
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "companion".to_string());
            members.push(dmz_core::workspace::WorkspaceMember::new(
                sc_name,
                sc.clone(),
                dmz_core::workspace::MemberRole::Sidecar,
            ));
        }

        let cfg = dmz_core::workspace::WorkspaceProjectConfig::new(ws_name, members);
        (cfg, args.workspace.clone())
    };

    // Apply CLI command override to the primary member if provided;
    // otherwise SidecarOrchestrator delegates to WorkspaceMember::resolve_command.
    if let Some(ref cmd) = args.command {
        for member in &mut project_config.members {
            if member.role == dmz_core::workspace::MemberRole::Primary {
                member.exec_command = Some(cmd.clone());
                if !args.args.is_empty() {
                    member.exec_args = Some(args.args.clone());
                }
                break;
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
            exit(1);
        }
    };

    rt.block_on(async {
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
                exit(1);
            }
        }

        tokio::select! {
            exit_res = orchestrator.wait_for_exit() => {
                match exit_res {
                    Ok(status) => info!("Primary process exited with status: {}", status),
                    Err(e) => warn!("Wait error: {}", e),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Interrupt received (SIGINT); draining workspace pair gracefully...");
            }
        }

        let _ = orchestrator.stop_pair().await;
        info!("Workspace supervision terminated cleanly");
    });
}
