use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Supported language toolchain ecosystems in DMZ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolchainEcosystem {
    Rust,
    Node,
    Python,
    Go,
    Php,
    Ruby,
}

impl ToolchainEcosystem {
    /// Returns the canonical string representation of the ecosystem.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Php => "php",
            Self::Ruby => "ruby",
        }
    }

    /// Returns a slice of all supported ecosystems.
    pub fn all() -> &'static [ToolchainEcosystem] {
        &[
            Self::Rust,
            Self::Node,
            Self::Python,
            Self::Go,
            Self::Php,
            Self::Ruby,
        ]
    }
}

/// Metadata entry recorded in the airgap manifest for a bundled toolchain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainManifestEntry {
    pub ecosystem: ToolchainEcosystem,
    pub version: String,
    pub path_in_archive: String,
    pub binaries: Vec<String>,
}

/// A discovered host toolchain with resolved binaries, runtime libraries, and metadata.
#[derive(Debug, Clone)]
pub struct DiscoveredToolchain {
    pub ecosystem: ToolchainEcosystem,
    pub version: String,
    pub root_dir: PathBuf,
    /// Host path and relative target path within the toolchain archive directory.
    pub bin_files: Vec<(PathBuf, String)>,
    /// Host path and relative target path within the toolchain archive directory.
    pub lib_files: Vec<(PathBuf, String)>,
}
