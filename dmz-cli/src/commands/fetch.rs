use dmz_core::resolver::ResolverEngine;
use std::path::PathBuf;
use tracing::{error, info};

/// Fetch and cache locked dependencies into the secure store
pub fn run_fetch(lockfile: PathBuf, store: PathBuf) -> anyhow::Result<i32> {
    info!("Starting dependency fetch from lockfile: {:?}", lockfile);
    match ResolverEngine::fetch_dependencies(&lockfile, &store) {
        Ok(count) => {
            info!(
                "Successfully fetched {} dependencies into store {:?}",
                count, store
            );
            Ok(0)
        }
        Err(e) => {
            error!("Dependency fetch failed: {}", e);
            Ok(1)
        }
    }
}
