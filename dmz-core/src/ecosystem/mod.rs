pub mod handlers;

use std::path::{Path, PathBuf};
use tracing::info;

/// Typed domain error for ecosystem discovery, resolution, and offline materialization.
#[derive(Debug)]
pub enum EcosystemError {
    Io {
        context: String,
        source: std::io::Error,
    },
    Install(crate::install::InstallError),
}

impl std::fmt::Display for EcosystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{}: {}", context, source),
            Self::Install(err) => write!(f, "Offline install error: {}", err),
        }
    }
}

impl std::error::Error for EcosystemError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Install(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for EcosystemError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            context: "Ecosystem I/O operation failed".to_string(),
            source,
        }
    }
}

impl From<crate::install::InstallError> for EcosystemError {
    fn from(err: crate::install::InstallError) -> Self {
        Self::Install(err)
    }
}

/// Command configuration for delegated resolution and verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    /// Resolves the command program path, checking bundled `.dmz/toolchains` first before falling back to system binary
    pub fn resolve_in_workspace(&self, workspace_root: &Path) -> Self {
        let toolchains_dir = workspace_root.join(".dmz").join("toolchains");
        if toolchains_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&toolchains_dir) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    let mut candidates = vec![
                        entry_path.join("bin").join(&self.program),
                        entry_path.join("bin").join(format!("{}.exe", self.program)),
                        entry_path.join(&self.program),
                        entry_path.join(format!("{}.exe", self.program)),
                    ];

                    // Windows Python distributions commonly ship `python.exe` rather than `python3.exe`
                    if self.program == "python3" {
                        candidates.push(entry_path.join("bin").join("python.exe"));
                        candidates.push(entry_path.join("python.exe"));
                        candidates.push(entry_path.join("bin").join("python"));
                    }

                    for candidate in candidates {
                        if candidate.is_file() {
                            return Self {
                                program: candidate.display().to_string(),
                                args: self.args.clone(),
                            };
                        }
                    }
                }
            }
        }
        self.clone()
    }
}

/// Synchronous trait for workspace ecosystem detection, verification, and dependency materialization.
///
/// Note: Implementations perform synchronous filesystem checks and process execution; callers on
/// async Tokio worker threads must invoke these methods inside `tokio::task::spawn_blocking`.
pub trait EcosystemHandler: Send + Sync {
    fn name(&self) -> &str;
    fn detect(&self, workspace_root: &Path) -> bool;
    fn cache_paths(&self, workspace_root: &Path) -> Vec<PathBuf>;
    fn resolution_command(&self) -> Option<CommandSpec>;

    /// Resolves the resolution command for a specific workspace root, checking bundled `.dmz/toolchains`.
    fn resolution_command_for_workspace(&self, workspace_root: &Path) -> Option<CommandSpec> {
        self.resolution_command()
            .map(|cmd| cmd.resolve_in_workspace(workspace_root))
    }

    /// Alias for backwards compatibility.
    fn resolution_command_in_workspace(&self, workspace_root: &Path) -> Option<CommandSpec> {
        self.resolution_command_for_workspace(workspace_root)
    }

    fn verification_command(&self, workspace_root: &Path) -> Option<CommandSpec>;

    /// Default: invokes unified InstallerEngine to materialize dependencies
    fn materialize(
        &self,
        workspace_root: &Path,
        offline_store: &Path,
        lockfile_path: &Path,
    ) -> Result<(), EcosystemError> {
        let config = crate::install::InstallConfig {
            workspace_path: workspace_root.to_path_buf(),
            lockfile_path: lockfile_path.to_path_buf(),
            store_dir: offline_store.to_path_buf(),
        };
        crate::install::InstallerEngine::install(&config)?;
        Ok(())
    }
}

/// Registry that aggregates and orchestrates all ecosystem handlers.
pub struct EcosystemRegistry {
    handlers: Vec<Box<dyn EcosystemHandler>>,
}

impl Default for EcosystemRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EcosystemRegistry {
    pub fn new() -> Self {
        Self {
            handlers: vec![
                Box::new(handlers::RustEcosystem),
                Box::new(handlers::PnpmEcosystem),
                Box::new(handlers::YarnEcosystem),
                Box::new(handlers::NpmEcosystem),
                Box::new(handlers::GoEcosystem),
                Box::new(handlers::PythonEcosystem),
                Box::new(handlers::PhpEcosystem),
                Box::new(handlers::RubyEcosystem),
                Box::new(handlers::MavenGradleEcosystem),
            ],
        }
    }

    /// Returns the workspace root plus any member directories declared in `dmz-workspace.toml`.
    pub fn candidate_roots(workspace_root: &Path) -> Vec<PathBuf> {
        let mut roots = vec![workspace_root.to_path_buf()];
        let manifest_path = workspace_root.join("dmz-workspace.toml");
        if manifest_path.is_file() {
            if let Ok(ws_cfg) =
                crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
            {
                for layout in ws_cfg.resolve_member_layouts(workspace_root, Some(&manifest_path)) {
                    if layout.host_path.is_dir() && !roots.contains(&layout.host_path) {
                        roots.push(layout.host_path);
                    }
                }
            }
        }
        roots
    }

    /// Automatically scans the workspace root (and any umbrella member roots) and returns all active handlers.
    pub fn discover_active(&self, workspace_root: &Path) -> Vec<&dyn EcosystemHandler> {
        let roots = Self::candidate_roots(workspace_root);
        self.handlers
            .iter()
            .filter(|h| roots.iter().any(|r| h.detect(r)))
            .map(|h| h.as_ref())
            .collect()
    }

    /// Returns each active `(EcosystemHandler, target_member_root)` pair for umbrella-aware verification.
    pub fn discover_active_with_roots(
        &self,
        workspace_root: &Path,
    ) -> Vec<(&dyn EcosystemHandler, PathBuf)> {
        let roots = Self::candidate_roots(workspace_root);
        let mut pairs = Vec::new();
        for h in &self.handlers {
            for r in &roots {
                if h.detect(r) {
                    pairs.push((h.as_ref(), r.clone()));
                }
            }
        }
        pairs
    }

    /// Processes the workspace, logging detected stacks and returning their names.
    pub fn process_workspace(&self, workspace_root: &Path) -> Result<Vec<String>, EcosystemError> {
        let active = self.discover_active(workspace_root);
        let mut detected_names = Vec::new();

        for eco in active {
            detected_names.push(eco.name().to_string());
            if let Some(cmd) = eco.resolution_command_for_workspace(workspace_root) {
                info!(
                    ecosystem = eco.name(),
                    program = %cmd.program,
                    args = ?cmd.args,
                    "Detected active ecosystem stack"
                );
                println!(
                    "[dmz ecosystem] Detected '{}' stack. Resolution rule: {} {}",
                    eco.name(),
                    cmd.program,
                    cmd.args.join(" ")
                );
            }
        }
        Ok(detected_names)
    }

    /// Gathers all generalized cache and store paths across active ecosystems.
    pub fn gather_all_caches(&self, workspace_root: &Path) -> Vec<PathBuf> {
        let active = self.discover_active(workspace_root);
        let mut all_paths = Vec::new();
        for eco in active {
            for path in eco.cache_paths(workspace_root) {
                if path.exists() {
                    all_paths.push(path);
                }
            }
        }
        all_paths
    }

    /// Iterates through all dynamically discovered active ecosystems and materializes them.
    pub fn install_workspace(
        &self,
        workspace_root: &Path,
        offline_store: &Path,
        lockfile_path: &Path,
    ) -> Result<(), EcosystemError> {
        let active_handlers = self.discover_active(workspace_root);

        if active_handlers.is_empty() {
            info!(workspace = ?workspace_root, "No active ecosystem stacks detected.");
            println!("[dmz install] No active ecosystem stacks detected.");
            return Ok(());
        }

        for eco in active_handlers {
            info!(
                ecosystem = eco.name(),
                workspace = ?workspace_root,
                "Materializing ecosystem stack"
            );
            println!(
                "[dmz install] Materializing ecosystem stack: '{}'",
                eco.name()
            );
            eco.materialize(workspace_root, offline_store, lockfile_path)?;
        }

        Ok(())
    }
}
