use crate::vcs::VcsProvenance;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    #[serde(default)]
    pub workspace_path: PathBuf,
    #[serde(default)]
    pub is_delta: bool,
    #[serde(default)]
    pub has_git_history: bool,
    #[serde(default)]
    pub has_toolchains: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_project: Option<crate::workspace::WorkspaceProjectConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsProvenance>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("Import IO Error: {0}")]
    IoError(String),
    #[error("Cryptographic Verification Failed: {0}")]
    VerificationFailed(String),
    #[error("Archive Extraction Failed: {0}")]
    ExtractionError(String),
}
