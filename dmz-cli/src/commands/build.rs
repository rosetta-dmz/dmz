use crate::cli::{TargetMode, DEFAULT_DIST_PATH, DEFAULT_LOCKFILE_PATH};
use dmz_core::packaging::{BuildConfig, PackageBuilder};
use std::path::PathBuf;
use tracing::{error, info};

/// Build deterministic package artifacts (Target A: Deps, Target B: App, Target C: Unified)
pub fn run_build(
    target: TargetMode,
    workspace: PathBuf,
    lockfile: PathBuf,
    output: PathBuf,
    name: String,
    zstd_level: i32,
) -> anyhow::Result<i32> {
    let ws_canon = std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());

    let effective_lockfile =
        super::resolve_workspace_relative_path(&ws_canon, &lockfile, DEFAULT_LOCKFILE_PATH);
    let effective_output =
        super::resolve_workspace_relative_path(&ws_canon, &output, DEFAULT_DIST_PATH);

    let sanitized_name = name.trim();
    if sanitized_name.is_empty() || sanitized_name.contains('/') || sanitized_name.contains('\\') {
        error!("Invalid package artifact name: {:?}", name);
        return Ok(1);
    }

    let config = BuildConfig {
        workspace_dir: ws_canon,
        lockfile_path: effective_lockfile,
        output_dir: effective_output,
        target: target.into(),
        archive_name: sanitized_name.to_string(),
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
            Ok(0)
        }
        Err(e) => {
            error!("Build failed: {}", e);
            Ok(1)
        }
    }
}
