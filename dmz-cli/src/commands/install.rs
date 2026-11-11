use crate::cli::{DEFAULT_LOCKFILE_PATH, DEFAULT_STORE_PATH};
use std::path::PathBuf;
use tracing::{error, info};

/// Install locked dependencies offline from store into project layouts
pub fn run_install(workspace: PathBuf, lockfile: PathBuf, store: PathBuf) -> anyhow::Result<i32> {
    info!(
        "Starting DMZ offline installation at workspace {:?}",
        workspace
    );
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

    let effective_lockfile =
        super::resolve_workspace_relative_path(&ws_canon, &lockfile, DEFAULT_LOCKFILE_PATH);
    let effective_store =
        super::resolve_workspace_relative_path(&ws_canon, &store, DEFAULT_STORE_PATH);

    let config = dmz_core::install::InstallConfig {
        workspace_path: ws_canon,
        lockfile_path: effective_lockfile,
        store_dir: effective_store,
    };
    match dmz_core::install::InstallerEngine::install(&config) {
        Ok(count) => {
            info!("Successfully installed {} packages offline", count);
            Ok(0)
        }
        Err(e) => {
            error!("Offline installation failed: {}", e);
            Ok(1)
        }
    }
}
