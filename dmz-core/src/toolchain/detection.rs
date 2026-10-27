use super::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

/// High-precision filesystem scanner and dependency inspector for detecting workspace toolchains.
pub struct ToolchainDetector;

impl ToolchainDetector {
    /// Recursively scans workspace directories up to `max_depth` to discover ecosystem-defining manifests.
    pub fn scan_dir_for_ecosystems(
        dir: &Path,
        depth: usize,
        max_depth: usize,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        if depth > max_depth || !dir.exists() || !dir.is_dir() {
            return;
        }

        // Standard primary language ecosystems
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
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if dir.join("build.ninja").is_file() {
            ecosystems.insert(ToolchainEcosystem::Ninja);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }
        if dir.join("Makefile").is_file()
            || dir.join("GNUmakefile").is_file()
            || dir.join("makefile").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Make);
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        // Android build manifests (surgical: only pull NDK and CMake if native code is declared)
        if dir.join("Android.mk").is_file() || dir.join("Application.mk").is_file() {
            ecosystems.insert(ToolchainEcosystem::AndroidNdk);
            ecosystems.insert(ToolchainEcosystem::CMake);
        }
        for gradle_name in &["build.gradle", "build.gradle.kts"] {
            let p = dir.join(gradle_name);
            if p.is_file() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.contains("ndkVersion")
                        || content.contains("ndk {")
                        || content.contains("externalNativeBuild")
                        || (content.contains("cmake") && content.contains("path"))
                    {
                        ecosystems.insert(ToolchainEcosystem::AndroidNdk);
                        ecosystems.insert(ToolchainEcosystem::CMake);
                    }
                }
            }
        }

        // Apple project manifests
        if dir.join("Package.swift").is_file()
            || dir.join("Podfile").is_file()
            || dir.join("Cartfile").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::AppleSdk);
            ecosystems.insert(ToolchainEcosystem::Cc);
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
                        if name_str.ends_with(".xcodeproj") || name_str.ends_with(".xcworkspace") {
                            ecosystems.insert(ToolchainEcosystem::AppleSdk);
                            ecosystems.insert(ToolchainEcosystem::Cc);
                        }
                        if name_str == "modules" && path.join("juce_core").exists() {
                            ecosystems.insert(ToolchainEcosystem::Juce);
                            ecosystems.insert(ToolchainEcosystem::Cc);
                        }
                        Self::scan_dir_for_ecosystems(&path, depth + 1, max_depth, ecosystems);
                    } else if path.is_file() {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext == "jucer" {
                            ecosystems.insert(ToolchainEcosystem::Juce);
                            ecosystems.insert(ToolchainEcosystem::Cc);
                        } else if ext == "pc" {
                            ecosystems.insert(ToolchainEcosystem::PkgConfig);
                        } else if matches!(ext, "c" | "cpp" | "cc" | "cxx" | "h" | "hpp") {
                            ecosystems.insert(ToolchainEcosystem::Cc);
                        }
                    }
                }
            }
        }
    }

    /// Discovers active workspace ecosystems based on workspace manifests, dependency trees, and closure lockfile.
    ///
    /// Adheres strictly to production-grade domain modeling: dependencies are inspected strictly within
    /// their declared ecosystem boundaries, preventing cross-ecosystem naming collisions.
    pub fn detect_workspace_ecosystems(
        workspace_root: &Path,
        lockfile: Option<&crate::resolver::lockfile::DmzLockfile>,
    ) -> HashSet<ToolchainEcosystem> {
        let mut ecosystems = HashSet::new();

        // 1. Filesystem manifest scan (root + nested up to depth 3)
        Self::scan_dir_for_ecosystems(workspace_root, 0, 3, &mut ecosystems);

        // 2. Lockfile inspection (strongly-typed, scoped strictly per ecosystem)
        let inspect_locked_package =
            |pkg: &crate::resolver::lockfile::LockedPackage,
             ecosystems: &mut HashSet<ToolchainEcosystem>| {
                if let Some(eco) = ToolchainEcosystem::from_resolver_ecosystem(&pkg.ecosystem) {
                    ecosystems.insert(eco);
                }

                match &pkg.ecosystem {
                    crate::resolver::manifest::Ecosystem::Cargo => match pkg.name.as_str() {
                        "cc" | "cc-rs" => {
                            ecosystems.insert(ToolchainEcosystem::Cc);
                        }
                        "cmake" => {
                            ecosystems.insert(ToolchainEcosystem::CMake);
                        }
                        "ninja" => {
                            ecosystems.insert(ToolchainEcosystem::Ninja);
                        }
                        "pkg-config" => {
                            ecosystems.insert(ToolchainEcosystem::PkgConfig);
                        }
                        "ndk-sys" | "cargo-ndk" => {
                            ecosystems.insert(ToolchainEcosystem::AndroidNdk);
                        }
                        "swift-rs" | "swift-bridge" => {
                            ecosystems.insert(ToolchainEcosystem::AppleSdk);
                        }
                        _ => {}
                    },
                    crate::resolver::manifest::Ecosystem::Npm => {
                        if pkg.name == "cmake-js" {
                            ecosystems.insert(ToolchainEcosystem::CMake);
                        }
                    }
                    crate::resolver::manifest::Ecosystem::PyPI => {
                        if pkg.name == "cmake" || pkg.name == "scikit-build" {
                            ecosystems.insert(ToolchainEcosystem::CMake);
                        } else if pkg.name == "ninja" {
                            ecosystems.insert(ToolchainEcosystem::Ninja);
                        }
                    }
                    _ => {}
                }
            };

        if let Some(lf) = lockfile {
            for pkg in lf.packages.values() {
                inspect_locked_package(pkg, &mut ecosystems);
            }
        } else {
            let default_lockfile = workspace_root.join("dmz.lock");
            if default_lockfile.exists() {
                if let Ok(lf) =
                    crate::resolver::lockfile::DmzLockfile::load_and_verify(&default_lockfile)
                {
                    for pkg in lf.packages.values() {
                        inspect_locked_package(pkg, &mut ecosystems);
                    }
                }
            }
        }

        // 3. Inspect Cargo.lock (if present) for Cargo native build dependencies
        let cargo_lock = workspace_root.join("Cargo.lock");
        if cargo_lock.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_lock) {
                if content.contains("name = \"cmake\"") {
                    ecosystems.insert(ToolchainEcosystem::CMake);
                }
                if content.contains("name = \"ninja\"") {
                    ecosystems.insert(ToolchainEcosystem::Ninja);
                }
                if content.contains("name = \"pkg-config\"") {
                    ecosystems.insert(ToolchainEcosystem::PkgConfig);
                }
                if content.contains("name = \"cc\"") {
                    ecosystems.insert(ToolchainEcosystem::Cc);
                }
                if content.contains("name = \"ndk-sys\"")
                    || content.contains("name = \"cargo-ndk\"")
                {
                    ecosystems.insert(ToolchainEcosystem::AndroidNdk);
                }
                if content.contains("name = \"swift-rs\"")
                    || content.contains("name = \"swift-bridge\"")
                {
                    ecosystems.insert(ToolchainEcosystem::AppleSdk);
                }
            }
        }

        // 4. Inspect vendored packages in .cargo/vendor/ (Cargo crates)
        let cargo_vendor = workspace_root.join(".cargo/vendor");
        if cargo_vendor.is_dir() {
            if let Ok(entries) = std::fs::read_dir(cargo_vendor) {
                for entry in entries.flatten() {
                    let fname = entry.file_name();
                    let s = fname.to_string_lossy();
                    let is_versioned_crate = |prefix: &str| {
                        s == prefix
                            || s.strip_prefix(&format!("{}-", prefix)).is_some_and(|rest| {
                                rest.chars().next().is_some_and(|c| c.is_ascii_digit())
                            })
                    };

                    if is_versioned_crate("cc") || is_versioned_crate("cc-rs") {
                        ecosystems.insert(ToolchainEcosystem::Cc);
                    }
                    if is_versioned_crate("cmake") {
                        ecosystems.insert(ToolchainEcosystem::CMake);
                    }
                    if is_versioned_crate("ninja") {
                        ecosystems.insert(ToolchainEcosystem::Ninja);
                    }
                    if is_versioned_crate("pkg-config") {
                        ecosystems.insert(ToolchainEcosystem::PkgConfig);
                    }
                    if is_versioned_crate("ndk-sys") || is_versioned_crate("cargo-ndk") {
                        ecosystems.insert(ToolchainEcosystem::AndroidNdk);
                    }
                    if is_versioned_crate("swift-rs") || is_versioned_crate("swift-bridge") {
                        ecosystems.insert(ToolchainEcosystem::AppleSdk);
                    }
                }
            }
        }

        // 5. Companion toolchain inference & propagation
        if ecosystems.contains(&ToolchainEcosystem::CMake) {
            ecosystems.insert(ToolchainEcosystem::Cc);
            if !ecosystems.contains(&ToolchainEcosystem::Make) {
                ecosystems.insert(ToolchainEcosystem::Ninja);
            }
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
            ecosystems.insert(ToolchainEcosystem::Ninja);
        }
        if ecosystems.contains(&ToolchainEcosystem::AppleSdk) {
            ecosystems.insert(ToolchainEcosystem::Cc);
        }

        ecosystems
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
    fn test_detect_native_build_tools_from_vendor_and_lockfile() {
        let dir = tempdir().unwrap();
        let vendor_dir = dir.path().join(".cargo/vendor/cmake-0.1.50");
        fs::create_dir_all(&vendor_dir).unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), None);
        assert!(
            detected.contains(&ToolchainEcosystem::CMake),
            "Vendored cmake crate must trigger CMake toolchain requirement"
        );
    }

    #[test]
    fn test_ecosystem_isolation_npm_packages_do_not_leak_native_toolchains() {
        let dir = tempdir().unwrap();
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
    fn test_detect_android_and_apple_from_lockfile() {
        let dir = tempdir().unwrap();
        let lf = make_test_lockfile(vec![
            (
                "ndk-sys",
                "0.5.0",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
            (
                "swift-bridge",
                "0.1.0",
                crate::resolver::manifest::Ecosystem::Cargo,
            ),
        ]);

        let detected = ToolchainDetector::detect_workspace_ecosystems(dir.path(), Some(&lf));
        assert!(detected.contains(&ToolchainEcosystem::AndroidNdk));
        assert!(detected.contains(&ToolchainEcosystem::AppleSdk));
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
            "Package.swift must detect AppleSdk"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::Cc),
            "Package.swift must detect companion C/C++ compiler"
        );
    }
}
