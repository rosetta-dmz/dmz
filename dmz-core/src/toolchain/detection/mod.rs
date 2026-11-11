use super::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub mod android;
pub mod apple;
pub mod helpers;
pub mod juce;
pub mod native;
pub mod node;
pub mod rust;
pub mod scripting;

pub use android::AndroidDetector;
pub use apple::AppleDetector;
pub use helpers::{check_member_native_manifests, expand_workspace_member_dirs};
pub use juce::JuceDetector;
pub use native::{
    has_explicit_ninja_manifest, makefile_has_native_c_recipes, workspace_has_native_c_sources,
    workspace_requires_c_compiler_for_make, NativeBuildDetector,
};
pub use node::NodeDetector;
pub use rust::RustDetector;
pub use scripting::ScriptingDetector;

/// Manifest-driven workspace ecosystem and toolchain detector trait.
pub trait EcosystemDetector: Send + Sync {
    /// Detects ecosystem from workspace/directory files.
    fn detect_in_dir(&self, dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>);
}

/// Manifest-driven workspace ecosystem and toolchain detector facade.
pub struct ToolchainDetector;

impl ToolchainDetector {
    /// Aggregates active toolchains across multiple workspace members (primary + sidecars).
    pub fn detect_multi_workspace_ecosystems(
        members: &[crate::workspace::WorkspaceMember],
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        let mut ecosystems = HashSet::new();
        for member in members {
            let member_ecosystems = Self::detect_workspace_ecosystems(&member.path, lockfile);
            ecosystems.extend(member_ecosystems);
        }
        ecosystems
    }

    /// Detects active workspace ecosystems strictly from declared workspace root manifests.
    pub fn detect_workspace_ecosystems(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        let mut ecosystems = HashSet::new();

        if !workspace_root.exists() || !workspace_root.is_dir() {
            return ecosystems;
        }

        // Detect ecosystems declared at workspace root
        Self::detect_dir_ecosystems(workspace_root, &mut ecosystems);

        // Auto-discover declared multi-workspace members in dmz-workspace.toml
        let manifest_path = workspace_root.join("dmz-workspace.toml");
        if manifest_path.is_file() {
            if let Ok(manifest) =
                crate::workspace::WorkspaceProjectConfig::load_from_file(&manifest_path)
            {
                for member in &manifest.members {
                    let member_dir = if member.path.is_absolute() {
                        member.path.clone()
                    } else {
                        workspace_root.join(&member.path)
                    };
                    if member_dir.is_dir() {
                        Self::detect_dir_ecosystems(&member_dir, &mut ecosystems);
                    }
                }
            }
        }

        // Map resolver lockfile ecosystems
        let map_lockfile =
            |lf: &crate::resolver::lockfile::DmzLockfile,
             ecosystems: &mut HashSet<ToolchainEcosystem>| {
                for pkg in lf.packages.values() {
                    if let Some(eco) = ToolchainEcosystem::from_resolver_ecosystem(&pkg.ecosystem) {
                        ecosystems.insert(eco);
                    }
                }
            };

        if let Some(lf) = lockfile {
            map_lockfile(lf, &mut ecosystems);
        } else {
            let default_lockfile = workspace_root.join("dmz.lock");
            if default_lockfile.exists() {
                if let Ok(lf) =
                    crate::resolver::lockfile::DmzLockfile::load_and_verify(&default_lockfile)
                {
                    map_lockfile(&lf, &mut ecosystems);
                }
            }
        }

        // Minimal companion propagation for native tools
        if ecosystems.contains(&ToolchainEcosystem::CMake) {
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if ecosystems.contains(&ToolchainEcosystem::Ninja) {
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if ecosystems.contains(&ToolchainEcosystem::Make)
            && Self::workspace_requires_c_compiler_for_make(workspace_root)
        {
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if ecosystems.contains(&ToolchainEcosystem::Juce) {
            ecosystems.insert(ToolchainEcosystem::Cc);
            ecosystems.insert(ToolchainEcosystem::CMake);
        }
        if ecosystems.contains(&ToolchainEcosystem::AndroidNdk) {
            ecosystems.insert(ToolchainEcosystem::CMake);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if ecosystems.contains(&ToolchainEcosystem::AppleSdk) {
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        // On macOS: If C/C++ compiler is required, the Apple SDK sysroot is strictly required
        #[cfg(target_os = "macos")]
        if ecosystems.contains(&ToolchainEcosystem::Cc) {
            ecosystems.insert(ToolchainEcosystem::AppleSdk);
        }

        ecosystems
    }

    /// Detects language and native build ecosystems in a specific project directory.
    pub fn detect_dir_ecosystems(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        if !dir.exists() || !dir.is_dir() {
            return;
        }

        RustDetector::detect(dir, ecosystems);
        NodeDetector::detect(dir, ecosystems);
        ScriptingDetector::detect(dir, ecosystems);
        NativeBuildDetector::detect(dir, ecosystems);
        AppleDetector::detect(dir, ecosystems);
        AndroidDetector::detect(dir, ecosystems);
        JuceDetector::detect(dir, ecosystems);
    }

    /// Returns true if the workspace root or any declared member explicitly contains `build.ninja`
    /// or explicitly declares a `ninja` dependency in its manifest tables.
    pub fn has_explicit_ninja_manifest(workspace_root: &Path) -> bool {
        native::has_explicit_ninja_manifest(workspace_root)
    }

    /// Check whether any Makefile in the given directory explicitly contains C/C++ compiler invocations.
    pub fn makefile_has_native_c_recipes(dir: &Path) -> bool {
        native::makefile_has_native_c_recipes(dir)
    }

    /// Check whether the workspace contains native C/C++ source or header files.
    pub fn workspace_has_native_c_sources(dir: &Path) -> bool {
        native::workspace_has_native_c_sources(dir)
    }

    /// Determine whether a workspace with a Makefile actually requires a C/C++ compiler.
    pub fn workspace_requires_c_compiler_for_make(dir: &Path) -> bool {
        native::workspace_requires_c_compiler_for_make(dir)
    }

    /// Backward-compatibility delegate.
    #[deprecated(note = "Use detect_workspace_ecosystems for manifest-driven detection")]
    pub fn scan_dir_for_ecosystems(
        dir: &Path,
        _depth: usize,
        _max_depth: usize,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        let detected = Self::detect_workspace_ecosystems(dir, None);
        ecosystems.extend(detected);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_test_lockfile(
        packages: Vec<(&str, &str, crate::resolver::manifest::Ecosystem)>,
    ) -> crate::resolver::lockfile::DmzLockfile {
        let mut map = std::collections::BTreeMap::new();
        for (name, version, eco) in packages {
            map.insert(
                format!("{}:{}", name, version),
                crate::resolver::lockfile::LockedPackage {
                    name: name.into(),
                    version: version.into(),
                    ecosystem: eco,
                    sha256: "fake".into(),
                    source_url: None,
                    dependencies: vec![],
                    member_id: None,
                },
            );
        }
        crate::resolver::lockfile::DmzLockfile {
            version: 1,
            target_platform: None,
            vcs: None,
            workspaces: std::collections::BTreeMap::new(),
            closure_signature: "sig".into(),
            packages: map,
        }
    }

    #[test]
    fn test_detect_workspace_ecosystems_single_rust() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
    }

    #[test]
    fn test_detect_workspace_ecosystems_polyglot() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("package.json"), "{\"name\": \"demo\"}\n").unwrap();
        fs::write(dir.path().join("go.mod"), "module demo\n").unwrap();
        fs::write(dir.path().join("requirements.txt"), "flask\n").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(detected.contains(&ToolchainEcosystem::Node));
        assert!(detected.contains(&ToolchainEcosystem::Go));
        assert!(detected.contains(&ToolchainEcosystem::Python));
        assert_eq!(detected.len(), 4);
    }

    #[test]
    fn test_detect_workspace_ecosystems_from_lockfile() {
        let dir = tempdir().unwrap();
        let lf = make_test_lockfile(vec![
            ("tokio", "1.0", crate::resolver::manifest::Ecosystem::Cargo),
            ("express", "4.17", crate::resolver::manifest::Ecosystem::Npm),
        ]);

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), Some(&lf));
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(detected.contains(&ToolchainEcosystem::Node));
    }

    #[test]
    fn test_detect_native_build_tools_cmakelists() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.10)\nproject(demo)",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::CMake));
        assert!(detected.contains(&ToolchainEcosystem::Cc));
    }

    #[test]
    fn test_detect_native_build_tools_ninja() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("build.ninja"),
            "rule cc\n  command = gcc -c $in -o $out",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::Ninja));
        assert!(detected.contains(&ToolchainEcosystem::Cc));
    }

    #[test]
    fn test_detect_native_build_tools_makefile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Makefile"), "all:\n\tgcc main.c").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::Make));
        assert!(detected.contains(&ToolchainEcosystem::Cc));
    }

    #[test]
    fn test_detect_task_runner_makefile_does_not_inject_cc() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Makefile"),
            "all:\n\tcargo check --workspace\n\tcargo test --workspace\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::Make));
        assert!(
            !detected.contains(&ToolchainEcosystem::Cc),
            "Task runner Makefile with only cargo recipes must NOT trigger Cc compiler"
        );
    }

    #[test]
    fn test_detect_native_build_tools_juce() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AudioPlugin.jucer"), "<JUCERPROJECT/>").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::Juce));
        assert!(detected.contains(&ToolchainEcosystem::Cc));
    }

    #[test]
    fn test_pure_rust_workspace_does_not_pull_cmake_or_cc_or_apple() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"pure-rust-app\"\n",
        )
        .unwrap();

        // Even if stray vendored folders exist or lockfile mentions libc/cc/swift-bridge, pure Rust workspace only has Rust
        let vendor_dir = dir.path().join(".cargo/vendor/cmake-0.1.50");
        fs::create_dir_all(&vendor_dir).unwrap();
        let vendor_cc = dir.path().join(".cargo/vendor/cc-1.0.83");
        fs::create_dir_all(&vendor_cc).unwrap();

        let lf = make_test_lockfile(vec![
            ("serde", "1.0", crate::resolver::manifest::Ecosystem::Cargo),
            ("libc", "0.2", crate::resolver::manifest::Ecosystem::Cargo),
            ("cc", "1.0.83", crate::resolver::manifest::Ecosystem::Cargo),
            (
                "swift-bridge",
                "0.1.0",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
        ]);

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), Some(&lf));
        assert_eq!(detected.len(), 1, "Only Rust toolchain must be detected");
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(!detected.contains(&ToolchainEcosystem::CMake));
        assert!(!detected.contains(&ToolchainEcosystem::Cc));
        assert!(!detected.contains(&ToolchainEcosystem::AppleSdk));
        assert!(!detected.contains(&ToolchainEcosystem::AndroidNdk));
    }

    #[test]
    fn test_rust_workspace_with_cmake_crate_detects_cmake_and_companions() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"native-tool-app\"\n\n[dependencies]\ncmake = \"0.1.50\"\npkg-config = \"0.3.30\"\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(
            detected.contains(&ToolchainEcosystem::CMake),
            "Rust workspace with cmake build crate must detect CMake"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::Cc),
            "Rust workspace with cmake build crate must detect companion Cc"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::Ninja),
            "Rust workspace with cmake build crate must not falsely pull Ninja"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::PkgConfig),
            "Rust workspace with pkg-config crate must detect PkgConfig"
        );
        #[cfg(target_os = "macos")]
        assert!(
            detected.contains(&ToolchainEcosystem::AppleSdk),
            "On macOS, Cc requires companion AppleSdk sysroot for standard C headers"
        );
        #[cfg(not(target_os = "macos"))]
        assert!(
            !detected.contains(&ToolchainEcosystem::AppleSdk),
            "Must never pull AppleSdk without Apple project manifest on non-macOS"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::AndroidNdk),
            "Must never pull AndroidNdk without Android project manifest"
        );
    }

    #[test]
    fn test_ecosystem_isolation_npm_packages_do_not_leak_native_toolchains() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{\"name\": \"app\"}").unwrap();
        let lf = make_test_lockfile(vec![
            (
                "make-dir",
                "3.1.0",
                crate::resolver::manifest::Ecosystem::Npm,
            ),
            (
                "cc-validator",
                "1.0.0",
                crate::resolver::manifest::Ecosystem::Npm,
            ),
            (
                "android-lib",
                "2.0.0",
                crate::resolver::manifest::Ecosystem::Npm,
            ),
            (
                "apple-pay",
                "1.2.0",
                crate::resolver::manifest::Ecosystem::Npm,
            ),
        ]);

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), Some(&lf));
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&ToolchainEcosystem::Node));
        assert!(
            !detected.contains(&ToolchainEcosystem::Make),
            "NPM packages must not trigger GNU Make"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::Cc),
            "NPM packages must not trigger C/C++ compiler"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::AndroidNdk),
            "NPM packages must not trigger Android NDK"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::AppleSdk),
            "NPM packages must not trigger Apple SDK"
        );
    }

    #[test]
    fn test_ecosystem_isolation_rust_crates_with_ffi_bindings_do_not_leak_apple_or_ndk() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"my-cli\"\n",
        )
        .unwrap();
        let lf = make_test_lockfile(vec![
            (
                "core-foundation",
                "0.9.3",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
            ("objc", "0.2.7", crate::resolver::manifest::Ecosystem::Cargo),
            (
                "android-tzdata",
                "0.1.1",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
            (
                "android_system_properties",
                "0.1.5",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
        ]);

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), Some(&lf));
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(
            !detected.contains(&ToolchainEcosystem::AppleSdk),
            "Rust crates with Apple bindings must not trigger Apple SDK bundling"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::AndroidNdk),
            "Rust crates with Android bindings must not trigger Android NDK bundling"
        );
    }

    #[test]
    fn test_pure_android_java_gradle_does_not_pull_ndk() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("build.gradle"),
            "plugins {\n    id 'com.android.application'\n}\n\nandroid {\n    compileSdkVersion 34\n}\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(
            !detected.contains(&ToolchainEcosystem::AndroidNdk),
            "Pure Java/Kotlin Android app must NOT trigger Android NDK bundling"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::CMake),
            "Pure Java/Kotlin Android app must NOT trigger CMake bundling"
        );
    }

    #[test]
    fn test_nested_apple_manifest_in_subfolder_does_not_leak_applesdk() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"rust-app\"\n",
        )
        .unwrap();

        // Nested test fixture or documentation example
        let nested = dir.path().join("tests/fixtures/demo.xcodeproj");
        fs::create_dir_all(&nested).unwrap();
        let nested_swift = dir.path().join("docs/Package.swift");
        fs::create_dir_all(nested_swift.parent().unwrap()).unwrap();
        fs::write(&nested_swift, "// demo").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert_eq!(detected.len(), 1);
        assert!(detected.contains(&ToolchainEcosystem::Rust));
        assert!(
            !detected.contains(&ToolchainEcosystem::AppleSdk),
            "Nested Apple manifests in tests/docs must NOT trigger Apple SDK bundling"
        );
    }

    #[test]
    fn test_detect_android_ndk_manifest_and_companion_inference() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("build.gradle"),
            "android {\n    ndkVersion '25.1.8937393'\n    externalNativeBuild {\n        cmake {\n            path 'src/main/cpp/CMakeLists.txt'\n        }\n    }\n}\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(
            detected.contains(&ToolchainEcosystem::AndroidNdk),
            "build.gradle with externalNativeBuild must detect AndroidNdk"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::CMake),
            "build.gradle with cmake path must detect CMake"
        );
    }

    #[test]
    fn test_detect_apple_sdk_manifest_and_companion_inference() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("Package.swift"),
            "// swift-tools-version: 5.9\nimport PackageDescription\nlet package = Package(name: \"Demo\")",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(
            detected.contains(&ToolchainEcosystem::AppleSdk),
            "Package.swift at root must detect AppleSdk"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::Cc),
            "Package.swift must detect companion C/C++ compiler"
        );
    }

    #[test]
    fn test_detect_native_pkg_config_manifests() {
        // 1. CMake with PkgConfig
        let dir_cmake = tempdir().unwrap();
        fs::write(
            dir_cmake.path().join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.20)\nfind_package(PkgConfig REQUIRED)\n",
        )
        .unwrap();
        let detected_cmake =
            ToolchainDetector::detect_workspace_ecosystems(dir_cmake.path(), None);
        assert!(detected_cmake.contains(&ToolchainEcosystem::CMake));
        assert!(detected_cmake.contains(&ToolchainEcosystem::Cc));
        assert!(detected_cmake.contains(&ToolchainEcosystem::PkgConfig));

        // 2. Makefile with pkg-config recipe
        let dir_make = tempdir().unwrap();
        fs::write(
            dir_make.path().join("Makefile"),
            "all:\n\tgcc $(shell pkg-config --cflags openssl) main.c\n",
        )
        .unwrap();
        let detected_make = ToolchainDetector::detect_workspace_ecosystems(dir_make.path(), None);
        assert!(detected_make.contains(&ToolchainEcosystem::Make));
        assert!(detected_make.contains(&ToolchainEcosystem::Cc));
        assert!(detected_make.contains(&ToolchainEcosystem::PkgConfig));

        // 3. Meson project
        let dir_meson = tempdir().unwrap();
        fs::write(
            dir_meson.path().join("meson.build"),
            "project('demo', 'c')\n",
        )
        .unwrap();
        let detected_meson =
            ToolchainDetector::detect_workspace_ecosystems(dir_meson.path(), None);
        assert!(detected_meson.contains(&ToolchainEcosystem::Ninja));
        assert!(detected_meson.contains(&ToolchainEcosystem::Cc));
        assert!(detected_meson.contains(&ToolchainEcosystem::PkgConfig));

        // 4. Standalone .pc files directory
        let dir_pc = tempdir().unwrap();
        let pc_sub = dir_pc.path().join("pkgconfig");
        fs::create_dir_all(&pc_sub).unwrap();
        fs::write(pc_sub.join("libdemo.pc"), "Name: libdemo\n").unwrap();
        let detected_pc = ToolchainDetector::detect_workspace_ecosystems(dir_pc.path(), None);
        assert!(detected_pc.contains(&ToolchainEcosystem::PkgConfig));
    }
}

