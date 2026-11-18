pub mod detection;
pub mod discovery;
pub mod env;
pub mod packager;
pub mod preflight;
pub mod python_abi;
pub mod setup;
pub mod types;

pub use detection::{ToolchainDetectOptions, ToolchainDetector};
pub use discovery::{
    emit_non_relocatable_notice, is_apple_system_toolchain_path, is_non_standalone_path,
    AndroidNdkDiscoverer, AppleSdkDiscoverer, CMakeDiscoverer, CcDiscoverer, DiscoveryRegistry,
    GoDiscoverer, JuceDiscoverer, MakeDiscoverer, NinjaDiscoverer, NodeDiscoverer, PhpDiscoverer,
    PkgConfigDiscoverer, PythonDiscoverer, RubyDiscoverer, RustDiscoverer, ToolchainDiscoverer,
};
pub use env::ToolchainEnv;
pub use packager::ToolchainArchivePackager;
pub use preflight::{
    check_apple_developer_tools, discover_filesystem_developer_dir,
    ecosystems_use_native_or_apple_tooling, export_apple_prereq_note,
    export_apple_prereq_note_for_ecosystems, export_apple_prereq_note_from_lockfile,
    has_usable_apple_sdk_or_clt, is_uninstalled_macos_clt_shim, resolve_non_stub_executable,
    resolve_real_apple_developer_tool, should_skip_uninstalled_macos_clt_command,
    verify_apple_prereq_for_ecosystems, verify_apple_prereq_for_workspace,
    workspace_requires_apple_clt, ApplePrereqStatus,
};
pub use python_abi::PythonTargetAbi;
pub use setup::ToolchainSetup;
pub use types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainManifestEntry,
    ToolchainProfile,
};

#[derive(Debug, thiserror::Error)]
pub enum ToolchainError {
    #[error("Toolchain IO Error: {0}")]
    Io(String),
    #[error("Toolchain Setup Error: {0}")]
    SetupFailed(String),
    #[error("Toolchain Discovery Error: {0}")]
    DiscoveryFailed(String),
    #[error("Toolchain Packaging Error: {0}")]
    PackagingFailed(String),
    #[error("Host Prerequisite Missing: {0}")]
    HostPrerequisiteMissing(String),
}

impl From<ToolchainError> for String {
    fn from(err: ToolchainError) -> Self {
        err.to_string()
    }
}

use std::collections::{HashMap, HashSet};
use std::path::Path;
use tar::Builder as TarBuilder;

/// Facade provider maintaining backward compatibility for toolchain discovery operations.
pub struct ToolchainProvider;

impl ToolchainProvider {
    /// Detect whether a path is non-standalone / non-relocatable (e.g. managed by system package managers
    /// that dynamically link against machine-specific cellar/system libraries).
    #[inline]
    pub fn is_non_standalone_path(path: &Path) -> bool {
        discovery::is_non_standalone_path(path)
    }

    /// Emit standard agnostic guidance when a non-relocatable toolchain is encountered.
    #[inline]
    pub fn emit_non_relocatable_notice(path: &Path) {
        discovery::emit_non_relocatable_notice(path);
    }

    /// Discover all available standalone host toolchains using the default registry.
    pub fn discover_all() -> Vec<DiscoveredToolchain> {
        DiscoveryRegistry::with_default_discoverers().discover_all()
    }

    /// Discover active workspace ecosystems based on workspace manifests, dependency trees, and closure lockfile.
    #[inline]
    pub fn detect_workspace_ecosystems(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        ToolchainDetector::detect_workspace_ecosystems(workspace_root, lockfile)
    }

    /// Discover active workspace ecosystems with explicit detection options.
    #[inline]
    pub fn detect_workspace_ecosystems_with_options(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        options: ToolchainDetectOptions,
    ) -> HashSet<ToolchainEcosystem> {
        ToolchainDetector::detect_workspace_ecosystems_with_options(
            workspace_root,
            lockfile,
            options,
        )
    }

    /// Discover standalone host toolchains scoped strictly to the target workspace's ecosystems.
    pub fn discover_for_workspace(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> Vec<DiscoveredToolchain> {
        Self::discover_for_workspace_with_options(
            workspace_root,
            lockfile,
            ToolchainDetectOptions::default(),
            ToolchainProfile::Minimal,
            lockfile.and_then(|l| l.target_platform.as_deref()),
        )
    }

    /// Discover standalone host toolchains scoped by detection options, toolchain profile, and target platform.
    pub fn discover_for_workspace_with_options(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        detect_options: ToolchainDetectOptions,
        profile: ToolchainProfile,
        target_platform: Option<&str>,
    ) -> Vec<DiscoveredToolchain> {
        Self::discover_for_workspace_with_project_and_options(
            workspace_root,
            lockfile,
            None,
            detect_options,
            profile,
            target_platform,
        )
    }

    /// Discover standalone host toolchains with an optional in-memory multi-workspace config.
    pub fn discover_for_workspace_with_project_and_options(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        ws_project_opt: Option<&crate::workspace::WorkspaceProjectConfig>,
        detect_options: ToolchainDetectOptions,
        profile: ToolchainProfile,
        target_platform: Option<&str>,
    ) -> Vec<DiscoveredToolchain> {
        Self::discover_for_workspace_with_project_and_options_checked(
            workspace_root,
            lockfile,
            ws_project_opt,
            detect_options,
            profile,
            target_platform,
            true,
        )
        .unwrap_or_default()
    }

    /// Discover standalone host toolchains and fail fast when a strictly required toolchain
    /// (such as standalone Python) is absent and `allow_missing_toolchains` is `false`.
    pub fn discover_for_workspace_with_project_and_options_checked(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        ws_project_opt: Option<&crate::workspace::WorkspaceProjectConfig>,
        detect_options: ToolchainDetectOptions,
        profile: ToolchainProfile,
        target_platform: Option<&str>,
        allow_missing_toolchains: bool,
    ) -> Result<Vec<DiscoveredToolchain>, ToolchainError> {
        let ecosystems = ToolchainDetector::detect_workspace_ecosystems_with_project_and_options(
            workspace_root,
            lockfile,
            ws_project_opt,
            detect_options,
        );
        if ecosystems.is_empty() {
            tracing::info!(
                workspace = ?workspace_root,
                "No supported language ecosystems detected in workspace; skipping toolchain discovery"
            );
            return Ok(Vec::new());
        }

        tracing::info!(
            workspace = ?workspace_root,
            profile = %profile,
            ecosystems = ?ecosystems.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
            "Discovered target workspace ecosystems for toolchain capture"
        );

        let member_roots =
            ToolchainDetector::collect_workspace_member_dirs(workspace_root, ws_project_opt);
        let ctx = ToolchainDiscoveryContext {
            profile,
            target_platform: target_platform.map(str::to_string),
            workspace_root: Some(workspace_root.to_path_buf()),
            member_roots,
            ecosystems: ecosystems.clone(),
        };
        let registry = DiscoveryRegistry::with_default_discoverers();
        let toolchains = registry.discover_for_ecosystems_with_context(&ecosystems, &ctx);

        let mut missing_required: Vec<ToolchainEcosystem> = Vec::new();
        for eco in &ecosystems {
            if eco.is_host_prerequisite() {
                tracing::info!(
                    ecosystem = eco.as_str(),
                    "Treating {} as a host OS baseline prerequisite (verified on target via pre-flight; not bundled in archive).",
                    eco.as_str()
                );
                continue;
            }
            if !toolchains.iter().any(|tc| tc.ecosystem == *eco) {
                let is_strictly_required = match eco {
                    ToolchainEcosystem::Ninja => {
                        ToolchainDetector::has_explicit_ninja_manifest_with_options(
                            workspace_root,
                            detect_options.include_dev,
                        )
                    }
                    _ => true,
                };

                if !is_strictly_required {
                    tracing::info!(
                        ecosystem = eco.as_str(),
                        "Companion {} toolchain was not discovered on host; omitting from bundle.",
                        eco.as_str()
                    );
                } else if *eco == ToolchainEcosystem::Make
                    && preflight::target_is_macos(target_platform)
                    && preflight::host_apple_clt_provides("make")
                {
                    tracing::info!(
                        ecosystem = eco.as_str(),
                        "No standalone Make distribution discovered on host; macOS Apple CLT/Xcode prerequisite provides 'make' on target."
                    );
                } else {
                    tracing::warn!(
                        ecosystem = eco.as_str(),
                        "Workspace requires {} toolchain, but no standalone host distribution was discovered.",
                        eco.as_str()
                    );
                    if eco.is_primary_runtime() {
                        missing_required.push(*eco);
                    }
                }
            }
        }

        if !allow_missing_toolchains && !missing_required.is_empty() {
            missing_required.sort_by_key(|e| e.as_str());
            let missing_list: Vec<&str> = missing_required.iter().map(|e| e.as_str()).collect();
            let remediation = if missing_required.contains(&ToolchainEcosystem::Python) {
                "Install an official standalone Python distribution from python.org or via `uv python install` on the export host, or pass `--allow-missing-toolchains` / `--no-toolchain`."
            } else {
                "Install the required standalone provider toolchain on the export host, or pass `--allow-missing-toolchains` / `--no-toolchain`."
            };
            return Err(ToolchainError::DiscoveryFailed(format!(
                "Missing required standalone toolchain(s) for workspace export: [{}]. {}",
                missing_list.join(", "),
                remediation
            )));
        }

        Ok(toolchains)
    }

    /// Discover the host Rust toolchain, prioritizing official standalone Rustup distributions.
    #[inline]
    pub fn discover_rust() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        RustDiscoverer.discover()
    }

    /// Discover the host Node.js runtime and npm binary.
    #[inline]
    pub fn discover_node() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        NodeDiscoverer.discover()
    }

    /// Discover the host Python interpreter and site-packages.
    #[inline]
    pub fn discover_python() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        PythonDiscoverer.discover()
    }

    /// Discover the host Go compiler and standard library packages.
    #[inline]
    pub fn discover_go() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        GoDiscoverer.discover()
    }

    /// Discover standalone PHP runtime distribution.
    #[inline]
    pub fn discover_php() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        PhpDiscoverer.discover()
    }

    /// Discover standalone Ruby runtime distribution.
    #[inline]
    pub fn discover_ruby() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        RubyDiscoverer.discover()
    }

    /// Discover standalone CMake distribution.
    #[inline]
    pub fn discover_cmake() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        CMakeDiscoverer.discover()
    }

    /// Discover standalone Ninja build tool distribution.
    #[inline]
    pub fn discover_ninja() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        NinjaDiscoverer.discover()
    }

    /// Discover standalone GNU Make / BSD Make distribution.
    #[inline]
    pub fn discover_make() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        MakeDiscoverer.discover()
    }

    /// Discover standalone JUCE framework distribution.
    #[inline]
    pub fn discover_juce() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        JuceDiscoverer.discover()
    }

    /// Discover standalone pkg-config distribution.
    #[inline]
    pub fn discover_pkgconfig() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        PkgConfigDiscoverer.discover()
    }

    /// Discover standalone C/C++ compiler toolchain.
    #[inline]
    pub fn discover_cc() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        CcDiscoverer.discover()
    }

    /// Discover standalone Android NDK distribution.
    #[inline]
    pub fn discover_android_ndk() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        AndroidNdkDiscoverer.discover()
    }

    /// Discover Apple Developer SDK and toolchain distribution.
    #[inline]
    pub fn discover_apple_sdk() -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        AppleSdkDiscoverer.discover()
    }
}

/// Facade packager maintaining backward compatibility for toolchain packaging and setup.
pub struct ToolchainPackager;

impl ToolchainPackager {
    /// Package discovered toolchains scoped to the workspace into the target tar archive under `toolchains/<eco>/`.
    pub fn package_toolchains<W: std::io::Write>(
        workspace_root: &Path,
        tar_builder: &mut TarBuilder<W>,
    ) -> Result<Vec<ToolchainManifestEntry>, ToolchainError> {
        Self::package_toolchains_with_lockfile(workspace_root, None, tar_builder)
    }

    /// Package discovered toolchains scoped to the workspace and closure lockfile into the target tar archive.
    pub fn package_toolchains_with_lockfile<W: std::io::Write>(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        tar_builder: &mut TarBuilder<W>,
    ) -> Result<Vec<ToolchainManifestEntry>, ToolchainError> {
        Self::package_toolchains_with_epoch(workspace_root, lockfile, tar_builder, 315_532_800)
    }

    /// Package discovered toolchains scoped to the workspace, lockfile, and deterministic epoch.
    pub fn package_toolchains_with_epoch<W: std::io::Write>(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        tar_builder: &mut TarBuilder<W>,
        deterministic_epoch: u64,
    ) -> Result<Vec<ToolchainManifestEntry>, ToolchainError> {
        Self::package_toolchains_with_options(
            workspace_root,
            lockfile,
            tar_builder,
            deterministic_epoch,
            ToolchainDetectOptions::default(),
            ToolchainProfile::Minimal,
            lockfile.and_then(|l| l.target_platform.as_deref()),
        )
    }

    /// Package discovered toolchains with explicit detection options, toolchain profile, and target platform.
    pub fn package_toolchains_with_options<W: std::io::Write>(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        tar_builder: &mut TarBuilder<W>,
        deterministic_epoch: u64,
        detect_options: ToolchainDetectOptions,
        profile: ToolchainProfile,
        target_platform: Option<&str>,
    ) -> Result<Vec<ToolchainManifestEntry>, ToolchainError> {
        Self::package_toolchains_with_options_and_project(
            workspace_root,
            lockfile,
            None,
            tar_builder,
            deterministic_epoch,
            detect_options,
            profile,
            target_platform,
        )
    }

    /// Package discovered toolchains using an optional in-memory multi-workspace config.
    #[allow(clippy::too_many_arguments)]
    pub fn package_toolchains_with_options_and_project<W: std::io::Write>(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        ws_project_opt: Option<&crate::workspace::WorkspaceProjectConfig>,
        tar_builder: &mut TarBuilder<W>,
        deterministic_epoch: u64,
        detect_options: ToolchainDetectOptions,
        profile: ToolchainProfile,
        target_platform: Option<&str>,
    ) -> Result<Vec<ToolchainManifestEntry>, ToolchainError> {
        Self::package_toolchains_with_options_and_project_checked(
            workspace_root,
            lockfile,
            ws_project_opt,
            tar_builder,
            deterministic_epoch,
            detect_options,
            profile,
            target_platform,
            true,
        )
    }

    /// Package discovered toolchains with strict missing-toolchain enforcement when
    /// `allow_missing_toolchains` is `false`.
    #[allow(clippy::too_many_arguments)]
    pub fn package_toolchains_with_options_and_project_checked<W: std::io::Write>(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
        ws_project_opt: Option<&crate::workspace::WorkspaceProjectConfig>,
        tar_builder: &mut TarBuilder<W>,
        deterministic_epoch: u64,
        detect_options: ToolchainDetectOptions,
        profile: ToolchainProfile,
        target_platform: Option<&str>,
        allow_missing_toolchains: bool,
    ) -> Result<Vec<ToolchainManifestEntry>, ToolchainError> {
        let toolchains =
            ToolchainProvider::discover_for_workspace_with_project_and_options_checked(
                workspace_root,
                lockfile,
                ws_project_opt,
                detect_options,
                profile,
                target_platform,
                allow_missing_toolchains,
            )?;
        ToolchainArchivePackager::package_with_epoch(tar_builder, &toolchains, deterministic_epoch)
    }

    /// Set up imported toolchain directory: execution permissions, quarantine clearance, and activator scripts.
    #[inline]
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<(), ToolchainError> {
        ToolchainSetup::setup_imported_toolchains(workspace_root)
    }

    /// Inject bundled toolchains into an environment variable map (for sandbox/shell executions).
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

        fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
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

    #[test]
    fn test_strict_toolchain_discovery_fails_on_missing_python_unless_allowed() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("requirements.txt"), "pydantic==2.9.0\n").unwrap();

        let cross_target = if cfg!(target_os = "linux") {
            "macos-arm64"
        } else {
            "linux-x86_64"
        };

        let strict_res = ToolchainProvider::discover_for_workspace_with_project_and_options_checked(
            ws,
            None,
            None,
            ToolchainDetectOptions::default(),
            ToolchainProfile::Minimal,
            Some(cross_target),
            false,
        );
        assert!(
            strict_res.is_err(),
            "Strict discovery must fail when Python cannot be provisioned for target"
        );
        let err_msg = strict_res.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            err_msg.contains("python"),
            "Error should mention python: {}",
            err_msg
        );
        assert!(
            err_msg.contains("--allow-missing-toolchains"),
            "Error should mention --allow-missing-toolchains: {}",
            err_msg
        );

        let lenient_res =
            ToolchainProvider::discover_for_workspace_with_project_and_options_checked(
                ws,
                None,
                None,
                ToolchainDetectOptions::default(),
                ToolchainProfile::Minimal,
                Some(cross_target),
                true,
            );
        assert!(lenient_res.is_ok());
    }
}
