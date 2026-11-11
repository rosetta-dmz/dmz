use crate::cli::{DEFAULT_LOCKFILE_PATH, DEFAULT_STORE_PATH};
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_core::resolver::ResolverEngine;
use std::path::PathBuf;
use tracing::{error, info};

/// Initialize a project workspace for DMZ capture
pub fn run_init(workspace: PathBuf, target: Option<String>) -> anyhow::Result<i32> {
    info!("Initializing project workspace at {:?}", workspace);
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    let registry = EcosystemRegistry::new();
    match registry.process_workspace(&ws_canon) {
        Ok(stacks) => {
            info!(active_stacks = ?stacks, "Discovered active ecosystems");
        }
        Err(e) => {
            error!("Ecosystem discovery failed: {}", e);
            return Ok(1);
        }
    }

    let dmz_store = ws_canon.join(DEFAULT_STORE_PATH);
    if let Err(e) = std::fs::create_dir_all(&dmz_store) {
        error!(
            "Failed to initialize store directory {:?}: {}",
            dmz_store, e
        );
        return Ok(1);
    }

    let lockfile_path = ws_canon.join(DEFAULT_LOCKFILE_PATH);
    info!("Generating initial closure lock at {:?}", lockfile_path);
    match ResolverEngine::resolve_workspace_with_target(&ws_canon, target) {
        Ok(lockfile) => {
            if let Err(e) = lockfile.save(&lockfile_path) {
                error!("Failed to save lockfile: {}", e);
                return Ok(1);
            }
            info!("Successfully created closure lock at {:?}", lockfile_path);
        }
        Err(e) => {
            error!("Dependency resolution failed during init: {}", e);
            return Ok(1);
        }
    }
    info!("DMZ initialization complete. Run `dmz bundle` to package this workspace.");
    Ok(0)
}
