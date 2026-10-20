use clap::{Parser, Subcommand, ValueEnum};
use dmz_core::archive::exporter::ExportMode as CoreExportMode;
use dmz_core::packaging::BuildTarget;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "dmz",
    author = "DMZ Open Source Contributors",
    version = env!("CARGO_PKG_VERSION"),
    about = "Universal Daemonless Environment & Packaging Engine",
    long_about = "DMZ provides zero-dependency, daemonless process isolation, dependency resolution, build targeting, and air-gap distribution."
)]
pub struct Cli {
    /// Verbose logging output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan workspace manifests and generate a deterministic dmz.lock closure
    Resolve {
        /// Path to project workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Destination path for generated lockfile
        #[arg(short, long, default_value = "dmz.lock")]
        output: PathBuf,

        /// Target platform specification (e.g., macos-arm64, linux-x86_64)
        #[arg(short, long)]
        target: Option<String>,
    },

    /// Fetch and cache locked dependencies into the secure store
    Fetch {
        #[arg(long, default_value = "dmz.lock")]
        lockfile: PathBuf,
        #[arg(long, default_value = ".dmz/store")]
        store: PathBuf,
    },

    /// Install locked dependencies offline from store into project layouts
    Install {
        /// Path to project workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to input lockfile
        #[arg(short, long, default_value = "dmz.lock")]
        lockfile: PathBuf,

        /// Path to local dependency store directory
        #[arg(short, long, default_value = ".dmz/store")]
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

    /// One-shot resolution, dependency fetching, and export into an air-gap bundle (.tar.zst)
    Bundle {
        /// Target platform specification (e.g., macos-arm64, linux-x86_64)
        #[arg(short, long)]
        target: Option<String>,

        /// Path to workspace root directory
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Destination path for generated lockfile
        #[arg(short, long, default_value = "dmz.lock")]
        lockfile: PathBuf,

        /// Path to local dependency store directory
        #[arg(short, long, default_value = ".dmz/store")]
        store: PathBuf,

        /// Destination path for generated output archive
        #[arg(short, long, default_value = "dist/workspace_closure.tar.zst")]
        output: PathBuf,

        /// Zstd compression level (1-22)
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,

        /// Air-gap export target mode
        #[arg(long, value_enum, default_value_t = CliExportMode::Unified)]
        mode: CliExportMode,

        /// Force re-resolving dependency manifests even if dmz.lock already exists
        #[arg(long, default_value_t = false)]
        refresh_lock: bool,
    },

    /// Unpack, verify, and restore an air-gap bundle into local store
    Import {
        /// Path to input .tar.zst air-gap bundle
        #[arg(short, long)]
        archive: PathBuf,

        /// Target local store directory
        #[arg(short, long, default_value = ".dmz/store")]
        store: PathBuf,

        /// Target workspace directory for extracted project files
        #[arg(
            short = 't',
            long = "to",
            alias = "workspace",
            default_value = "workspace"
        )]
        workspace: PathBuf,

        /// Skip closure hash verification (NOT recommended)
        #[arg(long, default_value_t = false)]
        skip_verify: bool,

        /// Skip automatic offline dependency installation into workspace
        #[arg(long, default_value_t = false)]
        skip_install: bool,

        /// Skip automatic ecosystem verification check
        #[arg(long, default_value_t = false)]
        skip_ecosystem_verify: bool,
    },

    /// Open an interactive shell inside the workspace (sandboxed by default)
    Shell {
        /// Path to workspace directory (mounted R/W)
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to dependency closure store (mounted R/O)
        #[arg(long, default_value = "/nix/store")]
        closure: PathBuf,

        /// Execute command string in shell and exit (e.g. -c "cargo test")
        #[arg(short = 'c', long = "command")]
        command: Option<String>,

        /// Permit outbound network connections inside the sandbox
        #[arg(long, default_value_t = false)]
        allow_network: bool,

        /// Disable sandbox isolation and execute directly on the host
        #[arg(long, default_value_t = false)]
        no_sandbox: bool,

        /// Shell binary or command to execute (defaults to $SHELL or /bin/sh)
        #[arg(short, long)]
        shell: Option<String>,

        /// Command-line arguments passed to the shell process
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Initialize a project workspace for DMZ capture
    Init {
        /// Path to project workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Target platform specification (e.g., macos-arm64, linux-x86_64)
        #[arg(short, long)]
        target: Option<String>,
    },

    /// Inspect and manage multi-ecosystem workspace stacks (Rust, Node, Go, Python, etc.)
    Ecosystem {
        #[command(subcommand)]
        action: EcosystemAction,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum EcosystemAction {
    /// Automatically discover active ecosystems in a workspace
    Discover {
        /// Path to workspace root directory
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Verify active ecosystems using native engine rules
    Verify {
        /// Path to workspace root directory
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetMode {
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
pub enum CliExportMode {
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

#[derive(Subcommand, Debug, Clone)]
pub enum AirgapCommands {
    /// One-shot resolution, dependency fetching, and export into an air-gap bundle (.tar.zst)
    #[command(alias = "pack")]
    Bundle {
        /// Target platform specification (e.g., macos-arm64, linux-x86_64)
        #[arg(short, long)]
        target: Option<String>,

        /// Path to workspace root directory
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Destination path for generated lockfile
        #[arg(short, long, default_value = "dmz.lock")]
        lockfile: PathBuf,

        /// Path to local dependency store directory
        #[arg(short, long, default_value = ".dmz/store")]
        store: PathBuf,

        /// Destination path for generated output archive
        #[arg(short, long, default_value = "dist/workspace_closure.tar.zst")]
        output: PathBuf,

        /// Zstd compression level (1-22)
        #[arg(long, default_value_t = 3)]
        zstd_level: i32,

        /// Air-gap export target mode
        #[arg(long, value_enum, default_value_t = CliExportMode::Unified)]
        mode: CliExportMode,

        /// Force re-resolving dependency manifests even if dmz.lock already exists
        #[arg(long, default_value_t = false)]
        refresh_lock: bool,
    },

    /// Bundle store items and dmz.lock into a portable air-gap archive (.tar.zst)
    Export {
        /// Target platform specification (e.g., macos-arm64, linux-x86_64)
        #[arg(short, long)]
        target: Option<String>,

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

        /// Target workspace directory for extracted project files
        #[arg(
            short = 't',
            long = "to",
            alias = "workspace",
            default_value = "workspace"
        )]
        workspace: PathBuf,

        /// Skip closure hash verification (NOT recommended)
        #[arg(long, default_value_t = false)]
        skip_verify: bool,

        /// Skip automatic offline dependency installation into workspace
        #[arg(long, default_value_t = false)]
        skip_install: bool,

        /// Skip automatic ecosystem verification check
        #[arg(long, default_value_t = false)]
        skip_ecosystem_verify: bool,
    },
}
