use crate::cli::{
    ExportArgs, DEFAULT_AIRGAP_ARCHIVE_PATH, DEFAULT_CLOSURE_ARCHIVE_PATH, DEFAULT_LOCKFILE_PATH,
    DEFAULT_STORE_PATH,
};
use dmz_core::archive::{AirGapExporter, ExportConfig};
use std::path::{Path, PathBuf};
use tracing::{error, info};

/// Bundle store items and dmz.lock into a portable air-gap archive (.tar.zst)
pub fn run_export(args: ExportArgs) -> anyhow::Result<i32> {
    let with_toolchain = args.toolchain_enabled();
    let ws_canon =
        std::fs::canonicalize(&args.workspace).unwrap_or_else(|_| args.workspace.clone());

    let effective_lockfile =
        super::resolve_workspace_relative_path(&ws_canon, &args.lockfile, DEFAULT_LOCKFILE_PATH);
    let effective_store =
        super::resolve_workspace_relative_path(&ws_canon, &args.store, DEFAULT_STORE_PATH);

    let effective_output = if args.output == Path::new(DEFAULT_AIRGAP_ARCHIVE_PATH)
        || args.output == Path::new(DEFAULT_CLOSURE_ARCHIVE_PATH)
    {
        ws_canon.join(&args.output)
    } else if args.output.is_relative() {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&args.output)
    } else {
        args.output
    };

    let multi_workspace = dmz_core::workspace::WorkspaceProjectConfig::discover_or_from_lockfile(
        &ws_canon,
        &effective_lockfile,
    );

    let config = ExportConfig {
        workspace_path: ws_canon,
        lockfile_path: effective_lockfile,
        store_dir: effective_store,
        output_path: effective_output,
        zstd_level: args.zstd_level,
        mode: args.mode.into(),
        target_platform: args.target,
        with_git_history: args.with_git_history,
        since: args.since,
        with_toolchain,
        multi_workspace,
    };

    match AirGapExporter::export(&config) {
        Ok(manifest) => {
            info!(
                checksum = %manifest.archive_sha256,
                packages = manifest.package_count,
                target_platform = ?manifest.target_platform,
                "Air-gap export completed successfully"
            );
            Ok(0)
        }
        Err(e) => {
            error!("Air-gap export failed: {}", e);
            Ok(1)
        }
    }
}
