use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::{
    emit_non_relocatable_notice, is_non_standalone_path, push_unique_path, ToolchainDiscoverer,
};
use std::path::PathBuf;
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
        let lib_files = Vec::new();

        // Locate developer binaries
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
