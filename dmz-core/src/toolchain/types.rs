use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Supported language toolchain ecosystems and native build tools in DMZ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolchainEcosystem {
    // Primary language runtimes
    Rust,
    Node,
    Python,
    Go,
    Php,
    Ruby,
    // Auxiliary native build systems & frameworks
    CMake,
    Ninja,
    Make,
    Juce,
    PkgConfig,
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
            Self::CMake => "cmake",
            Self::Ninja => "ninja",
            Self::Make => "make",
            Self::Juce => "juce",
            Self::PkgConfig => "pkgconfig",
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
            Self::CMake,
            Self::Ninja,
            Self::Make,
            Self::Juce,
            Self::PkgConfig,
        ]
    }

    /// Returns true if this ecosystem represents a primary language compiler/runtime.
    pub fn is_primary_runtime(&self) -> bool {
        matches!(
            self,
            Self::Rust | Self::Node | Self::Python | Self::Go | Self::Php | Self::Ruby
        )
    }

    /// Returns true if this ecosystem represents an auxiliary native build tool or framework.
    pub fn is_auxiliary_build_tool(&self) -> bool {
        matches!(
            self,
            Self::CMake | Self::Ninja | Self::Make | Self::Juce | Self::PkgConfig
        )
    }

    /// Parses an ecosystem identifier or stack name into a ToolchainEcosystem.
    pub fn from_str_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "rust" | "cargo" => Some(Self::Rust),
            "node" | "nodejs" | "npm" | "pnpm" | "yarn" | "javascript" | "typescript" => {
                Some(Self::Node)
            }
            "python" | "python3" | "pypi" | "pip" => Some(Self::Python),
            "go" | "golang" => Some(Self::Go),
            "php" | "composer" => Some(Self::Php),
            "ruby" | "bundler" | "gem" => Some(Self::Ruby),
            "cmake" => Some(Self::CMake),
            "ninja" => Some(Self::Ninja),
            "make" | "gmake" => Some(Self::Make),
            "juce" | "projucer" => Some(Self::Juce),
            "pkgconfig" | "pkg-config" | "pkgconf" => Some(Self::PkgConfig),
            _ => None,
        }
    }

    /// Maps a package dependency resolver ecosystem to its corresponding compiler/runtime toolchain ecosystem.
    pub fn from_resolver_ecosystem(eco: &crate::resolver::manifest::Ecosystem) -> Option<Self> {
        match eco {
            crate::resolver::manifest::Ecosystem::Cargo => Some(Self::Rust),
            crate::resolver::manifest::Ecosystem::Npm => Some(Self::Node),
            crate::resolver::manifest::Ecosystem::PyPI => Some(Self::Python),
            crate::resolver::manifest::Ecosystem::Go => Some(Self::Go),
            crate::resolver::manifest::Ecosystem::Composer => Some(Self::Php),
            crate::resolver::manifest::Ecosystem::Bundler => Some(Self::Ruby),
            _ => None,
        }
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
