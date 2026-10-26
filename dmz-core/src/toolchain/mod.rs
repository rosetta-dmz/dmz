pub mod discovery;
pub mod env;
pub mod packager;
pub mod setup;
pub mod types;

pub use discovery::{
    emit_non_relocatable_notice, is_non_standalone_path, CMakeDiscoverer, DiscoveryRegistry,
    GoDiscoverer, JuceDiscoverer, MakeDiscoverer, NinjaDiscoverer, NodeDiscoverer, PhpDiscoverer,
    PkgConfigDiscoverer, PythonDiscoverer, RubyDiscoverer, RustDiscoverer, ToolchainDiscoverer,
};
pub use env::ToolchainEnv;
pub use packager::ToolchainArchivePackager;
pub use setup::ToolchainSetup;
pub use types::{DiscoveredToolchain, ToolchainEcosystem, ToolchainManifestEntry};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tar::Builder as TarBuilder;

fn scan_dir_for_ecosystems(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    ecosystems: &mut HashSet<ToolchainEcosystem>,
) {
    if depth > max_depth || !dir.exists() || !dir.is_dir() {
        return;
    }

    if dir.join("Cargo.toml").is_file() {
        ecosystems.insert(ToolchainEcosystem::Rust);
    }
    if dir.join("package.json").is_file() {
        ecosystems.insert(ToolchainEcosystem::Node);
        if let Ok(content) = std::fs::read_to_string(dir.join("package.json")) {
            if content.contains("cmake-js") {
                ecosystems.insert(ToolchainEcosystem::CMake);
            }
        }
    }
    if dir.join("go.mod").is_file() {
        ecosystems.insert(ToolchainEcosystem::Go);
    }
    if dir.join("composer.json").is_file() {
        ecosystems.insert(ToolchainEcosystem::Php);
    }
    if dir.join("Gemfile").is_file() {
        ecosystems.insert(ToolchainEcosystem::Ruby);
    }
    if dir.join("requirements.txt").is_file()
        || dir.join("pyproject.toml").is_file()
        || dir.join("Pipfile").is_file()
        || dir.join("setup.py").is_file()
    {
        ecosystems.insert(ToolchainEcosystem::Python);
        if let Ok(content) = std::fs::read_to_string(dir.join("pyproject.toml")) {
            if content.contains("cmake") || content.contains("scikit-build") {
                ecosystems.insert(ToolchainEcosystem::CMake);
            }
            if content.contains("ninja") {
                ecosystems.insert(ToolchainEcosystem::Ninja);
            }
        }
    }

    // Auxiliary native build files
    if dir.join("CMakeLists.txt").is_file() {
        ecosystems.insert(ToolchainEcosystem::CMake);
    }
    if dir.join("build.ninja").is_file() {
        ecosystems.insert(ToolchainEcosystem::Ninja);
    }
    if dir.join("Makefile").is_file()
        || dir.join("GNUmakefile").is_file()
        || dir.join("makefile").is_file()
    {
        ecosystems.insert(ToolchainEcosystem::Make);
    }

    if depth < max_depth {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with('.')
                        || name_str == "target"
                        || name_str == "node_modules"
                        || name_str == "dist"
                        || name_str == "build"
                        || name_str == "venv"
                        || name_str == ".venv"
                        || name_str == "vendor"
                    {
                        continue;
                    }
                    if name_str == "modules" && path.join("juce_core").exists() {
                        ecosystems.insert(ToolchainEcosystem::Juce);
                    }
                    scan_dir_for_ecosystems(&path, depth + 1, max_depth, ecosystems);
                } else if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if ext == "jucer" {
                        ecosystems.insert(ToolchainEcosystem::Juce);
                    } else if ext == "pc" {
                        ecosystems.insert(ToolchainEcosystem::PkgConfig);
                    }
                }
            }
        }
    }
}

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
    pub fn detect_workspace_ecosystems(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        let mut ecosystems = HashSet::new();

        // 1. Filesystem manifest scan (root + nested up to depth 3)
        scan_dir_for_ecosystems(workspace_root, 0, 3, &mut ecosystems);

        // Helper to inspect package and crate names for native build tool requirements
        let inspect_package_name = |name: &str, ecosystems: &mut HashSet<ToolchainEcosystem>| {
            let s = name.to_ascii_lowercase();
            if s == "cmake" || s.starts_with("cmake-") {
                ecosystems.insert(ToolchainEcosystem::CMake);
            } else if s == "ninja" || s.starts_with("ninja-") {
                ecosystems.insert(ToolchainEcosystem::Ninja);
            } else if s == "make" || s.starts_with("make-") {
                ecosystems.insert(ToolchainEcosystem::Make);
            } else if s == "juce" || s.starts_with("juce-") {
                ecosystems.insert(ToolchainEcosystem::Juce);
            } else if s == "pkg-config" || s.starts_with("pkg-config-") || s == "pkgconf" {
                ecosystems.insert(ToolchainEcosystem::PkgConfig);
            }
        };

        // 2. Lockfile inspection
        if let Some(lf) = lockfile {
            for pkg in lf.packages.values() {
                if let Some(eco) = ToolchainEcosystem::from_resolver_ecosystem(&pkg.ecosystem) {
                    ecosystems.insert(eco);
                }
                inspect_package_name(&pkg.name, &mut ecosystems);
            }
        } else {
            let default_lockfile = workspace_root.join("dmz.lock");
            if default_lockfile.exists() {
                if let Ok(lf) =
                    crate::resolver::lockfile::DmzLockfile::load_and_verify(&default_lockfile)
                {
                    for pkg in lf.packages.values() {
                        if let Some(eco) =
                            ToolchainEcosystem::from_resolver_ecosystem(&pkg.ecosystem)
                        {
                            ecosystems.insert(eco);
                        }
                        inspect_package_name(&pkg.name, &mut ecosystems);
                    }
                }
            }
        }

        // 3. Inspect Cargo.lock (if present) for native build dependencies
        let cargo_lock = workspace_root.join("Cargo.lock");
        if cargo_lock.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_lock) {
                if content.contains("name = \"cmake\"") || content.contains("\"cmake\"") {
                    ecosystems.insert(ToolchainEcosystem::CMake);
                }
                if content.contains("name = \"ninja\"") || content.contains("\"ninja\"") {
                    ecosystems.insert(ToolchainEcosystem::Ninja);
                }
                if content.contains("name = \"pkg-config\"") || content.contains("\"pkg-config\"") {
                    ecosystems.insert(ToolchainEcosystem::PkgConfig);
                }
            }
        }

        // 4. Inspect vendored packages in .cargo/vendor/ or vendor/
        for vpath in &[
            workspace_root.join(".cargo/vendor"),
            workspace_root.join("vendor"),
        ] {
            if vpath.is_dir() {
                if let Ok(entries) = std::fs::read_dir(vpath) {
                    for entry in entries.flatten() {
                        let fname = entry.file_name();
                        let s = fname.to_string_lossy();
                        inspect_package_name(&s, &mut ecosystems);
                    }
                }
            }
        }

        ecosystems
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

    /// Discovers standalone CMake distribution.
    #[inline]
    pub fn discover_cmake() -> Result<Option<DiscoveredToolchain>, String> {
        CMakeDiscoverer.discover()
    }

    /// Discovers standalone Ninja build tool.
    #[inline]
    pub fn discover_ninja() -> Result<Option<DiscoveredToolchain>, String> {
        NinjaDiscoverer.discover()
    }

    /// Discovers standalone Make toolchain.
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
        assert_eq!(ToolchainEcosystem::CMake.as_str(), "cmake");
        assert_eq!(ToolchainEcosystem::Ninja.as_str(), "ninja");
        assert_eq!(ToolchainEcosystem::Make.as_str(), "make");
        assert_eq!(ToolchainEcosystem::Juce.as_str(), "juce");
        assert_eq!(ToolchainEcosystem::PkgConfig.as_str(), "pkgconfig");
        assert_eq!(ToolchainEcosystem::all().len(), 11);
    }

    #[test]
    fn test_detect_workspace_ecosystems_single_rust() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"",
        )
        .unwrap();

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
    }

    #[test]
    fn test_detect_workspace_ecosystems_polyglot() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[package]\nname = \"backend\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        let frontend_dir = ws.join("frontend");
        std::fs::create_dir_all(&frontend_dir).unwrap();
        std::fs::write(
            frontend_dir.join("package.json"),
            "{\"name\": \"frontend\"}",
        )
        .unwrap();

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
        assert_eq!(detected.len(), 2);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(detected.contains(&ToolchainEcosystem::Node));
    }

    #[test]
    fn test_detect_workspace_ecosystems_from_lockfile() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "golang.org/x/net:v1.0.0".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "golang.org/x/net".into(),
                version: "v1.0.0".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Go,
                sha256: "fake".into(),
                source_url: None,
                dependencies: vec![],
            },
        );

        let lockfile = crate::resolver::lockfile::DmzLockfile {
            version: 1,
            target_platform: None,
            vcs: None,
            closure_signature: "sig".into(),
            packages,
        };

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(detected.contains(&ToolchainEcosystem::Go));
    }

    #[test]
    fn test_detect_native_build_tools_cmakelists() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nproject(Test)",
        )
        .unwrap();

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::CMake));
    }

    #[test]
    fn test_detect_native_build_tools_ninja() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("build.ninja"), "rule cc\n  command = cc").unwrap();

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::Ninja));
    }

    #[test]
    fn test_detect_native_build_tools_makefile() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("Makefile"), "all:\n\techo ok").unwrap();

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::Make));
    }

    #[test]
    fn test_detect_native_build_tools_juce() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("AudioPlugin.jucer"), "<JUCERPROJECT/>").unwrap();

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::Juce));
    }

    #[test]
    fn test_detect_native_build_tools_from_vendor_and_lockfile() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        // Create vendor/cmake-0.1.50
        let vendor_cmake = ws.join(".cargo/vendor/cmake-0.1.50");
        std::fs::create_dir_all(&vendor_cmake).unwrap();
        std::fs::write(
            vendor_cmake.join("Cargo.toml"),
            "[package]\nname = \"cmake\"",
        )
        .unwrap();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "pkg-config:0.3.30".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "pkg-config".into(),
                version: "0.3.30".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Cargo,
                sha256: "fake".into(),
                source_url: None,
                dependencies: vec![],
            },
        );

        let lockfile = crate::resolver::lockfile::DmzLockfile {
            version: 1,
            target_platform: None,
            vcs: None,
            closure_signature: "sig".into(),
            packages,
        };

        let detected = ToolchainProvider::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(detected.contains(&ToolchainEcosystem::CMake));
        assert!(detected.contains(&ToolchainEcosystem::PkgConfig));
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

        let mut filter = HashSet::new();
        filter.insert(ToolchainEcosystem::Rust);

        let discovered = registry.discover_for_ecosystems(&filter);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].ecosystem, ToolchainEcosystem::Rust);
    }
}
