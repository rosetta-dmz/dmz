use clap::{Parser, Subcommand, ValueEnum};
use dmz_core::archive::exporter::ExportMode as CoreExportMode;
use dmz_core::packaging::BuildTarget;
use std::path::PathBuf;

/// Canonical default paths and settings across DMZ CLI commands
pub const DEFAULT_WORKSPACE_PATH: &str = ".";
pub const DEFAULT_LOCKFILE_PATH: &str = "dmz.lock";
pub const DEFAULT_STORE_PATH: &str = ".dmz/store";
pub const DEFAULT_DIST_PATH: &str = "dist";
pub const DEFAULT_WORKSPACE_CLOSURE_NAME: &str = "workspace_closure";
pub const DEFAULT_CLOSURE_ARCHIVE_PATH: &str = "dist/workspace_closure.tar.zst";
pub const DEFAULT_AIRGAP_ARCHIVE_PATH: &str = "dist/airgap_bundle.tar.zst";
pub const DEFAULT_IMPORT_WORKSPACE_DIR: &str = "workspace";
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

#[derive(Parser, Debug)]
#[command(
    name = "dmz",
    author = "DMZ Open Source Contributors",
    version = env!("CARGO_PKG_VERSION"),
    about = "Universal Daemonless Environment & Workspace Package Manager",
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
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,

        /// Path to companion sidecar workspace root (optional dual-repo mode)
        #[arg(long)]
        sidecar: Option<PathBuf>,

        /// Destination path for generated lockfile
        #[arg(short, long, default_value = DEFAULT_LOCKFILE_PATH)]
        output: PathBuf,

        /// Target platform specification (e.g., macos-arm64, linux-x86_64)
        #[arg(short, long)]
        target: Option<String>,
    },

    /// Fetch and cache locked dependencies into the secure store
    Fetch {
        /// Path to input lockfile
        #[arg(short, long, default_value = DEFAULT_LOCKFILE_PATH)]
        lockfile: PathBuf,

        /// Path to local dependency store directory
        #[arg(short, long, default_value = DEFAULT_STORE_PATH)]
        store: PathBuf,
    },

    /// Install locked dependencies offline from store into project layouts
    Install {
        /// Path to project workspace root
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,

        /// Path to input lockfile
        #[arg(short, long, default_value = DEFAULT_LOCKFILE_PATH)]
        lockfile: PathBuf,

        /// Path to local dependency store directory
        #[arg(short, long, default_value = DEFAULT_STORE_PATH)]
        store: PathBuf,
    },

    /// Build deterministic package artifacts (Target A: Deps, Target B: App, Target C: Unified)
    Build {
        /// Build target specification
        #[arg(short, long, value_enum, default_value_t = TargetMode::Unified, visible_alias = "mode")]
        target: TargetMode,

        /// Path to workspace root directory
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,

        /// Path to input lockfile
        #[arg(short, long, default_value = DEFAULT_LOCKFILE_PATH)]
        lockfile: PathBuf,

        /// Output directory for build artifacts
        #[arg(short, long, default_value = DEFAULT_DIST_PATH)]
        output: PathBuf,

        /// Base name for generated artifact
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_CLOSURE_NAME)]
        name: String,

        /// Zstd compression level (1-22)
        #[arg(long, default_value_t = DEFAULT_ZSTD_LEVEL, value_parser = clap::value_parser!(i32).range(1..=22))]
        zstd_level: i32,
    },

    /// Execute a command inside a native daemonless OS sandbox
    Sandbox {
        /// Path to workspace directory (mounted R/W)
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,

        /// Path to companion sidecar workspace root (optional dual-repo mode)
        #[arg(long)]
        sidecar: Option<PathBuf>,

        /// Path to dependency closure store (mounted R/O)
        #[arg(short, long, default_value = DEFAULT_STORE_PATH)]
        closure: PathBuf,

        /// Permit outbound network connections
        #[arg(long, default_value_t = false)]
        allow_network: bool,

        /// Enable automated diagnostic error categorization
        #[arg(long, default_value_t = false)]
        diagnose: bool,

        /// Binary or script command to execute
        #[arg(default_value = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" })]
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
    Bundle(BundleArgs),

    /// Bundle store items and dmz.lock into a portable air-gap archive (.tar.zst)
    Export(ExportArgs),

    /// Unpack, verify, and restore an air-gap bundle into local store
    Import(ImportArgs),

    /// Open an interactive shell inside the workspace (sandboxed by default)
    Shell {
        /// Path to workspace directory (mounted R/W)
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,

        /// Path to companion sidecar workspace root (optional dual-repo mode)
        #[arg(long)]
        sidecar: Option<PathBuf>,

        /// Path to dependency closure store (mounted R/O)
        #[arg(long, default_value = DEFAULT_STORE_PATH)]
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
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
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

    /// Launch workspace members under coordinated supervisor execution
    Run(RunArgs),
}

/// Shared command arguments for air-gap bundle creation
#[derive(clap::Args, Debug, Clone)]
pub struct BundleArgs {
    /// Target platform specification (e.g., macos-arm64, linux-x86_64)
    #[arg(short, long)]
    pub target: Option<String>,

    /// Path to workspace root directory
    #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
    pub workspace: PathBuf,

    /// Path to companion sidecar workspace root (optional dual-repo mode)
    #[arg(long)]
    pub sidecar: Option<PathBuf>,

    /// Destination path for generated lockfile
    #[arg(short, long, default_value = DEFAULT_LOCKFILE_PATH)]
    pub lockfile: PathBuf,

    /// Path to local dependency store directory
    #[arg(short, long, default_value = DEFAULT_STORE_PATH)]
    pub store: PathBuf,

    /// Destination path for generated output archive
    #[arg(short, long, default_value = DEFAULT_CLOSURE_ARCHIVE_PATH)]
    pub output: PathBuf,

    /// Zstd compression level (1-22)
    #[arg(long, default_value_t = DEFAULT_ZSTD_LEVEL, value_parser = clap::value_parser!(i32).range(1..=22))]
    pub zstd_level: i32,

    /// Air-gap export target mode
    #[arg(long, value_enum, default_value_t = CliExportMode::Unified)]
    pub mode: CliExportMode,

    /// Force re-resolving dependency manifests even if dmz.lock already exists
    #[arg(long, default_value_t = false)]
    pub refresh_lock: bool,

    /// Archive deterministic Git bundle alongside closure
    #[arg(long, default_value_t = false)]
    pub with_git_history: bool,

    /// Baseline commit, ref, or previous manifest/bundle for incremental delta export
    #[arg(long)]
    pub since: Option<String>,

    /// Package active compiler toolchains (cargo, rustc, node, go, python, php, ruby) into bundle
    #[arg(long, default_value_t = true, overrides_with = "no_toolchain")]
    pub with_toolchain: bool,

    /// Omit packaging compiler toolchains into the bundle
    #[arg(long, default_value_t = false, overrides_with = "with_toolchain")]
    pub no_toolchain: bool,
}

impl BundleArgs {
    /// Returns whether toolchain packaging is effectively enabled
    pub fn toolchain_enabled(&self) -> bool {
        self.with_toolchain && !self.no_toolchain
    }
}

/// Shared command arguments for air-gap store archive export
#[derive(clap::Args, Debug, Clone)]
pub struct ExportArgs {
    /// Target platform specification (e.g., macos-arm64, linux-x86_64)
    #[arg(short, long)]
    pub target: Option<String>,

    /// Path to workspace root directory
    #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
    pub workspace: PathBuf,

    /// Path to dmz.lock closure specification
    #[arg(short, long, default_value = DEFAULT_LOCKFILE_PATH)]
    pub lockfile: PathBuf,

    /// Path to local store directory
    #[arg(short, long, default_value = DEFAULT_STORE_PATH)]
    pub store: PathBuf,

    /// Path for generated output archive
    #[arg(short, long, default_value = DEFAULT_AIRGAP_ARCHIVE_PATH)]
    pub output: PathBuf,

    /// Zstd compression level (1-22)
    #[arg(long, default_value_t = DEFAULT_ZSTD_LEVEL, value_parser = clap::value_parser!(i32).range(1..=22))]
    pub zstd_level: i32,

    /// Air-gap export target mode
    #[arg(long, value_enum, default_value_t = CliExportMode::Unified)]
    pub mode: CliExportMode,

    /// Archive deterministic Git bundle alongside closure
    #[arg(long, default_value_t = false)]
    pub with_git_history: bool,

    /// Baseline commit, ref, or previous manifest/bundle for incremental delta export
    #[arg(long)]
    pub since: Option<String>,

    /// Package active compiler toolchains (cargo, rustc, node, go, python, php, ruby) into bundle
    #[arg(long, default_value_t = true, overrides_with = "no_toolchain")]
    pub with_toolchain: bool,

    /// Omit packaging compiler toolchains into the bundle
    #[arg(long, default_value_t = false, overrides_with = "with_toolchain")]
    pub no_toolchain: bool,
}

impl ExportArgs {
    /// Returns whether toolchain packaging is effectively enabled
    pub fn toolchain_enabled(&self) -> bool {
        self.with_toolchain && !self.no_toolchain
    }
}

/// Shared command arguments for air-gap bundle restoration
#[derive(clap::Args, Debug, Clone)]
pub struct ImportArgs {
    /// Path to input .tar.zst air-gap bundle
    #[arg(short, long)]
    pub archive: PathBuf,

    /// Target local store directory
    #[arg(short, long, default_value = DEFAULT_STORE_PATH)]
    pub store: PathBuf,

    /// Target workspace directory for extracted project files
    #[arg(
        short = 't',
        long = "to",
        alias = "workspace",
        default_value = DEFAULT_IMPORT_WORKSPACE_DIR
    )]
    pub workspace: PathBuf,

    /// Skip closure hash verification (NOT recommended)
    #[arg(long, default_value_t = false)]
    pub skip_verify: bool,

    /// Skip automatic offline dependency installation into workspace
    #[arg(long, default_value_t = false)]
    pub skip_install: bool,

    /// Skip automatic ecosystem verification check
    #[arg(long, default_value_t = false)]
    pub skip_ecosystem_verify: bool,

    /// Run verification without sandbox isolation (NOT recommended)
    #[arg(long, default_value_t = false)]
    pub no_sandbox: bool,
}

/// Shared command arguments for workspace execution under supervisor orchestration
#[derive(clap::Args, Debug, Clone)]
pub struct RunArgs {
    /// Path to project workspace root or umbrella directory
    #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
    pub workspace: PathBuf,

    /// Path to companion workspace root (optional override)
    #[arg(long)]
    pub sidecar: Option<PathBuf>,

    /// Permit outbound network connections inside sandboxes
    #[arg(long, default_value_t = false)]
    pub allow_network: bool,

    /// Disable sandbox isolation and execute directly on host
    #[arg(long, default_value_t = false)]
    pub no_sandbox: bool,

    /// Custom primary command override (e.g., "npm run tauri dev")
    #[arg(short = 'c', long = "command")]
    pub command: Option<String>,

    /// Arguments passed to the primary command
    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum EcosystemAction {
    /// Automatically discover active ecosystems in a workspace
    Discover {
        /// Path to workspace root directory
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,
    },
    /// Verify active ecosystems using native engine rules
    Verify {
        /// Path to workspace root directory
        #[arg(short, long, default_value = DEFAULT_WORKSPACE_PATH)]
        workspace: PathBuf,

        /// Run verification without sandbox isolation (NOT recommended)
        #[arg(long, default_value_t = false)]
        no_sandbox: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetMode {
    /// Target A: Package dependencies only
    Dependencies,
    /// Target B: Package repository code only
    App,
    /// Target C: Package repository code and locked dependencies
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
    Bundle(BundleArgs),

    /// Bundle store items and dmz.lock into a portable air-gap archive (.tar.zst)
    Export(ExportArgs),

    /// Unpack and cryptographically verify an air-gap bundle into local store
    Import(ImportArgs),
}
