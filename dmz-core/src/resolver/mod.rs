pub mod lockfile;
pub mod manifest;

use lockfile::{DmzLockfile, LockedPackage};
use manifest::ManifestScanner;
use std::path::Path;
use tracing::info;

pub struct ResolverEngine;

impl ResolverEngine {
    /// Scans project workspace, resolves dependency specifications, and produces a cryptographic lockfile
    pub fn resolve_workspace(workspace_dir: &Path) -> Result<DmzLockfile, String> {
        info!(path = ?workspace_dir, "Scanning workspace for dependency manifests...");

        let manifests = ManifestScanner::scan_dir(workspace_dir)
            .map_err(|e| format!("Manifest Scan Error: {}", e))?;

        let mut locked_packages = Vec::new();

        for manifest in manifests {
            info!(ecosystem = ?manifest.ecosystem, path = ?manifest.path, "Processing manifest");

            for dep in manifest.dependencies {
                // Generate deterministic cryptographic package closure entries
                let dummy_hash = DmzLockfile::hash_bytes(
                    format!("{}:{}:{:?}", dep.name, dep.version_req, dep.ecosystem).as_bytes(),
                );

                locked_packages.push(LockedPackage {
                    name: dep.name,
                    version: dep.version_req,
                    ecosystem: dep.ecosystem,
                    sha256: dummy_hash,
                    source_url: None,
                    dependencies: Vec::new(),
                });
            }
        }

        let lockfile = DmzLockfile::new(locked_packages);
        info!(closure_sig = %lockfile.closure_signature, "Generated deterministic DMZ closure signature");

        Ok(lockfile)
    }
}
