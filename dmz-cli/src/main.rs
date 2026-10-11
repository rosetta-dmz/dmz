use clap::{Parser, Subcommand, ValueEnum};
use dmz_core::archive::exporter::ExportMode as CoreExportMode;
use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::packaging::{BuildConfig, BuildTarget, PackageBuilder};
use dmz_core::resolver::ResolverEngine;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::path::PathBuf;
use std::process::exit;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(
    name = "dmz",
    author = "DMZ Open Source Contributors",
    version = env!("CARGO_PKG_VERSION"),
    about = "Universal Daemonless Environment & Packaging Engine",
    long_about = "DMZ provides zero-dependency, daemonless process isolation, dependency resolution, build targeting, and air-gap distribution."
)]
struct Cli {
    /// Verbose logging output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan workspace manifests and generate a deterministic dmz.lock closure
    Resolve {
        /// Path to project workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Destination path for generated lockfile
        #[arg(short, long, default_value = "dmz.lock")]
        output: PathBuf,
    },

    /// Fetch and cache locked dependencies into the secure store
    Fetch {
        #[arg(long, default_value = "dmz.lock")]
        lockfile: PathBuf,
        #[arg(long, default_value = ".dmz/store")]
        store: PathBuf,
    },

    /// Build deterministic package artifacts (Target A: Deps, Target B: App, Target C: Unified)
    Build {
        /// Build target specification
        #[arg(short, long, value_enum, default_value_t = TargetMode::Unified)]
        target: TargetMode,

        /// Path to workspace root directory
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to input lockfile
        #[arg(short, long, default_value = "dmz.lock")]
        lockfile: PathBuf,

        /// Output directory for build artifacts
        #[arg(short, long, default_value = "dist")]
        output: PathBuf,

        /// Base name for generated artifact
        #[arg(short, long, default_value = "workspace_closure")]
        name: String,

        /// Zstd compression level (1-22)
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,
    },

    /// Execute a command inside a native daemonless OS sandbox
    Sandbox {
        /// Path to workspace directory (mounted R/W)
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to dependency closure store (mounted R/O)
        #[arg(short, long, default_value = "/nix/store")]
        closure: PathBuf,

        /// Permit outbound network connections
        #[arg(long, default_value_t = false)]
        allow_network: bool,

        /// Enable automated diagnostic error categorization
        #[arg(long, default_value_t = false)]
        diagnose: bool,

        /// Binary or script command to execute
        #[arg(default_value = "/bin/sh")]
        exec_command: String,

        /// Command-line arguments passed to the executed process
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Manage offline, air-gapped package bundles
    Airgap {
        #[command(subcommand)]
        action: AirgapCommands,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum TargetMode {
    /// Target A: Package dependencies only
    Dependencies,
    /// Target B: Package repository code only
    App,
    /// Target C: Package repository code combined with locked dependencies
    Unified,
}

impl From<TargetMode> for BuildTarget {
    fn from(mode: TargetMode) -> Self {
        match mode {
            TargetMode::Dependencies => BuildTarget::DependenciesOnly,
            TargetMode::App => BuildTarget::AppOnly,
            TargetMode::Unified => BuildTarget::Unified,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum CliExportMode {
    /// Workspace source code + dependency store archives
    Unified,
    /// Workspace source code only (no dependencies)
    CodeOnly,
    /// Dependency store archives only (no source code)
    DepsOnly,
}

impl From<CliExportMode> for CoreExportMode {
    fn from(mode: CliExportMode) -> Self {
        match mode {
            CliExportMode::Unified => CoreExportMode::Unified,
            CliExportMode::CodeOnly => CoreExportMode::CodeOnly,
            CliExportMode::DepsOnly => CoreExportMode::DepsOnly,
        }
    }
}

#[derive(Subcommand)]
enum AirgapCommands {
    /// Bundle store items and dmz.lock into a portable air-gap archive (.tar.zst)
    Export {
        /// Path to workspace root directory
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to dmz.lock closure specification
        #[arg(short, long, default_value = "dmz.lock")]
        lockfile: PathBuf,

        /// Path to local store directory
        #[arg(short, long, default_value = ".dmz/store")]
        store: PathBuf,

        /// Path for generated output archive
        #[arg(short, long, default_value = "dist/airgap_bundle.tar.zst")]
        output: PathBuf,

        /// Zstd compression level (1-22)
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,

        /// Air-gap export target mode
        #[arg(long, value_enum, default_value_t = CliExportMode::Unified)]
        mode: CliExportMode,
    },

    /// Unpack and cryptographically verify an air-gap bundle into local store
    Import {
        /// Path to input .tar.zst air-gap bundle
        #[arg(short, long)]
        archive: PathBuf,

        /// Target local store directory
        #[arg(short, long, default_value = ".dmz/store")]
        store: PathBuf,

        /// Skip closure hash verification (NOT recommended)
        #[arg(long, default_value_t = false)]
        skip_verify: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    // Initialize logging output subscriber[cite: 1]
    let log_level = if cli.verbose { Level::DEBUG } else { Level::INFO };
    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set tracing subscriber");

    // Dispatch CLI commands[cite: 1]
    match cli.command {
        Commands::Resolve { workspace, output } => {
            info!("Resolving workspace dependencies at {:?}", workspace);
            match ResolverEngine::resolve_workspace(&workspace) {
                Ok(lockfile) => {
                    if let Err(e) = lockfile.save(&output) {
                        error!("Failed to save lockfile: {}", e);
                        exit(1);
                    }
                    info!("Successfully written closure lock to {:?}", output);
                }
                Err(e) => {
                    error!("Dependency resolution failed: {}", e);
                    exit(1);
                }
            }
        }

        Commands::Fetch { lockfile, store } => {
            info!("Starting dependency fetch from lockfile: {:?}", lockfile);
            match dmz_core::ResolverEngine::fetch_dependencies(&lockfile, &store) {
                Ok(count) => {
                    info!("Successfully fetched {} dependencies into store {:?}", count, store);
                }
                Err(e) => {
                    error!("Dependency fetch failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Build {
            target,
            workspace,
            lockfile,
            output,
            name,
            zstd_level,
        } => {
            let config = BuildConfig {
                workspace_dir: workspace,
                lockfile_path: lockfile,
                output_dir: output,
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

        Commands::Sandbox {
            workspace,
            closure,
            allow_network,
            diagnose,
            exec_command,
            args,
        } => {
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

        Commands::Airgap { action } => match action {
            AirgapCommands::Export {
                workspace,
                lockfile,
                store,
                output,
                zstd_level,
                mode,
            } => {
                let config = ExportConfig {
                    workspace_path: workspace,
                    lockfile_path: lockfile,
                    store_dir: store,
                    output_path: output,
                    zstd_level,
                    mode: mode.into(),
                };

                match AirGapExporter::export(&config) {
                    Ok(manifest) => {
                        info!(
                            checksum = %manifest.archive_sha256,
                            packages = manifest.package_count,
                            "Air-gap export completed successfully"
                        );
                    }
                    Err(e) => {
                        error!("Air-gap export failed: {}", e);
                        exit(1);
                    }
                }
            }

            AirgapCommands::Import {
                archive,
                store,
                skip_verify,
            } => {
                let config = ImportConfig {
                    archive_path: archive,
                    target_store_dir: store,
                    verify_closure_hash: !skip_verify,
                    ..ImportConfig::default()
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
            }
        },
    }
}
