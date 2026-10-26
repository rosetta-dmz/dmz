use super::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

/// Filesystem and dependency tree detector for active workspace ecosystems.
pub struct ToolchainDetector;

impl ToolchainDetector {
    /// Detects active workspace ecosystems based on workspace manifests, dependency trees, and closure lockfile.
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
            if s == "cmake" || s == "cmake-js" || s.starts_with("cmake-") {
                ecosystems.insert(ToolchainEcosystem::CMake);
            } else if s == "ninja" || s == "ninja-build" {
                ecosystems.insert(ToolchainEcosystem::Ninja);
            } else if s == "make" || s == "gmake" || s == "make-pkg" {
                ecosystems.insert(ToolchainEcosystem::Make);
            } else if s == "juce" || s.starts_with("juce-") {
                ecosystems.insert(ToolchainEcosystem::Juce);
            } else if s == "pkg-config" || s == "pkgconf" || s.starts_with("pkg-config-") {
                ecosystems.insert(ToolchainEcosystem::PkgConfig);
            } else if s == "cc" || s == "clang" || s == "gcc" || s == "cc-rs" {
                ecosystems.insert(ToolchainEcosystem::Cc);
            } else if s == "ndk-sys" || s == "cargo-ndk" || s.starts_with("ndk-sys-") {
                ecosystems.insert(ToolchainEcosystem::AndroidNdk);
            } else if s == "swift-rs"
                || s == "swift-bridge"
                || s.starts_with("swift-bridge-")
                || s.starts_with("xcode-")
            {
                ecosystems.insert(ToolchainEcosystem::AppleSdk);
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

        // 3. Inspect Cargo.lock (if present) specifically for native build dependencies
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

/// Recursively scans workspace directories up to `max_depth` to discover active language ecosystems.
fn scan_dir_for_ecosystems(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    ecosystems: &mut HashSet<ToolchainEcosystem>,
) {
    if !dir.is_dir() {
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

    // Android build manifests (NDK only if native C/C++ configuration is present)
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
                    || content.contains("cmake")
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
                    scan_dir_for_ecosystems(&path, depth + 1, max_depth, ecosystems);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_workspace_ecosystems_single_rust() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::CMake));
    }

    #[test]
    fn test_detect_native_build_tools_ninja() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("build.ninja"), "rule cc\n  command = cc").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::Ninja));
    }

    #[test]
    fn test_detect_native_build_tools_makefile() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("Makefile"), "all:\n\techo ok").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::Make));
    }

    #[test]
    fn test_detect_native_build_tools_juce() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(ws.join("AudioPlugin.jucer"), "<JUCERPROJECT/>").unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(detected.contains(&ToolchainEcosystem::Juce));
    }

    #[test]
    fn test_detect_native_build_tools_from_vendor_and_lockfile() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(detected.contains(&ToolchainEcosystem::CMake));
        assert!(detected.contains(&ToolchainEcosystem::PkgConfig));
    }

    #[test]
    fn test_detect_android_ndk_manifest_and_companion_inference() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("build.gradle"),
            "android {\n    ndkVersion \"26.1.10909125\"\n}\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(
            detected.contains(&ToolchainEcosystem::AndroidNdk),
            "Must detect Android NDK from build.gradle with ndkVersion"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::CMake),
            "Must infer companion CMake when Android NDK is required"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::Ninja),
            "Must infer companion Ninja when Android NDK is required"
        );
    }

    #[test]
    fn test_pure_android_java_gradle_does_not_pull_ndk() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("build.gradle"),
            "plugins {\n    id 'com.android.application'\n}\nandroid {\n    compileSdk 34\n}\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(
            !detected.contains(&ToolchainEcosystem::AndroidNdk),
            "Pure Java/Kotlin Android app must not trigger Android NDK"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::CMake),
            "Pure Java/Kotlin Android app must not trigger CMake"
        );
    }

    #[test]
    fn test_detect_apple_sdk_manifest_and_companion_inference() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();
        std::fs::write(
            ws.join("Package.swift"),
            "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"MyPkg\")\n",
        )
        .unwrap();

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, None);
        assert!(
            detected.contains(&ToolchainEcosystem::AppleSdk),
            "Must detect Apple SDK from Package.swift"
        );
        assert!(
            detected.contains(&ToolchainEcosystem::Cc),
            "Must infer companion Cc compiler when Apple SDK is required"
        );
    }

    #[test]
    fn test_detect_android_and_apple_from_lockfile() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "ndk-sys:0.5.0".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "ndk-sys".into(),
                version: "0.5.0".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Cargo,
                sha256: "fake".into(),
                source_url: None,
                dependencies: vec![],
            },
        );
        packages.insert(
            "swift-rs:1.0.0".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "swift-rs".into(),
                version: "1.0.0".into(),
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(detected.contains(&ToolchainEcosystem::AndroidNdk));
        assert!(detected.contains(&ToolchainEcosystem::AppleSdk));
    }

    #[test]
    fn test_rust_with_android_tzdata_does_not_pull_android_ndk() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "android-tzdata:0.1.1".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "android-tzdata".into(),
                version: "0.1.1".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Cargo,
                sha256: "fake".into(),
                source_url: None,
                dependencies: vec![],
            },
        );
        packages.insert(
            "android_system_properties:0.1.5".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "android_system_properties".into(),
                version: "0.1.5".into(),
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(
            !detected.contains(&ToolchainEcosystem::AndroidNdk),
            "android-tzdata and android_system_properties must not trigger Android NDK"
        );
    }

    #[test]
    fn test_node_with_make_dir_does_not_pull_make_or_cc() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "make-dir:3.1.0".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "make-dir".into(),
                version: "3.1.0".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Npm,
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(detected.contains(&ToolchainEcosystem::Node));
        assert!(
            !detected.contains(&ToolchainEcosystem::Make),
            "make-dir npm package must not trigger GNU Make"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::Cc),
            "make-dir npm package must not trigger Cc compiler"
        );
    }

    #[test]
    fn test_node_with_cc_validator_does_not_pull_cc() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "cc-validator:1.0.0".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "cc-validator".into(),
                version: "1.0.0".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Npm,
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(detected.contains(&ToolchainEcosystem::Node));
        assert!(
            !detected.contains(&ToolchainEcosystem::Cc),
            "cc-validator npm package must not trigger Cc compiler"
        );
    }

    #[test]
    fn test_pure_rust_with_core_foundation_does_not_pull_apple_sdk() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path();

        let mut packages = std::collections::BTreeMap::new();
        packages.insert(
            "core-foundation:0.9.3".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "core-foundation".into(),
                version: "0.9.3".into(),
                ecosystem: crate::resolver::manifest::Ecosystem::Cargo,
                sha256: "fake".into(),
                source_url: None,
                dependencies: vec![],
            },
        );
        packages.insert(
            "objc:0.2.7".into(),
            crate::resolver::lockfile::LockedPackage {
                name: "objc".into(),
                version: "0.2.7".into(),
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

        let detected = ToolchainDetector::detect_workspace_ecosystems(ws, Some(&lockfile));
        assert!(
            detected.contains(&ToolchainEcosystem::Rust),
            "Must detect Rust"
        );
        assert!(
            !detected.contains(&ToolchainEcosystem::AppleSdk),
            "Must NOT detect AppleSdk just because core-foundation or objc is present"
        );
    }
}
