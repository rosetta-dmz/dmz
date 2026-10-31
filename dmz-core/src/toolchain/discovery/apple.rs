use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    emit_non_relocatable_notice, is_non_standalone_path, push_unique_path, ToolchainDiscoverer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

/// Discovers official Apple Developer toolchains (Xcode Command Line Tools, SDKs, and Swift/Clang compilers).
#[derive(Debug, Default, Clone)]
pub struct AppleSdkDiscoverer;

impl ToolchainDiscoverer for AppleSdkDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::AppleSdk
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1. Explicit DEVELOPER_DIR or SDKROOT environment variables
        for var in &["DEVELOPER_DIR", "SDKROOT"] {
            if let Ok(val) = std::env::var(var) {
                let p = PathBuf::from(val);
                if p.exists() {
                    push_unique_path(&mut candidates, p);
                }
            }
        }

        // 2. Query active Xcode Developer path via xcode-select
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = Command::new("xcode-select").arg("-p").output() {
                if output.status.success() {
                    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let p = PathBuf::from(path_str);
                    if p.exists() {
                        push_unique_path(&mut candidates, p);
                    }
                }
            }

            if let Ok(output) = Command::new("xcrun").arg("--show-sdk-path").output() {
                if output.status.success() {
                    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let p = PathBuf::from(path_str);
                    if p.exists() {
                        push_unique_path(&mut candidates, p);
                    }
                }
            }
        }

        // 3. Standard Apple system paths
        push_unique_path(
            &mut candidates,
            PathBuf::from("/Library/Developer/CommandLineTools"),
        );
        push_unique_path(
            &mut candidates,
            PathBuf::from("/Applications/Xcode.app/Contents/Developer"),
        );

        candidates.retain(|c| c.exists());
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by_key(|c| is_non_standalone_path(c));

        let dev_dir = candidates[0].clone();
        if is_non_standalone_path(&dev_dir) {
            emit_non_relocatable_notice(&dev_dir);
        }

        // Parse version via xcodebuild or clang
        let mut version = "Apple Developer SDK".to_string();
        if let Ok(output) = Command::new("xcodebuild").arg("-version").output() {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !s.is_empty() {
                    version = s;
                }
            }
        } else if let Ok(output) = Command::new("clang").arg("--version").output() {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !s.is_empty() {
                    version = s;
                }
            }
        }

        info!(
            developer_dir = ?dev_dir,
            version = %version,
            "Discovered Apple Developer SDK & Toolchain distribution"
        );

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        // 1. Locate and bundle relocatable Apple SDK sysroot (headers, .tbd stubs, frameworks)
        if let Some(sdk_path) = locate_apple_sdk_root(&dev_dir) {
            info!(
                sdk_path = ?sdk_path,
                "Bundling relocatable Apple SDK sysroot for hermetic C compilation"
            );
            collect_apple_sdk_sysroot(&sdk_path, &mut lib_files);
        }

        // 2. Locate developer binaries
        let search_bin_dirs = vec![
            dev_dir.join("usr/bin"),
            dev_dir.join("Toolchains/XcodeDefault.xctoolchain/usr/bin"),
        ];

        let target_binaries = &[
            "xcrun",
            "xcodebuild",
            "swift",
            "swiftc",
            "lipo",
            "codesign",
            "clang",
            "clang++",
            "ar",
            "ranlib",
        ];

        for bin_name in target_binaries {
            let mut found = false;
            for dir in &search_bin_dirs {
                let p = dir.join(bin_name);
                if p.exists() {
                    bin_files.push((p, format!("bin/{}", bin_name)));
                    found = true;
                    break;
                }
            }
            if !found {
                if let Ok(p) = which::which(bin_name) {
                    bin_files.push((p, format!("bin/{}", bin_name)));
                }
            }
        }

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::AppleSdk,
            version,
            root_dir: dev_dir,
            bin_files,
            lib_files,
        }))
    }
}

/// Locates the active macOS SDK sysroot directory (*.sdk).
fn locate_apple_sdk_root(dev_dir: &Path) -> Option<PathBuf> {
    // 1. Explicit SDKROOT environment variable
    if let Ok(val) = std::env::var("SDKROOT") {
        let p = PathBuf::from(val);
        if p.is_dir() && p.exists() {
            return Some(p);
        }
    }

    // 2. Query xcrun --show-sdk-path
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("xcrun").arg("--show-sdk-path").output() {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let p = PathBuf::from(path_str);
            if p.is_dir() && p.exists() {
                return Some(p);
            }
        }
    }

    // 3. Known direct subpaths within dev_dir
    for sub in &[
        "SDKs/MacOSX.sdk",
        "Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk",
    ] {
        let candidate = dev_dir.join(sub);
        if candidate.is_dir() && candidate.exists() {
            return Some(candidate);
        }
    }

    // 4. Search in SDKs parent directories within dev_dir or standard locations
    let parent_dirs = [
        dev_dir.join("SDKs"),
        dev_dir.join("Platforms/MacOSX.platform/Developer/SDKs"),
        PathBuf::from("/Library/Developer/CommandLineTools/SDKs"),
        PathBuf::from(
            "/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs",
        ),
    ];

    for parent in &parent_dirs {
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() && p.extension().is_some_and(|ext| ext == "sdk") {
                    return Some(p);
                }
            }
        }
    }

    None
}

/// Recursively collects Apple SDK sysroot library stubs, framework headers, and settings.
fn collect_apple_sdk_sysroot(sdk_dir: &Path, lib_files: &mut Vec<(PathBuf, String)>) {
    let target_prefix = "SDKs/MacOSX.sdk";

    // 1. Root SDK descriptor plists
    for fname in &[
        "SDKSettings.plist",
        "SDKSettings.json",
        "SystemVersion.plist",
    ] {
        let p = sdk_dir.join(fname);
        if p.is_file() {
            lib_files.push((p, format!("{}/{}", target_prefix, fname)));
        }
    }

    // 2. Standard C/C++ headers in usr/include
    let usr_inc = sdk_dir.join("usr/include");
    if usr_inc.is_dir() {
        super::collect_files_recursive(&usr_inc, sdk_dir, lib_files, target_prefix);
    }

    // 3. Linker dynamic library stubs and runtime objects in usr/lib
    let usr_lib = sdk_dir.join("usr/lib");
    if usr_lib.is_dir() {
        collect_sdk_lib_recursive(&usr_lib, sdk_dir, lib_files, target_prefix);
    }

    // 4. Framework headers and stubs in System/Library/Frameworks
    let frameworks = sdk_dir.join("System/Library/Frameworks");
    if frameworks.is_dir() {
        let mut visited = std::collections::HashSet::new();
        collect_frameworks_recursive(&frameworks, sdk_dir, lib_files, target_prefix, &mut visited);
    }
}

fn collect_sdk_lib_recursive(
    dir: &Path,
    sdk_base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    target_prefix: &str,
) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_sdk_lib_recursive(&p, sdk_base, acc, target_prefix);
            } else if p.is_file() {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "tbd" || ext == "o" || ext == "dylib" || ext == "a" {
                    if let Ok(rel) = p.strip_prefix(sdk_base) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        let target_path =
                            format!("{}/{}", target_prefix.trim_end_matches('/'), rel_str);
                        acc.push((p, target_path));
                    }
                }
            }
        }
    }
}

fn collect_frameworks_recursive(
    dir: &Path,
    sdk_base: &Path,
    acc: &mut Vec<(PathBuf, String)>,
    target_prefix: &str,
    visited: &mut std::collections::HashSet<PathBuf>,
) {
    if let Ok(canon) = dir.canonicalize() {
        if !visited.insert(canon) {
            return;
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect_frameworks_recursive(&p, sdk_base, acc, target_prefix, visited);
            } else if p.is_file() {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ext == "h"
                    || ext == "hpp"
                    || ext == "tbd"
                    || ext == "plist"
                    || ext == "json"
                    || ext == "modulemap"
                {
                    if let Ok(rel) = p.strip_prefix(sdk_base) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        let target_path =
                            format!("{}/{}", target_prefix.trim_end_matches('/'), rel_str);
                        acc.push((p, target_path));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_collect_apple_sdk_sysroot_structure() {
        let temp = tempdir().unwrap();
        let sdk_root = temp.path().join("MacOSX.sdk");

        // Create standard SDK structure: usr/include, usr/lib, System/Library/Frameworks, SDKSettings.plist
        let usr_inc = sdk_root.join("usr/include");
        let usr_lib = sdk_root.join("usr/lib");
        let frameworks =
            sdk_root.join("System/Library/Frameworks/CoreFoundation.framework/Headers");

        fs::create_dir_all(&usr_inc).unwrap();
        fs::create_dir_all(&usr_lib).unwrap();
        fs::create_dir_all(&frameworks).unwrap();

        fs::write(sdk_root.join("SDKSettings.plist"), "<plist/>").unwrap();
        fs::write(usr_inc.join("stdio.h"), "int printf(const char*, ...);").unwrap();
        fs::write(usr_inc.join("stdlib.h"), "void* malloc(size_t);").unwrap();
        fs::write(usr_lib.join("libSystem.tbd"), "--- !tapi-tbd").unwrap();
        fs::write(frameworks.join("CoreFoundation.h"), "// CoreFoundation").unwrap();

        let mut lib_files = Vec::new();
        collect_apple_sdk_sysroot(&sdk_root, &mut lib_files);

        let target_paths: Vec<String> = lib_files.into_iter().map(|(_, rel)| rel).collect();
        assert!(target_paths.contains(&"SDKs/MacOSX.sdk/SDKSettings.plist".to_string()));
        assert!(target_paths.contains(&"SDKs/MacOSX.sdk/usr/include/stdio.h".to_string()));
        assert!(target_paths.contains(&"SDKs/MacOSX.sdk/usr/include/stdlib.h".to_string()));
        assert!(target_paths.contains(&"SDKs/MacOSX.sdk/usr/lib/libSystem.tbd".to_string()));
        assert!(target_paths.iter().any(|p| p.contains("CoreFoundation.h")));
    }
}
