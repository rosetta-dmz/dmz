pub mod discovery;
pub mod env;
pub mod packager;
pub mod setup;
pub mod types;

pub use discovery::{
    emit_non_relocatable_notice, is_non_standalone_path, DiscoveryRegistry, GoDiscoverer,
    NodeDiscoverer, PhpDiscoverer, PythonDiscoverer, RubyDiscoverer, RustDiscoverer,
    ToolchainDiscoverer,
};
pub use env::ToolchainEnv;
pub use packager::ToolchainArchivePackager;
pub use setup::ToolchainSetup;
pub use types::{DiscoveredToolchain, ToolchainEcosystem, ToolchainManifestEntry};

use std::collections::HashMap;
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

    /// Discovers the host Rust toolchain, prioritizing official standalone Rustup distributions.
    #[inline]
    pub fn discover_rust() -> Result<Option<DiscoveredToolchain>, String> {
        RustDiscoverer.discover()
    }

    /// Discovers host Node.js runtime and companion package managers (npm, pnpm, yarn).
    #[inline]
    pub fn discover_node() -> Result<Option<DiscoveredToolchain>, String> {
        NodeDiscoverer.discover()
    }

    /// Discovers host Python runtime (python3, pip), prioritizing standalone environments.
    #[inline]
    pub fn discover_python() -> Result<Option<DiscoveredToolchain>, String> {
        PythonDiscoverer.discover()
    }

    /// Discovers host Go toolchain ($GOROOT, go), prioritizing official standalone GOROOT.
    #[inline]
    pub fn discover_go() -> Result<Option<DiscoveredToolchain>, String> {
        GoDiscoverer.discover()
    }

    /// Discovers host PHP runtime and Composer package manager.
    #[inline]
    pub fn discover_php() -> Result<Option<DiscoveredToolchain>, String> {
        PhpDiscoverer.discover()
    }

    /// Discovers host Ruby runtime and Bundler package manager.
    #[inline]
    pub fn discover_ruby() -> Result<Option<DiscoveredToolchain>, String> {
        RubyDiscoverer.discover()
    }
}

/// Facade packager maintaining backward compatibility for toolchain packaging and setup.
pub struct ToolchainPackager;

impl ToolchainPackager {
    /// Packages all discovered toolchains into the target tar archive under `toolchains/<eco>/`.
    pub fn package_toolchains<W: std::io::Write>(
        _workspace_root: &Path,
        tar_builder: &mut TarBuilder<W>,
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        let toolchains = ToolchainProvider::discover_all();
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
    fn test_registry_polymorphism_and_open_closed_principle() {
        let mut registry = DiscoveryRegistry::empty();

        let mock_tc = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "rustc 1.99.0-mock".into(),
            root_dir: PathBuf::from("/mock/sysroot"),
            bin_files: vec![(PathBuf::from("/mock/bin/rustc"), "bin/rustc".into())],
            lib_files: vec![],
        };

        registry.register(Box::new(MockDiscoverer {
            ecosystem: ToolchainEcosystem::Rust,
            mock_toolchain: Some(mock_tc),
        }));

        let discovered = registry.discover_all();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].ecosystem, ToolchainEcosystem::Rust);
        assert_eq!(discovered[0].version, "rustc 1.99.0-mock");
    }

    #[test]
    fn test_is_non_standalone_path_heuristics() {
        assert!(is_non_standalone_path(Path::new(
            "/opt/homebrew/Cellar/rust/1.96.0/bin/cargo"
        )));
        assert!(is_non_standalone_path(Path::new(
            "/usr/local/Cellar/node/20.0/bin/node"
        )));
        assert!(is_non_standalone_path(Path::new(
            "/nix/store/abc-rust/bin/rustc"
        )));

        assert!(!is_non_standalone_path(Path::new(
            "/Users/user/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo"
        )));
        assert!(!is_non_standalone_path(Path::new(
            "/Users/user/.nvm/versions/node/v20.0.0/bin/node"
        )));
    }

    #[test]
    fn test_ecosystem_representations() {
        assert_eq!(ToolchainEcosystem::Rust.as_str(), "rust");
        assert_eq!(ToolchainEcosystem::Node.as_str(), "node");
        assert_eq!(ToolchainEcosystem::Python.as_str(), "python");
        assert_eq!(ToolchainEcosystem::Go.as_str(), "go");
        assert_eq!(ToolchainEcosystem::Php.as_str(), "php");
        assert_eq!(ToolchainEcosystem::Ruby.as_str(), "ruby");
        assert_eq!(ToolchainEcosystem::all().len(), 6);
    }
}
