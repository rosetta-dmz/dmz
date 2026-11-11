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
            Self::VerificationFailed(msg) => {
                write!(f, "Cryptographic Verification Failed: {}", msg)
            }
            Self::ExtractionError(msg) => write!(f, "Archive Extraction Failed: {}", msg),
        }
    }
}

impl std::error::Error for ImportError {}
