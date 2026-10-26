pub mod detection;
pub mod discovery;
pub mod env;
pub mod packager;
pub mod setup;
pub mod types;

pub use detection::ToolchainDetector;
pub use discovery::{
    emit_non_relocatable_notice, is_non_standalone_path, AndroidNdkDiscoverer, AppleSdkDiscoverer,
    CMakeDiscoverer, CcDiscoverer, DiscoveryRegistry, GoDiscoverer, JuceDiscoverer, MakeDiscoverer,
    NinjaDiscoverer, NodeDiscoverer, PhpDiscoverer, PkgConfigDiscoverer, PythonDiscoverer,
    RubyDiscoverer, RustDiscoverer, ToolchainDiscoverer,
};
pub use env::ToolchainEnv;
pub use packager::ToolchainArchivePackager;
pub use setup::ToolchainSetup;
pub use types::{DiscoveredToolchain, ToolchainEcosystem, ToolchainManifestEntry};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tar::Builder as TarBuilder;

/// Facade provider maintaining backward compatibility for toolchain discovery operations.
pub struct ToolchainProvider;

impl ToolchainProvider {
    /// Detects whether a path is non-standalone / non-relocatable (e.g. managed by system package managers
    /// that dynamically link against machine-specific cellar/system libraries).
    #[inline]
    pub fn is_non_standalone_path(path: &Path) -> bool {
        discovery::is_non_standalone_path(path)
    }

    /// Emits standard agnostic guidance when a non-relocatable toolchain is encountered.
    #[inline]
    pub fn emit_non_relocatable_notice(path: &Path) {
        discovery::emit_non_relocatable_notice(path);
    }

    /// Discovers all available standalone host toolchains using the default registry.
    pub fn discover_all() -> Vec<DiscoveredToolchain> {
        DiscoveryRegistry::with_default_discoverers().discover_all()
    }

    /// Discovers active workspace ecosystems based on workspace manifests, dependency trees, and closure lockfile.
    #[inline]
    pub fn detect_workspace_ecosystems(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        ToolchainDetector::detect_workspace_ecosystems(workspace_root, lockfile)
    }

    /// Discovers standalone host toolchains scoped strictly to the target workspace's ecosystems.
    pub fn discover_for_workspace(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> Vec<DiscoveredToolchain> {
        let ecosystems = Self::detect_workspace_ecosystems(workspace_root, lockfile);
        if ecosystems.is_empty() {
            tracing::info!(
                workspace = ?workspace_root,
                "No supported language ecosystems detected in workspace; skipping toolchain discovery"
            );
            return Vec::new();
        }

        tracing::info!(
            workspace = ?workspace_root,
            ecosystems = ?ecosystems.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
            "Discovered target workspace ecosystems for toolchain capture"
        );

        let registry = DiscoveryRegistry::with_default_discoverers();
        let toolchains = registry.discover_for_ecosystems(&ecosystems);

        for eco in &ecosystems {
            if !toolchains.iter().any(|tc| tc.ecosystem == *eco) {
                tracing::warn!(
                    ecosystem = eco.as_str(),
                    "Workspace requires {} toolchain, but no standalone host distribution was discovered.",
                    eco.as_str()
                );
            }
        }

        toolchains
    }

    /// Discovers the host Rust toolchain, prioritizing official standalone Rustup distributions.
    #[inline]
    pub fn discover_rust() -> Result<Option<DiscoveredToolchain>, String> {
        RustDiscoverer.discover()
    }

    /// Discovers the host Node.js runtime and npm binary.
    #[inline]
    pub fn discover_node() -> Result<Option<DiscoveredToolchain>, String> {
        NodeDiscoverer.discover()
    }

    /// Discovers the host Python interpreter and site-packages.
    #[inline]
    pub fn discover_python() -> Result<Option<DiscoveredToolchain>, String> {
        PythonDiscoverer.discover()
    }

    /// Discovers the host Go compiler and standard library packages.
    #[inline]
    pub fn discover_go() -> Result<Option<DiscoveredToolchain>, String> {
        GoDiscoverer.discover()
    }

    /// Discovers standalone PHP runtime distribution.
    #[inline]
    pub fn discover_php() -> Result<Option<DiscoveredToolchain>, String> {
        PhpDiscoverer.discover()
    }

    /// Discovers standalone Ruby runtime distribution.
    #[inline]
    pub fn discover_ruby() -> Result<Option<DiscoveredToolchain>, String> {
        RubyDiscoverer.discover()
    }

    /// Discovers standalone CMake distribution.
    #[inline]
    pub fn discover_cmake() -> Result<Option<DiscoveredToolchain>, String> {
        CMakeDiscoverer.discover()
    }

    /// Discovers standalone Ninja build tool distribution.
    #[inline]
    pub fn discover_ninja() -> Result<Option<DiscoveredToolchain>, String> {
        NinjaDiscoverer.discover()
    }

    /// Discovers standalone GNU Make / BSD Make distribution.
    #[inline]
    pub fn discover_make() -> Result<Option<DiscoveredToolchain>, String> {
        MakeDiscoverer.discover()
    }

    /// Discovers standalone JUCE framework distribution.
    #[inline]
    pub fn discover_juce() -> Result<Option<DiscoveredToolchain>, String> {
        JuceDiscoverer.discover()
    }

    /// Discovers standalone pkg-config distribution.
    #[inline]
    pub fn discover_pkgconfig() -> Result<Option<DiscoveredToolchain>, String> {
        PkgConfigDiscoverer.discover()
    }

    /// Discovers standalone C/C++ compiler toolchain.
    #[inline]
    pub fn discover_cc() -> Result<Option<DiscoveredToolchain>, String> {
        CcDiscoverer.discover()
    }

    /// Discovers standalone Android NDK distribution.
    #[inline]
    pub fn discover_android_ndk() -> Result<Option<DiscoveredToolchain>, String> {
        AndroidNdkDiscoverer.discover()
    }

    /// Discovers Apple Developer SDK and toolchain distribution.
    #[inline]
    pub fn discover_apple_sdk() -> Result<Option<DiscoveredToolchain>, String> {
        AppleSdkDiscoverer.discover()
    }
}

/// Facade packager maintaining backward compatibility for toolchain packaging and setup.
pub struct ToolchainPackager;

impl ToolchainPackager {
    /// Packages discovered toolchains scoped to the workspace into the target tar archive under `toolchains/<eco>/`.
    pub fn package_toolchains<W: std::io::Write>(
        workspace_root: &Path,
        tar_builder: &mut TarBuilder<W>,
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        Self::package_toolchains_with_lockfile(workspace_root, None, tar_builder)
    }

    /// Packages discovered toolchains scoped to the workspace and closure lockfile into the target tar archive.
    pub fn package_toolchains_with_lockfile<W: std::io::Write>(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        tar_builder: &mut TarBuilder<W>,
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        let toolchains = ToolchainProvider::discover_for_workspace(workspace_root, lockfile);
        ToolchainArchivePackager::package(tar_builder, &toolchains)
    }

    /// Sets up imported toolchain directory: execution permissions, quarantine clearance, and activator scripts.
    #[inline]
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<(), String> {
        ToolchainSetup::setup_imported_toolchains(workspace_root)
    }

    /// Injects bundled toolchains into an environment variable map (for sandbox/shell executions).
    #[inline]
    pub fn inject_toolchain_paths(workspace_root: &Path, env_map: &mut HashMap<String, String>) {
        ToolchainEnv::inject_toolchain_paths(workspace_root, env_map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct MockDiscoverer {
        ecosystem: ToolchainEcosystem,
        mock_toolchain: Option<DiscoveredToolchain>,
    }

    impl ToolchainDiscoverer for MockDiscoverer {
        fn ecosystem(&self) -> ToolchainEcosystem {
            self.ecosystem
        }

        fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
            Ok(self.mock_toolchain.clone())
        }
    }

    #[test]
    fn test_ecosystem_representations() {
        assert_eq!(ToolchainEcosystem::Rust.as_str(), "rust");
        assert_eq!(ToolchainEcosystem::Node.as_str(), "node");
        assert_eq!(ToolchainEcosystem::Python.as_str(), "python");
        assert_eq!(ToolchainEcosystem::Go.as_str(), "go");
        assert_eq!(ToolchainEcosystem::Php.as_str(), "php");
        assert_eq!(ToolchainEcosystem::Ruby.as_str(), "ruby");
        assert_eq!(ToolchainEcosystem::CMake.as_str(), "cmake");
        assert_eq!(ToolchainEcosystem::Ninja.as_str(), "ninja");
        assert_eq!(ToolchainEcosystem::Make.as_str(), "make");
        assert_eq!(ToolchainEcosystem::Juce.as_str(), "juce");
        assert_eq!(ToolchainEcosystem::PkgConfig.as_str(), "pkgconfig");
        assert_eq!(ToolchainEcosystem::Cc.as_str(), "cc");
        assert_eq!(ToolchainEcosystem::AndroidNdk.as_str(), "androidndk");
        assert_eq!(ToolchainEcosystem::AppleSdk.as_str(), "applesdk");
        assert_eq!(ToolchainEcosystem::all().len(), 14);
    }

    #[test]
    fn test_is_non_standalone_path_heuristics() {
        assert!(ToolchainProvider::is_non_standalone_path(Path::new(
            "/opt/homebrew/bin/cmake"
        )));
        assert!(ToolchainProvider::is_non_standalone_path(Path::new(
            "/usr/local/Cellar/cmake/3.28.1/bin/cmake"
        )));
        assert!(ToolchainProvider::is_non_standalone_path(Path::new(
            "/var/lib/snapd/snaps/bin/cmake"
        )));
        assert!(ToolchainProvider::is_non_standalone_path(Path::new(
            "/nix/store/abc-cmake/bin/cmake"
        )));

        assert!(!ToolchainProvider::is_non_standalone_path(Path::new(
            "/Users/dev/.rustup/toolchains/stable-x86_64-apple-darwin/bin/rustc"
        )));
        assert!(!ToolchainProvider::is_non_standalone_path(Path::new(
            "/opt/cmake-3.28.1/bin/cmake"
        )));
        assert!(!ToolchainProvider::is_non_standalone_path(Path::new(
            "/home/user/android-ndk-r26b/build/cmake"
        )));
    }

    #[test]
    fn test_registry_polymorphism_and_open_closed_principle() {
        let mut registry = DiscoveryRegistry::empty();

        let mock_tc = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::CMake,
            version: "cmake 3.99.0-mock".into(),
            root_dir: PathBuf::from("/mock/cmake"),
            bin_files: vec![(
                PathBuf::from("/mock/cmake/bin/cmake"),
                "bin/cmake".to_string(),
            )],
            lib_files: vec![],
        };

        registry.register(Box::new(MockDiscoverer {
            ecosystem: ToolchainEcosystem::CMake,
            mock_toolchain: Some(mock_tc),
        }));

        let discovered = registry.discover_all();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].ecosystem, ToolchainEcosystem::CMake);
        assert_eq!(discovered[0].version, "cmake 3.99.0-mock");
    }

    #[test]
    fn test_discover_for_ecosystems_filtering() {
        let mut registry = DiscoveryRegistry::empty();

        let rust_tc = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "rustc 1.99.0-mock".into(),
            root_dir: PathBuf::from("/mock/rust"),
            bin_files: vec![],
            lib_files: vec![],
        };

        let go_tc = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Go,
            version: "go 1.22-mock".into(),
            root_dir: PathBuf::from("/mock/go"),
            bin_files: vec![],
            lib_files: vec![],
        };

        registry.register(Box::new(MockDiscoverer {
            ecosystem: ToolchainEcosystem::Rust,
            mock_toolchain: Some(rust_tc),
        }));

        registry.register(Box::new(MockDiscoverer {
            ecosystem: ToolchainEcosystem::Go,
            mock_toolchain: Some(go_tc),
        }));

        let filter = vec![ToolchainEcosystem::Rust]
            .into_iter()
            .collect::<HashSet<_>>();

        let discovered = registry.discover_for_ecosystems(&filter);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].ecosystem, ToolchainEcosystem::Rust);
    }
}
