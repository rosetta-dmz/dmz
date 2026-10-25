pub mod handlers;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Represents a standardized command configuration for delegated resolution and verification.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

pub trait EcosystemHandler: Send + Sync {
    fn name(&self) -> &str;
    fn detect(&self, workspace_root: &Path) -> bool;
    fn cache_paths(&self, workspace_root: &Path) -> Vec<PathBuf>;
    fn resolution_command(&self) -> Option<CommandSpec>;
    fn verification_command(&self, workspace_root: &Path) -> Option<CommandSpec>;

    /// Default implementation: invokes unified InstallerEngine to materialize dependencies
    fn materialize(
        &self,
        workspace_root: &Path,
        offline_store: &Path,
        lockfile_path: &Path,
    ) -> Result<()> {
        let config = crate::install::InstallConfig {
            workspace_path: workspace_root.to_path_buf(),
            lockfile_path: lockfile_path.to_path_buf(),
            store_dir: offline_store.to_path_buf(),
        };
        crate::install::InstallerEngine::install(&config)
            .map_err(|e| anyhow::anyhow!("Offline install error: {}", e))?;
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
            ],
        }
    }

    /// Automatically scans the workspace root and returns all active handlers.
    pub fn discover_active(&self, workspace_root: &Path) -> Vec<&dyn EcosystemHandler> {
        self.handlers
            .iter()
            .filter(|h| h.detect(workspace_root))
            .map(|h| h.as_ref())
            .collect()
    }

    /// Processes the workspace, logging detected stacks and returning their names.
    pub fn process_workspace(&self, workspace_root: &Path) -> Result<Vec<String>> {
        let active = self.discover_active(workspace_root);
        let mut detected_names = Vec::new();

        for eco in active {
            detected_names.push(eco.name().to_string());
            if let Some(cmd) = eco.resolution_command() {
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
    ) -> Result<()> {
        let active_handlers = self.discover_active(workspace_root);

        if active_handlers.is_empty() {
            println!("[dmz install] No active ecosystem stacks detected.");
            return Ok(());
        }

        for eco in active_handlers {
            println!(
                "[dmz install] Materializing ecosystem stack: '{}'",
                eco.name()
            );
            eco.materialize(workspace_root, offline_store, lockfile_path)?;
        }

        Ok(())
    }
}
