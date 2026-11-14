use super::super::types::{DiscoveredToolchain, ToolchainEcosystem};
use super::super::ToolchainError;
use super::ToolchainDiscoverer;
use tracing::info;

/// Represents the Apple Developer SDK & Command Line Tools ecosystem.
///
/// Under DMZ's host-baseline architecture, Apple Command Line Tools (`xcode-select --install`),
/// Xcode (`xcodebuild`, `xcrun`, `swiftc`), and macOS SDKs (`MacOSX.sdk`) are treated as a
/// **macOS OS baseline prerequisite** rather than bundled into air-gap archives:
/// - Apple's SLA prohibits redistributing Xcode / Command Line Tools / SDKs.
/// - Core Apple tools (`xcodebuild`, `xcrun`, `swiftc`, `ld`) depend on unbundled `@rpath` IDE
///   frameworks and `/usr/lib/libxcselect.dylib`.
/// - Readiness is enforced via [`crate::toolchain::preflight::check_apple_developer_tools`].
#[derive(Debug, Default, Clone)]
pub struct AppleSdkDiscoverer;

impl ToolchainDiscoverer for AppleSdkDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::AppleSdk
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        info!(
            ecosystem = self.ecosystem().as_str(),
            "Apple Developer SDK / Command Line Tools are treated as a macOS host baseline prerequisite and are not bundled into DMZ archives."
        );
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::discovery::is_apple_system_toolchain_path;
    use std::path::Path;

    #[test]
    fn test_apple_sdk_discoverer_does_not_bundle_files() {
        let discoverer = AppleSdkDiscoverer;
        assert_eq!(discoverer.ecosystem(), ToolchainEcosystem::AppleSdk);
        let result = discoverer.discover().expect("discover must succeed");
        assert!(
            result.is_none(),
            "AppleSdk must never bundle proprietary SDKs or @rpath Xcode binaries"
        );
    }

    #[test]
    fn test_apple_system_toolchain_path_detection() {
        assert!(is_apple_system_toolchain_path(Path::new(
            "/Library/Developer/CommandLineTools/usr/bin/clang"
        )));
        assert!(is_apple_system_toolchain_path(Path::new(
            "/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild"
        )));
        assert!(is_apple_system_toolchain_path(Path::new(
            "/Applications/Xcode-16.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/ld"
        )));
        assert!(!is_apple_system_toolchain_path(Path::new(
            "/opt/llvm/bin/clang"
        )));
    }
}
