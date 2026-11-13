// Execution permissions and quarantine clearance for toolchains

use super::super::ToolchainError;
#[cfg(unix)]
use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

/// Configure execution permissions (0o755) and clear macOS quarantine flags.
pub fn apply_permissions_and_quarantine(toolchains_dir: &Path) -> Result<(), ToolchainError> {
    let _ = toolchains_dir;
    // Ensure 0o755 execution permissions on all binary files.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fn make_exec_recursive(dir: &Path) {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        make_exec_recursive(&p);
                    } else if p.is_file() {
                        let in_bin = p
                            .parent()
                            .and_then(|parent| parent.file_name())
                            .is_some_and(|name| name == "bin");
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        let is_shlib_or_tool = fname.ends_with(".dylib")
                            || fname.ends_with(".so")
                            || fname.contains(".so.")
                            || p.to_string_lossy().contains("pkg/tool")
                            || p.to_string_lossy().contains("libexec");
                        if in_bin || is_shlib_or_tool {
                            if let Err(e) =
                                fs::set_permissions(&p, fs::Permissions::from_mode(0o755))
                            {
                                tracing::warn!(
                                    path = ?p,
                                    error = %e,
                                    "Failed to set 0o755 execution permissions"
                                );
                            }
                        }
                    }
                }
            }
        }
        make_exec_recursive(toolchains_dir);
    }

    // Clear macOS quarantine attribute (xattr -cr)
    #[cfg(target_os = "macos")]
    {
        match Command::new("xattr")
            .arg("-cr")
            .arg(toolchains_dir)
            .output()
        {
            Ok(out) if !out.status.success() => {
                tracing::warn!(
                    status = ?out.status.code(),
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "Non-zero exit code while clearing macOS quarantine xattrs on toolchains"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to invoke xattr to clear quarantine attributes on toolchains"
                );
            }
            _ => {}
        }
    }

    Ok(())
}
