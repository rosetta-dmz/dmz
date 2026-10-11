use crate::archive::exporter::AirGapManifest;
use crate::resolver::lockfile::DmzLockfile;
use crate::ecosystem::EcosystemRegistry;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use tar::Archive as TarArchive;
use tracing::info;
use zstd::stream::read::Decoder as ZstdDecoder;

#[derive(Debug, Clone)]
pub struct ImportConfig {
    pub archive_path: PathBuf,
    pub target_store_dir: PathBuf,
    pub target_workspace_dir: PathBuf, // Destination for workspace source code & metadata
    pub verify_closure_hash: bool,
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self {
            archive_path: PathBuf::from("airgap_bundle.tar.zst"),
            target_store_dir: PathBuf::from(".dmz/store"),
            target_workspace_dir: PathBuf::from("workspace"), // Sub-folder default 
            verify_closure_hash: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported_packages: usize,
    pub closure_signature: String,
    pub store_path: PathBuf,
}

#[derive(Debug)]
pub enum ImportError {
    IoError(String),
    VerificationFailed(String),
    ExtractionError(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Import IO Error: {}", msg),
            Self::VerificationFailed(msg) => write!(f, "Cryptographic Verification Failed: {}", msg),
            Self::ExtractionError(msg) => write!(f, "Archive Extraction Failed: {}", msg),
        }
    }
}

impl std::error::Error for ImportError {}

pub struct AirGapImporter;

impl AirGapImporter {
    /// Verifies and unpacks an air-gap bundle into the target store and workspace directory
    pub fn import(config: &ImportConfig) -> Result<ImportResult, ImportError> {
        info!(
            archive = ?config.archive_path,
            store = ?config.target_store_dir,
            workspace = ?config.target_workspace_dir,
            "Starting air-gap archive import"
        );

        if !config.archive_path.exists() {
            return Err(ImportError::IoError(format!(
                "Archive file not found: {}",
                config.archive_path.display()
            )));
        }

        // 1. Prepare target directories
        if !config.target_store_dir.exists() {
            fs::create_dir_all(&config.target_store_dir)
                .map_err(|e| ImportError::IoError(format!("Failed to create target store: {}", e)))?;
        }

        if !config.target_workspace_dir.exists() {
            fs::create_dir_all(&config.target_workspace_dir)
                .map_err(|e| ImportError::IoError(format!("Failed to create target workspace: {}", e)))?;
        }

        // 2. Open archive and initialize zstd stream
        let file = File::open(&config.archive_path)
            .map_err(|e| ImportError::IoError(format!("Failed to open archive: {}", e)))?;

        let zstd_decoder = ZstdDecoder::new(file)
            .map_err(|e| ImportError::ExtractionError(format!("Failed to initialize zstd decoder: {}", e)))?;

        let mut archive = TarArchive::new(zstd_decoder);

        let mut closure_signature = String::new();
        let mut imported_count = 0;

        // 3. Single-pass entry iteration, extraction, and verification
        let entries = archive
            .entries()
            .map_err(|e| ImportError::ExtractionError(format!("Failed to read tar entries: {}", e)))?;

        for entry_res in entries {
            let mut entry = entry_res
                .map_err(|e| ImportError::ExtractionError(format!("Tar entry error: {}", e)))?;

            let path = entry
                .path()
                .map_err(|e| ImportError::ExtractionError(e.to_string()))?
                .to_path_buf();

            if path == Path::new("airgap_manifest.json") {
                let mut content = String::new();
                entry
                    .read_to_string(&mut content)
                    .map_err(|e| ImportError::ExtractionError(format!("Failed to read manifest: {}", e)))?;

                let manifest: AirGapManifest = serde_json::from_str(&content)
                    .map_err(|e| ImportError::VerificationFailed(format!("Invalid airgap_manifest.json: {}", e)))?;

                closure_signature = manifest.closure_signature;
                info!(
                    closure = %closure_signature,
                    version = %manifest.dmz_version,
                    "Air-gap manifest verified"
                );

                let dest = config.target_workspace_dir.join(&path);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| ImportError::IoError(e.to_string()))?;
                }
                fs::write(&dest, content).map_err(|e| ImportError::IoError(e.to_string()))?;
            } else if path == Path::new("dmz.lock") {
                let mut content = String::new();
                entry
                    .read_to_string(&mut content)
                    .map_err(|e| ImportError::ExtractionError(format!("Failed to read lockfile: {}", e)))?;

                let lockfile: DmzLockfile = serde_json::from_str(&content)
                    .map_err(|e| ImportError::VerificationFailed(format!("Invalid embedded dmz.lock: {}", e)))?;

                if config.verify_closure_hash {
                    let computed = DmzLockfile::compute_closure_hash(&lockfile.packages);
                    if computed != lockfile.closure_signature {
                        return Err(ImportError::VerificationFailed(format!(
                            "Closure hash mismatch! Lockfile signature: {}, Computed: {}",
                            lockfile.closure_signature, computed
                        )));
                    }
                }

                let dest = config.target_workspace_dir.join(&path);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| ImportError::IoError(e.to_string()))?;
                }
                fs::write(&dest, content).map_err(|e| ImportError::IoError(e.to_string()))?;
            } else if path.starts_with("store") {
                let rel_path = path.strip_prefix("store").unwrap_or(&path);
                if rel_path.as_os_str().is_empty() {
                    continue;
                }

                let dest_path = config.target_store_dir.join(rel_path);

                if let Some(parent) = dest_path.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent)
                            .map_err(|e| ImportError::IoError(format!("Failed to create path {:?}: {}", parent, e)))?;
                    }
                }

                entry
                    .unpack(&dest_path)
                    .map_err(|e| ImportError::ExtractionError(format!("Failed to unpack file {:?}: {}", dest_path, e)))?;

                imported_count += 1;
            } else {
                // Workspace source files (Cargo.toml, src/, etc.)
                entry
                    .unpack_in(&config.target_workspace_dir)
                    .map_err(|e| ImportError::ExtractionError(format!("Failed to unpack workspace file: {}", e)))?;
            }
        }

        // 4. Run post-import ecosystem validation using the registry
        let registry = EcosystemRegistry::new();
        match registry.process_workspace(&config.target_workspace_dir) {
            Ok(stacks) => {
                info!(active_stacks = ?stacks, "Post-import ecosystem discovery complete");
            }
            Err(e) => {
                info!(error = %e, "Ecosystem post-processing check note");
            }
        }

        info!(
            imported_files = imported_count,
            closure = %closure_signature,
            "Air-gap import completed successfully"
        );

        Ok(ImportResult {
            imported_packages: imported_count,
            closure_signature,
            store_path: config.target_store_dir.clone(),
        })
    }
}