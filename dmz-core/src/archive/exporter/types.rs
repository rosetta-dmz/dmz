use crate::vcs::VcsProvenance;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    Unified,  // Workspace source code + dependency store archives
    CodeOnly, // Workspace source code only (no dependencies)
    DepsOnly, // Dependency store archives only (no source code)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaBaseInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_archive_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_closure_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirGapManifest {
    pub dmz_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<VcsProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_base: Option<DeltaBaseInfo>,
    #[serde(default)]
    pub has_git_history: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolchains: Vec<crate::toolchain::ToolchainManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_project: Option<crate::workspace::WorkspaceProjectConfig>,
    pub closure_signature: String,
    pub package_count: usize,
    pub created_at_utc: u64,
    pub archive_sha256: String,
}

#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub workspace_path: PathBuf,
    pub lockfile_path: PathBuf,
    pub store_dir: PathBuf,
    pub output_path: PathBuf,
    pub zstd_level: i32,
    pub mode: ExportMode,
    pub target_platform: Option<String>,
    pub with_git_history: bool,
    pub since: Option<String>,
    pub with_toolchain: bool,
    pub multi_workspace: Option<crate::workspace::WorkspaceProjectConfig>,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            lockfile_path: PathBuf::from("dmz.lock"),
            store_dir: PathBuf::from(".dmz/store"),
            output_path: PathBuf::from("dist/airgap_bundle.tar.zst"),
            zstd_level: 3,
            mode: ExportMode::Unified,
            target_platform: None,
            with_git_history: false,
            since: None,
            with_toolchain: false,
            multi_workspace: None,
        }
    }
}

#[derive(Debug)]
pub enum ExportError {
    IoError(String),
    LockfileError(String),
    ArchiveError(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "Export IO Error: {}", msg),
            Self::LockfileError(msg) => write!(f, "Export Lockfile Error: {}", msg),
            Self::ArchiveError(msg) => write!(f, "Export Archive Error: {}", msg),
        }
    }
}

impl std::error::Error for ExportError {}
