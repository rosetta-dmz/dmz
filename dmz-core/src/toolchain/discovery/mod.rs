use super::types::{DiscoveredToolchain, ToolchainEcosystem};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

pub mod android;
pub mod apple;
pub mod cc;
pub mod cmake;
pub mod go;
pub mod juce;
pub mod make;
pub mod ninja;
pub mod node;
pub mod php;
pub mod pkgconfig;
pub mod python;
pub mod ruby;
pub mod rust;

pub use android::AndroidNdkDiscoverer;
pub use apple::AppleSdkDiscoverer;
pub use cc::CcDiscoverer;
pub use cmake::CMakeDiscoverer;
pub use go::GoDiscoverer;
pub use juce::JuceDiscoverer;
pub use make::MakeDiscoverer;
pub use ninja::NinjaDiscoverer;
pub use node::NodeDiscoverer;
pub use php::PhpDiscoverer;
pub use pkgconfig::PkgConfigDiscoverer;
pub use python::PythonDiscoverer;
pub use ruby::RubyDiscoverer;
pub use rust::RustDiscoverer;

/// Core abstraction for discovering a standalone host toolchain for a specific language ecosystem.
///
/// Adhering to the Open/Closed Principle (OCP) and Dependency Inversion Principle (DIP),
/// new language runtimes and toolchain detection strategies implement this trait
/// without modifying closed provider orchestration logic.
pub trait ToolchainDiscoverer: Send + Sync {
    /// Returns the target ecosystem managed by this discoverer.
    fn ecosystem(&self) -> ToolchainEcosystem;

    /// Discovers the host toolchain, returning resolved binaries, shared libraries, and metadata.
    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String>;
}

/// Registry coordinating one or more `ToolchainDiscoverer` implementations.
pub struct DiscoveryRegistry {
    discoverers: Vec<Box<dyn ToolchainDiscoverer>>,
}

impl Default for DiscoveryRegistry {
    fn default() -> Self {
        Self::with_default_discoverers()
    }
}

impl DiscoveryRegistry {
    /// Constructs a registry with the default standalone discoverers for all supported ecosystems.
    pub fn with_default_discoverers() -> Self {
        Self {
            discoverers: vec![
                Box::new(RustDiscoverer),
                Box::new(NodeDiscoverer),
                Box::new(PythonDiscoverer),
                Box::new(GoDiscoverer),
                Box::new(PhpDiscoverer),
                Box::new(RubyDiscoverer),
                Box::new(CMakeDiscoverer),
                Box::new(NinjaDiscoverer),
                Box::new(MakeDiscoverer),
                Box::new(JuceDiscoverer),
                Box::new(PkgConfigDiscoverer),
                Box::new(CcDiscoverer),
                Box::new(AndroidNdkDiscoverer),
                Box::new(AppleSdkDiscoverer),
            ],
        }
    }

    /// Creates an empty registry for custom discoverer configurations.
    pub fn empty() -> Self {
        Self {
            discoverers: Vec::new(),
        }
    }

    /// Registers a custom or alternate toolchain discoverer (Open/Closed Principle).
    pub fn register(&mut self, discoverer: Box<dyn ToolchainDiscoverer>) {
        self.discoverers.push(discoverer);
    }

    /// Discovers all available standalone toolchains across registered discoverers.
    pub fn discover_all(&self) -> Vec<DiscoveredToolchain> {
        let mut toolchains = Vec::new();
        for discoverer in &self.discoverers {
            if let Ok(Some(tc)) = discoverer.discover() {
                toolchains.push(tc);
            }
        }
        toolchains
    }

    /// Discovers standalone host toolchains scoped strictly to the specified active ecosystems.
    pub fn discover_for_ecosystems(
        &self,
        ecosystems: &HashSet<ToolchainEcosystem>,
    ) -> Vec<DiscoveredToolchain> {
        let mut toolchains = Vec::new();
        for discoverer in &self.discoverers {
            if ecosystems.contains(&discoverer.ecosystem()) {
                if let Ok(Some(tc)) = discoverer.discover() {
                    toolchains.push(tc);
                }
            }
        }
        toolchains
    }
}

/// Detects whether a path is non-standalone / non-relocatable (e.g. managed by system package managers
/// that dynamically link against machine-specific cellar/system libraries).
pub fn is_non_standalone_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/opt/homebrew/")
        || s.contains("/usr/local/Cellar/")
        || s.contains("/usr/local/opt/")
        || s.contains("/var/lib/snapd/")
        || s.contains("/nix/store/")
}

/// Emits standard agnostic guidance when a non-relocatable toolchain is encountered.
pub fn emit_non_relocatable_notice(path: &Path) {
    warn!(
        "Non-relocatable toolchain detected at {:?}. Please use the official standalone toolchain from the provider.",
        path
    );
}

/// Appends a path to the candidates list if not already present.
pub fn push_unique_path(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.contains(&path) {
        candidates.push(path);
    }
}

/// Recursively collects all files within a directory relative to a base directory.
pub fn collect_files_recursive(
    dir: &Path,
    base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    target_prefix: &str,
) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_files_recursive(&p, base, acc, target_prefix);
            } else if p.is_file() {
                if let Ok(rel) = p.strip_prefix(base) {
                    let rel_str = rel.to_string_lossy();
                    let target_path = if target_prefix.is_empty() {
                        rel_str.to_string()
                    } else if target_prefix.ends_with('/') {
                        format!("{}{}", target_prefix, rel_str)
                    } else {
                        format!("{}/{}", target_prefix, rel_str)
                    };
                    acc.push((p.clone(), target_path));
                }
            }
        }
    }
}
