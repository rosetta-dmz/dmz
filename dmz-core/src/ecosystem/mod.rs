pub mod handlers;

use std::path::{Path, PathBuf};
use anyhow::Result;

/// Represents a standardized command configuration for delegated resolution.
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// The core trait defining a pluggable workspace ecosystem handler.
pub trait EcosystemHandler: Send + Sync {
    /// Unique identifier for the ecosystem (e.g., "rust", "node", "go", "python")
    fn name(&self) -> &str;

    /// Automatically detects if this ecosystem is present in the workspace root.
    fn detect(&self, workspace_root: &Path) -> bool;

    /// Returns well-known dependency cache or store paths relative to root.
    fn cache_paths(&self, workspace_root: &Path) -> Vec<PathBuf>;

    /// Returns the optional default command to resolve/fetch dependencies offline.
    fn resolution_command(&self) -> Option<CommandSpec>;
}

/// Registry that aggregates and orchestrates all ecosystem handlers.
pub struct EcosystemRegistry {
    handlers: Vec<Box<dyn EcosystemHandler>>,
}

impl EcosystemRegistry {
    pub fn new() -> Self {
        Self {
            handlers: vec![
                Box::new(handlers::RustEcosystem),
                Box::new(handlers::NodeEcosystem),
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
                println!("[dmz ecosystem] Detected '{}' stack. Resolution rule: {} {}", eco.name(), cmd.program, cmd.args.join(" "));
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
}