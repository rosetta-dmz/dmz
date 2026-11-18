use super::discovery::redact_home_prefix;
use super::types::ToolchainEcosystem;
use super::{ToolchainDetector, ToolchainError};
use crate::resolver::lockfile::DmzLockfile;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Active host readiness state for Apple Command Line Tools / Xcode on macOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplePrereqStatus {
    /// Apple CLT or Xcode is installed, license-accepted, and resolves a valid macOS SDK.
    Ready {
        developer_dir: PathBuf,
        sdk_path: PathBuf,
        sdk_version: Option<String>,
    },
    /// No active Apple Developer Tools installation was found.
    NotInstalled { reason: String },
    /// Xcode / Apple SDK license agreement has not yet been accepted on this host.
    LicenseNotAccepted {
        developer_dir: Option<PathBuf>,
        detail: String,
    },
    /// Apple Developer Tools appear partially present or broken (e.g., invalid DEVELOPER_DIR).
    BrokenOrUnusable { reason: String },
}

impl ApplePrereqStatus {
    /// Returns `true` when Apple CLT / Xcode is usable on the host.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Actionable host command for resolving the missing or unusable prerequisite.
    pub fn remediation_hint(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "",
            Self::LicenseNotAccepted { .. } => {
                "Run 'sudo xcodebuild -license accept' on this macOS host, then retry."
            }
            Self::NotInstalled { .. } => {
                "Install Apple Command Line Tools via 'xcode-select --install' (or install Xcode and run 'sudo xcode-select -s /Applications/Xcode.app/Contents/Developer'), then retry."
            }
            Self::BrokenOrUnusable { .. } => {
                "Reset or install Apple Command Line Tools via 'sudo xcode-select --reset' or 'xcode-select --install', then verify 'xcrun --sdk macosx --show-sdk-path' succeeds."
            }
        }
    }

    /// Formats a user-facing error message explaining the prerequisite gap and remediation.
    pub fn to_error_message(&self) -> String {
        match self {
            Self::Ready { .. } => String::new(),
            Self::NotInstalled { reason } => {
                format!(
                    "macOS Apple Command Line Tools / Xcode prerequisite is missing ({}). {}",
                    reason,
                    self.remediation_hint()
                )
            }
            Self::LicenseNotAccepted {
                developer_dir,
                detail,
            } => {
                let dev = developer_dir
                    .as_deref()
                    .map(redact_home_prefix)
                    .unwrap_or_else(|| "Xcode/CLT".to_string());
                format!(
                    "Apple Xcode license agreement has not been accepted for '{}' ({}). {}",
                    dev,
                    detail,
                    self.remediation_hint()
                )
            }
            Self::BrokenOrUnusable { reason } => {
                format!(
                    "macOS Apple Developer Tools are unusable ({}). {}",
                    reason,
                    self.remediation_hint()
                )
            }
        }
    }
}

/// Returns `true` if `ecosystems` includes any native or Apple tooling that depends on Apple CLT / Xcode
/// when building or running on macOS (`AppleSdk`, `Cc`, `CMake`, `Make`, `Rust`, `Juce`, `AndroidNdk`).
pub fn ecosystems_use_native_or_apple_tooling(ecosystems: &HashSet<ToolchainEcosystem>) -> bool {
    ecosystems.iter().any(|eco| {
        matches!(
            eco,
            ToolchainEcosystem::AppleSdk
                | ToolchainEcosystem::Cc
                | ToolchainEcosystem::CMake
                | ToolchainEcosystem::Make
                | ToolchainEcosystem::Rust
                | ToolchainEcosystem::Juce
                | ToolchainEcosystem::AndroidNdk
        )
    })
}

/// Returns `true` if running on macOS and the workspace ecosystems depend on host Apple CLT / Xcode
/// (SDK sysroot, linker, `clang`, `make`, `xcodebuild`, or `xcrun`).
pub fn workspace_requires_apple_clt(ecosystems: &HashSet<ToolchainEcosystem>) -> bool {
    cfg!(target_os = "macos") && ecosystems_use_native_or_apple_tooling(ecosystems)
}

pub(crate) fn target_is_macos(target_platform: Option<&str>) -> bool {
    match target_platform.map(str::trim).filter(|s| !s.is_empty()) {
        Some(target) => {
            let lower = target.to_ascii_lowercase();
            lower.contains("darwin") || lower.contains("macos") || lower.contains("apple")
        }
        None => cfg!(target_os = "macos"),
    }
}

fn has_real_apple_compiler(dev_dir: &Path) -> bool {
    dev_dir.join("usr/bin/clang").is_file()
        || dev_dir
            .join("Toolchains/XcodeDefault.xctoolchain/usr/bin/clang")
            .is_file()
}

/// Discovers an installed Apple Command Line Tools / Xcode developer directory using pure
/// filesystem checks (`DEVELOPER_DIR`, `/var/db/xcode_select_link`, `/Library/Developer/CommandLineTools`,
/// `/Applications/Xcode*.app/Contents/Developer`) without spawning `/usr/bin/xcrun` or `/usr/bin/xcode-select`.
pub fn discover_filesystem_developer_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }

    let mut candidates = Vec::new();
    if let Some(dev_env) = std::env::var_os("DEVELOPER_DIR").filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(dev_env));
    }
    if let Ok(link_target) = std::fs::read_link("/var/db/xcode_select_link") {
        candidates.push(link_target);
    }
    candidates.push(PathBuf::from("/Library/Developer/CommandLineTools"));
    candidates.push(PathBuf::from("/Applications/Xcode.app/Contents/Developer"));

    if let Ok(apps) = std::fs::read_dir("/Applications") {
        for entry in apps.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("Xcode") && name_str.ends_with(".app") {
                candidates.push(entry.path().join("Contents/Developer"));
            }
        }
    }

    candidates
        .into_iter()
        .find(|cand| cand.is_dir() && cand != Path::new("/") && has_real_apple_compiler(cand))
}

/// Resolves a real Apple Developer binary (`install_name_tool`, `otool`, `clang`, etc.) directly
/// inside a verified developer directory without invoking `/usr/bin/*` GUI prompt stubs.
pub fn resolve_real_apple_developer_tool(tool: &str) -> Option<PathBuf> {
    let dev_dir = discover_filesystem_developer_dir()?;
    for rel in [
        Path::new("usr/bin").join(tool),
        Path::new("Toolchains/XcodeDefault.xctoolchain/usr/bin").join(tool),
    ] {
        let candidate = dev_dir.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Returns `true` when `path` is a Mach-O binary linking Apple's `libxcselect.dylib` CLT shim loader.
pub(crate) fn is_macho_xcselect_stub(path: &Path) -> bool {
    crate::toolchain::setup::macho::inspect_macho_load_info(path).is_some_and(|info| {
        info.dylib_deps
            .iter()
            .any(|dep| dep.contains("libxcselect"))
    })
}

/// Returns `true` when `path` resolves on macOS (without spawning it) to an Apple CLT stub
/// that would trigger the Command Line Developer Tools GUI prompt while no usable CLT/SDK exists.
pub fn is_uninstalled_macos_clt_shim(path: &Path, workspace_root: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }

    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let in_usr_bin = path.parent() == Some(Path::new("/usr/bin"))
        || resolved.parent() == Some(Path::new("/usr/bin"));
    let in_apple_dev_tree =
        crate::toolchain::discovery::is_apple_system_toolchain_path(&resolved) && !in_usr_bin;

    if in_apple_dev_tree {
        return !has_usable_apple_sdk_or_clt(workspace_root);
    }

    // Any `/usr/bin/*` Mach-O linking `libxcselect.dylib` (or `/usr/bin/python*`) ALWAYS invokes
    // Apple's CLT popup when no active CLT/Xcode directory exists on disk, even if `SDKROOT`
    // is configured alongside a standalone compiler. Check developer dir presence BEFORE SDKROOT.
    if is_macho_xcselect_stub(&resolved)
        || (in_usr_bin
            && crate::toolchain::discovery::python::is_unrelocatable_macos_python(&resolved))
    {
        return discover_filesystem_developer_dir().is_none()
            || !check_apple_developer_tools().is_ready();
    }

    if has_usable_apple_sdk_or_clt(workspace_root) {
        return false;
    }

    false
}

/// Resolves `program` against `envs["PATH"]` (or host `PATH`) and returns `Some(stub_path)`
/// if executing it on macOS would invoke an uninstalled Apple CLT shim.
pub fn should_skip_uninstalled_macos_clt_command<S: std::hash::BuildHasher>(
    program: &str,
    envs: &std::collections::HashMap<String, String, S>,
    cwd: &Path,
) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let path_env = envs
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let resolved = crate::workspace::resolve_executable_in_env(program, &path_env, cwd);
    let resolved_path = PathBuf::from(&resolved);
    if is_uninstalled_macos_clt_shim(&resolved_path, cwd) {
        return Some(resolved_path);
    }
    if Path::new(program).file_name().and_then(|n| n.to_str()) == Some("python")
        && !resolved_path.is_file()
        && Path::new("/usr/bin/python").is_file()
        && is_uninstalled_macos_clt_shim(Path::new("/usr/bin/python"), cwd)
    {
        return Some(PathBuf::from("/usr/bin/python"));
    }
    None
}

/// Returns a `PATH` value with `/usr/bin` removed on macOS when Apple CLT/Xcode is absent so
/// wrapper script probes cannot fall back to `/usr/bin/*` Apple CLT GUI shims.
pub(crate) fn sanitized_probe_path_without_macos_clt_stubs() -> Option<std::ffi::OsString> {
    if !cfg!(target_os = "macos") || discover_filesystem_developer_dir().is_some() {
        return None;
    }
    let raw = std::env::var_os("PATH")?;
    let filtered: Vec<PathBuf> = std::env::split_paths(&raw)
        .filter(|dir| *dir != Path::new("/usr/bin"))
        .collect();
    std::env::join_paths(filtered).ok()
}

/// Resolves an executable from `workspace_root`, bundled toolchains, or `path_env`,
/// skipping any macOS `/usr/bin/*` Apple CLT shims when CLT/Xcode is unavailable.
pub fn resolve_non_stub_executable(
    program: &str,
    path_env: Option<&std::ffi::OsStr>,
    workspace_root: &Path,
) -> Option<PathBuf> {
    let raw_path = Path::new(program);
    if raw_path.is_absolute() || program.contains('/') || program.contains('\\') {
        let candidate = if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            workspace_root.join(raw_path)
        };
        if candidate.is_file() && !is_uninstalled_macos_clt_shim(&candidate, workspace_root) {
            return Some(candidate);
        }
        return None;
    }

    let path_str = path_env?;
    for dir in std::env::split_paths(path_str) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(program);
        if cand.is_file() && !is_uninstalled_macos_clt_shim(&cand, workspace_root) {
            return Some(cand);
        }
    }
    None
}

/// Returns `true` when running on macOS and active Apple CLT / Xcode provides `tool`.
pub(crate) fn host_apple_clt_provides(tool: &str) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    if resolve_real_apple_developer_tool(tool).is_some() {
        return true;
    }
    if discover_filesystem_developer_dir().is_none() {
        return false;
    }
    run_apple_tool_trimmed("/usr/bin/xcrun", &["--find", tool]).is_ok()
}

/// Returns a concise operator note for export summaries when the workspace relies on native/Apple
/// tooling and targets macOS (either explicitly via `--target` or implicitly on a macOS host).
pub fn export_apple_prereq_note(
    workspace_root: &Path,
    lockfile: Option<&DmzLockfile>,
    target_platform: Option<&str>,
) -> Option<String> {
    let ecosystems = ToolchainDetector::detect_workspace_ecosystems(workspace_root, lockfile);
    export_apple_prereq_note_for_ecosystems(&ecosystems, target_platform)
}

/// Loads `lockfile_path` (if present) and evaluates [`export_apple_prereq_note`].
pub fn export_apple_prereq_note_from_lockfile(
    workspace_root: &Path,
    lockfile_path: &Path,
    target_platform: Option<&str>,
) -> Option<String> {
    let lockfile = if lockfile_path.is_file() {
        DmzLockfile::load_and_verify(lockfile_path).ok()
    } else {
        None
    };
    export_apple_prereq_note(workspace_root, lockfile.as_ref(), target_platform)
}

/// Evaluates whether exported `ecosystems` require a macOS target Apple CLT/Xcode prerequisite note.
pub fn export_apple_prereq_note_for_ecosystems(
    ecosystems: &HashSet<ToolchainEcosystem>,
    target_platform: Option<&str>,
) -> Option<String> {
    if !ecosystems_use_native_or_apple_tooling(ecosystems) || !target_is_macos(target_platform) {
        return None;
    }

    Some(
        "Target macOS prerequisite: target Macs must have Apple Command Line Tools installed ('xcode-select --install') and license accepted ('sudo xcodebuild -license accept'); Apple SDKs and CLT binaries are not bundled in archives."
            .to_string(),
    )
}

/// Classifies `xcrun` failure stderr into a structured `ApplePrereqStatus`.
pub fn classify_xcrun_failure(stderr: &str, developer_dir: Option<PathBuf>) -> ApplePrereqStatus {
    let trimmed = stderr.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower.contains("license") || lower.contains("agreeing to the") {
        return ApplePrereqStatus::LicenseNotAccepted {
            developer_dir,
            detail: if trimmed.is_empty() {
                "license agreement required".to_string()
            } else {
                trimmed.to_string()
            },
        };
    }

    if lower.contains("no developer tools were found")
        || lower.contains("requires the command line developer tools")
    {
        return ApplePrereqStatus::NotInstalled {
            reason: trimmed.to_string(),
        };
    }

    if lower.contains("invalid active developer path")
        || lower.contains("unable to find utility")
        || lower.contains("sdk")
    {
        return ApplePrereqStatus::BrokenOrUnusable {
            reason: trimmed.to_string(),
        };
    }

    if developer_dir.is_none() {
        ApplePrereqStatus::NotInstalled {
            reason: if trimmed.is_empty() {
                "xcode-select did not resolve a developer directory".to_string()
            } else {
                trimmed.to_string()
            },
        }
    } else {
        ApplePrereqStatus::BrokenOrUnusable {
            reason: if trimmed.is_empty() {
                "xcrun failed to resolve macOS SDK".to_string()
            } else {
                trimmed.to_string()
            },
        }
    }
}

fn run_apple_tool_trimmed(tool_path: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(tool_path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to invoke {}: {}", tool_path, e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Err(format!("{} returned empty output", tool_path));
        }
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Err(format!(
                "{} exited with status {:?}",
                tool_path,
                output.status.code()
            ))
        } else {
            Err(stderr)
        }
    }
}

/// Probes host Apple Command Line Tools / Xcode installation and license acceptance on macOS
/// without triggering the `/usr/bin/xcrun` GUI installer popup when developer tools are absent.
pub fn check_apple_developer_tools() -> ApplePrereqStatus {
    let Some(developer_dir) = discover_filesystem_developer_dir() else {
        return ApplePrereqStatus::NotInstalled {
            reason: "no active Apple Command Line Tools or Xcode directory found on disk"
                .to_string(),
        };
    };

    let dev_out = match run_apple_tool_trimmed("/usr/bin/xcode-select", &["-p"]) {
        Ok(out) => PathBuf::from(out),
        Err(_) => developer_dir,
    };

    if !dev_out.is_dir() || dev_out == Path::new("/") {
        return ApplePrereqStatus::NotInstalled {
            reason: format!(
                "active developer directory does not exist: {}",
                redact_home_prefix(&dev_out)
            ),
        };
    }

    let sdk_out =
        match run_apple_tool_trimmed("/usr/bin/xcrun", &["--sdk", "macosx", "--show-sdk-path"]) {
            Ok(out) => out,
            Err(err) => return classify_xcrun_failure(&err, Some(dev_out)),
        };

    let sdk_path = PathBuf::from(&sdk_out);
    if !sdk_path.is_dir() {
        return ApplePrereqStatus::BrokenOrUnusable {
            reason: format!(
                "resolved macOS SDK path does not exist: {}",
                redact_home_prefix(&sdk_path)
            ),
        };
    }

    // Verify core compiler utility resolves cleanly inside active developer dir (not an empty tree)
    if let Err(err) =
        run_apple_tool_trimmed("/usr/bin/xcrun", &["--sdk", "macosx", "--find", "clang"])
    {
        return classify_xcrun_failure(&err, Some(dev_out));
    }

    let sdk_version =
        run_apple_tool_trimmed("/usr/bin/xcrun", &["--sdk", "macosx", "--show-sdk-version"]).ok();

    ApplePrereqStatus::Ready {
        developer_dir: dev_out,
        sdk_path,
        sdk_version,
    }
}

/// Resolves the active host macOS SDK root path from `SDKROOT` or verified Apple CLT/Xcode
/// without spawning `/usr/bin/xcrun` when CLT/Xcode is missing.
pub fn resolve_host_macos_sdk_path() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    if let Some(sdkroot) = std::env::var_os("SDKROOT").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(sdkroot);
        if p.is_dir() {
            return Some(p);
        }
    }
    discover_filesystem_developer_dir()?;
    let sdk_out =
        run_apple_tool_trimmed("/usr/bin/xcrun", &["--sdk", "macosx", "--show-sdk-path"]).ok()?;
    let sdk_path = PathBuf::from(sdk_out);
    if sdk_path.is_dir() {
        Some(sdk_path)
    } else {
        None
    }
}

/// Returns `true` when Apple CLT is ready OR an explicit `SDKROOT` pairs with a bundled/standalone C compiler.
pub fn has_usable_apple_sdk_or_clt(workspace_root: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    if let Some(sdkroot) = std::env::var_os("SDKROOT").filter(|v| !v.is_empty()) {
        let sdk_dir = PathBuf::from(sdkroot);
        if sdk_dir.is_dir()
            && (crate::toolchain::ToolchainEnv::find_binary(
                &workspace_root.join(".dmz/toolchains/cc"),
                "bin/clang",
            )
            .is_some()
                || super::discovery::find_standalone_bin_in_path("clang", true).is_some())
        {
            return true;
        }
    }
    check_apple_developer_tools().is_ready()
}

/// Verifies macOS Apple CLT/Xcode prerequisites when required by `ecosystems`.
///
/// Returns `Ok(None)` when not running on macOS or when `ecosystems` does not require Apple CLT/Xcode.
pub fn verify_apple_prereq_for_ecosystems(
    ecosystems: &HashSet<ToolchainEcosystem>,
) -> Result<Option<ApplePrereqStatus>, ToolchainError> {
    if !workspace_requires_apple_clt(ecosystems) {
        return Ok(None);
    }

    let status = check_apple_developer_tools();
    if status.is_ready() {
        if let ApplePrereqStatus::Ready {
            ref developer_dir,
            ref sdk_path,
            ref sdk_version,
        } = status
        {
            tracing::debug!(
                developer_dir = %redact_home_prefix(developer_dir),
                sdk_path = %redact_home_prefix(sdk_path),
                sdk_version = ?sdk_version,
                "Verified macOS Apple CLT/Xcode host prerequisite"
            );
        }
        Ok(Some(status))
    } else {
        Err(ToolchainError::HostPrerequisiteMissing(
            status.to_error_message(),
        ))
    }
}

/// Detects workspace ecosystems and verifies macOS Apple CLT/Xcode prerequisites if required.
pub fn verify_apple_prereq_for_workspace(
    workspace_root: &Path,
    lockfile: Option<&DmzLockfile>,
) -> Result<Option<ApplePrereqStatus>, ToolchainError> {
    let ecosystems = ToolchainDetector::detect_workspace_ecosystems(workspace_root, lockfile);
    verify_apple_prereq_for_ecosystems(&ecosystems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_xcrun_failure_detects_license_and_missing_states() {
        let lic = classify_xcrun_failure(
            "You have not agreed to the Xcode license agreements. Run 'sudo xcodebuild -license'.",
            Some(PathBuf::from("/Applications/Xcode.app/Contents/Developer")),
        );
        assert!(matches!(lic, ApplePrereqStatus::LicenseNotAccepted { .. }));
        assert!(lic
            .remediation_hint()
            .contains("xcodebuild -license accept"));

        let missing = classify_xcrun_failure(
            "xcode-select: note: No developer tools were found, requesting install.",
            None,
        );
        assert!(matches!(missing, ApplePrereqStatus::NotInstalled { .. }));
        assert!(missing
            .remediation_hint()
            .contains("xcode-select --install"));

        let broken = classify_xcrun_failure(
            "xcrun: error: invalid active developer path (/missing/dir)",
            Some(PathBuf::from("/missing/dir")),
        );
        assert!(matches!(broken, ApplePrereqStatus::BrokenOrUnusable { .. }));
        assert!(broken.remediation_hint().contains("xcode-select --reset"));
    }

    #[test]
    fn test_workspace_requires_apple_clt_scoping() {
        let mut py_only = HashSet::new();
        py_only.insert(ToolchainEcosystem::Python);
        py_only.insert(ToolchainEcosystem::Node);
        assert!(!workspace_requires_apple_clt(&py_only));

        let mut rust_ws = HashSet::new();
        rust_ws.insert(ToolchainEcosystem::Rust);
        assert_eq!(
            workspace_requires_apple_clt(&rust_ws),
            cfg!(target_os = "macos")
        );

        let mut apple_ws = HashSet::new();
        apple_ws.insert(ToolchainEcosystem::AppleSdk);
        assert_eq!(
            workspace_requires_apple_clt(&apple_ws),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn test_export_apple_prereq_note_respects_ecosystems_and_target_platform() {
        let mut py_only = HashSet::new();
        py_only.insert(ToolchainEcosystem::Python);
        assert!(export_apple_prereq_note_for_ecosystems(&py_only, Some("darwin-arm64")).is_none());

        let mut rust_ws = HashSet::new();
        rust_ws.insert(ToolchainEcosystem::Rust);

        // Explicit darwin / macos targets must notify regardless of export host OS
        let darwin_note =
            export_apple_prereq_note_for_ecosystems(&rust_ws, Some("aarch64-apple-darwin"));
        assert!(darwin_note
            .as_deref()
            .is_some_and(|n| n.contains("xcode-select --install")));

        let macos_note = export_apple_prereq_note_for_ecosystems(&rust_ws, Some("macos-arm64"));
        assert!(macos_note.is_some());

        // Explicit Linux / Windows targets must NOT show an Apple CLT reminder
        assert!(export_apple_prereq_note_for_ecosystems(&rust_ws, Some("linux-x86_64")).is_none());
        assert!(
            export_apple_prereq_note_for_ecosystems(&rust_ws, Some("windows-x86_64")).is_none()
        );

        // Implicit target uses host OS
        assert_eq!(
            export_apple_prereq_note_for_ecosystems(&rust_ws, None).is_some(),
            cfg!(target_os = "macos")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_host_apple_prereq_probe_on_macos_if_installed() {
        if Path::new("/Library/Developer/CommandLineTools").is_dir()
            || Path::new("/Applications/Xcode.app/Contents/Developer").is_dir()
        {
            let status = check_apple_developer_tools();
            if let ApplePrereqStatus::Ready {
                developer_dir,
                sdk_path,
                ..
            } = status
            {
                assert!(developer_dir.is_dir());
                assert!(sdk_path.is_dir());
            }
        }
    }

    #[test]
    fn test_target_is_macos_matching() {
        assert!(target_is_macos(Some("macos-arm64")));
        assert!(target_is_macos(Some("aarch64-apple-darwin")));
        assert!(!target_is_macos(Some("linux-x86_64")));
        assert!(!target_is_macos(Some("windows-x86_64")));
        assert_eq!(target_is_macos(None), cfg!(target_os = "macos"));
    }

    #[test]
    fn test_real_developer_tool_resolver_never_returns_usr_bin_stub() {
        if let Some(tool) = resolve_real_apple_developer_tool("install_name_tool") {
            assert_ne!(tool.parent(), Some(Path::new("/usr/bin")));
            assert!(tool.is_file());
        }
    }

    #[test]
    fn test_is_macho_xcselect_stub_detects_libxcselect_dylib() {
        let dir = tempfile::tempdir().unwrap();
        let stub_path = dir.path().join("python3");
        let non_stub_path = dir.path().join("env");

        let stub_bytes = crate::toolchain::setup::macho::tests::build_synthetic_macho64(
            "@loader_path",
            "/usr/lib/libxcselect.dylib",
        );
        let non_stub_bytes = crate::toolchain::setup::macho::tests::build_synthetic_macho64(
            "@loader_path",
            "/usr/lib/libSystem.B.dylib",
        );

        std::fs::write(&stub_path, stub_bytes).unwrap();
        std::fs::write(&non_stub_path, non_stub_bytes).unwrap();

        assert!(is_macho_xcselect_stub(&stub_path));
        assert!(!is_macho_xcselect_stub(&non_stub_path));
    }

    #[test]
    fn test_should_skip_uninstalled_macos_clt_command_handles_non_stub_safely() {
        let dir = tempfile::tempdir().unwrap();
        let non_stub = dir.path().join("my-tool");
        let non_stub_bytes = crate::toolchain::setup::macho::tests::build_synthetic_macho64(
            "@loader_path",
            "/usr/lib/libSystem.B.dylib",
        );
        std::fs::write(&non_stub, non_stub_bytes).unwrap();
        let envs = std::collections::HashMap::new();
        assert!(should_skip_uninstalled_macos_clt_command(
            &non_stub.display().to_string(),
            &envs,
            dir.path()
        )
        .is_none());
    }
}
