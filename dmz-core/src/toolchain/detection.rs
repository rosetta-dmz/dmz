use super::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

/// Manifest-driven workspace ecosystem and toolchain detector.
///
/// Adheres strictly to the First Principles / Root Manifest Dominance pattern:
/// A workspace's toolchain requirements are determined strictly by what the target workspace
/// actually declares and has at its root, never by heuristic crawling of arbitrary nested directories
/// or inspecting transitive package names in dependency lockfiles.
pub struct ToolchainDetector;

impl ToolchainDetector {
    /// Detects active workspace ecosystems strictly from declared workspace root manifests.
    pub fn detect_workspace_ecosystems(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        let mut ecosystems = HashSet::new();

        if !workspace_root.exists() || !workspace_root.is_dir() {
            return ecosystems;
        }

        // 1. Primary language ecosystems declared at workspace root
        if workspace_root.join("Cargo.toml").is_file() {
            ecosystems.insert(ToolchainEcosystem::Rust);
            Self::check_cargo_workspace_members(workspace_root, &mut ecosystems);
        }
        if workspace_root.join("package.json").is_file()
            || workspace_root.join("pnpm-workspace.yaml").is_file()
            || workspace_root.join("pnpm-lock.yaml").is_file()
            || workspace_root.join("yarn.lock").is_file()
            || workspace_root.join("package-lock.json").is_file()
            || workspace_root.join("bun.lockb").is_file()
            || workspace_root.join("bunfig.toml").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Node);
            Self::check_node_workspace_members(workspace_root, &mut ecosystems);
        }
        if workspace_root.join("go.mod").is_file() || workspace_root.join("go.work").is_file() {
            ecosystems.insert(ToolchainEcosystem::Go);
        }
        if workspace_root.join("composer.json").is_file()
            || workspace_root.join("composer.lock").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Php);
        }
        if workspace_root.join("Gemfile").is_file() || workspace_root.join("Gemfile.lock").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Ruby);
        }
        if workspace_root.join("requirements.txt").is_file()
            || workspace_root.join("pyproject.toml").is_file()
            || workspace_root.join("Pipfile").is_file()
            || workspace_root.join("Pipfile.lock").is_file()
            || workspace_root.join("setup.py").is_file()
            || workspace_root.join("setup.cfg").is_file()
            || workspace_root.join("poetry.lock").is_file()
            || workspace_root.join("uv.lock").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Python);
        }

        // 2. Native project manifests declared at workspace root
        if workspace_root.join("CMakeLists.txt").is_file() {
            ecosystems.insert(ToolchainEcosystem::CMake);
            ecosystems.insert(ToolchainEcosystem::Cc);
            ecosystems.insert(ToolchainEcosystem::Ninja);
        }
        if workspace_root.join("build.ninja").is_file() {
            ecosystems.insert(ToolchainEcosystem::Ninja);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if workspace_root.join("Makefile").is_file()
            || workspace_root.join("GNUmakefile").is_file()
            || workspace_root.join("makefile").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Make);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        // 3. Platform SDK project manifests declared at workspace root
        if workspace_root.join("Package.swift").is_file()
            || workspace_root.join("Podfile").is_file()
            || workspace_root.join("Cartfile").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::AppleSdk);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        // Android build manifests (declared at root or standard app module)
        if workspace_root.join("Android.mk").is_file()
            || workspace_root.join("Application.mk").is_file()
            || workspace_root.join("app/src/main/jni/Android.mk").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::AndroidNdk);
            ecosystems.insert(ToolchainEcosystem::CMake);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        for gradle_name in &[
            "build.gradle",
            "build.gradle.kts",
            "app/build.gradle",
            "app/build.gradle.kts",
        ] {
            let p = workspace_root.join(gradle_name);
            if p.is_file() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.contains("ndkVersion")
                        || content.contains("ndk {")
                        || content.contains("externalNativeBuild")
                    {
                        ecosystems.insert(ToolchainEcosystem::AndroidNdk);
                        ecosystems.insert(ToolchainEcosystem::CMake);
                        ecosystems.insert(ToolchainEcosystem::Cc);
                    }
                }
            }
        }

        // Check root directory entries for root project bundles (*.xcodeproj, *.xcworkspace, *.jucer)
        if let Ok(entries) = std::fs::read_dir(workspace_root) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.ends_with(".xcodeproj") || name_str.ends_with(".xcworkspace") {
                    ecosystems.insert(ToolchainEcosystem::AppleSdk);
                    ecosystems.insert(ToolchainEcosystem::Cc);
                } else if name_str.ends_with(".jucer") {
                    ecosystems.insert(ToolchainEcosystem::Juce);
                    ecosystems.insert(ToolchainEcosystem::Cc);
                    ecosystems.insert(ToolchainEcosystem::CMake);
                }
            }
        }

        // 4. Map resolver lockfile ecosystems (strictly mapping package ecosystem to its language compiler)
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

        // 5. Minimal companion propagation for native tools
        if ecosystems.contains(&ToolchainEcosystem::CMake) {
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if ecosystems.contains(&ToolchainEcosystem::Make)
            || ecosystems.contains(&ToolchainEcosystem::Ninja)
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

        ecosystems
    }

    /// Checks declared Cargo workspace members for explicit native build manifests.
    fn check_cargo_workspace_members(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        let cargo_toml = workspace_root.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(val) = content.parse::<toml::Value>() {
                if let Some(workspace) = val.get("workspace") {
                    if let Some(members) = workspace.get("members").and_then(|m| m.as_array()) {
                        for m in members {
                            if let Some(member_str) = m.as_str() {
                                let member_dir = workspace_root.join(member_str);
                                if member_dir.join("CMakeLists.txt").is_file() {
                                    ecosystems.insert(ToolchainEcosystem::CMake);
                                    ecosystems.insert(ToolchainEcosystem::Cc);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Checks declared Node/pnpm/yarn workspace packages for explicit native build manifests.
    fn check_node_workspace_members(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        let pkg_json = workspace_root.join("package.json");
        if let Ok(content) = std::fs::read_to_string(&pkg_json) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                let pkgs_opt = val.get("workspaces").and_then(|w| {
                    w.as_array()
                        .cloned()
                        .or_else(|| w.get("packages").and_then(|p| p.as_array()).cloned())
                });

                if let Some(pkgs) = pkgs_opt {
                    for pkg in pkgs {
                        if let Some(pattern) = pkg.as_str() {
                            let clean = pattern.trim_end_matches("/*").trim_end_matches('*');
                            let candidate_dir = workspace_root.join(clean);
                            if candidate_dir.join("CMakeLists.txt").is_file() {
                                ecosystems.insert(ToolchainEcosystem::CMake);
                                ecosystems.insert(ToolchainEcosystem::Cc);
                            }
                        }
                    }
                }
            }
        }
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
                },
            );
        }
        crate::resolver::lockfile::DmzLockfile {
            version: 1,
            target_platform: None,
            vcs: None,
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

        // Even if vendored crates exist or Cargo.lock mentions cmake/cc/swift, pure Rust workspace only has Rust
        let vendor_dir = dir.path().join(".cargo/vendor/cmake-0.1.50");
        fs::create_dir_all(&vendor_dir).unwrap();
        let vendor_cc = dir.path().join(".cargo/vendor/cc-1.0.83");
        fs::create_dir_all(&vendor_cc).unwrap();

        let lf = make_test_lockfile(vec![
            ("serde", "1.0", crate::resolver::manifest::Ecosystem::Cargo),
            ("libc", "0.2", crate::resolver::manifest::Ecosystem::Cargo),
            (
                "cmake",
                "0.1.50",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
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
}
