use std::fs;
use std::path::{Path, PathBuf};

/// Information about sidecar binary format, native dynamic libraries, and ABI compatibility
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidecarAbiReport {
    /// Discovered dynamic shared libraries (.so, .dylib, .dll, .node)
    pub dynamic_libraries: Vec<PathBuf>,
    /// Whether native binaries in the sidecar are fully static (musl / no external dynamic interpreter)
    pub is_static: bool,
    /// Diagnostic warnings regarding ABI, glibc dependencies, or missing static linkage
    pub warnings: Vec<String>,
}

/// Checks whether a POSIX socket path approaches or exceeds system path length limits
pub fn check_socket_path_length(path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    let len = path_str.len();

    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    let max_len = 104;
    #[cfg(target_os = "linux")]
    let max_len = 108;
    #[cfg(not(unix))]
    let max_len = 260;

    if len >= max_len {
        Some(format!(
            "Socket path '{}' length ({} bytes) exceeds the platform sun_path limit of {} bytes! Connection will fail. Use safe_socket_path() to anchor to a short temp hash.",
            path_str, len, max_len
        ))
    } else if len >= max_len.saturating_sub(10) {
        Some(format!(
            "Socket path '{}' length ({} bytes) is dangerously close to the platform sun_path limit of {} bytes. Consider using safe_socket_path().",
            path_str, len, max_len
        ))
    } else {
        None
    }
}

/// Inspects a sidecar directory to analyze native binaries, shared libraries, and static linkage
pub fn analyze_sidecar_abi(sidecar_path: &Path) -> SidecarAbiReport {
    let mut dynamic_libraries = Vec::new();
    let mut warnings = Vec::new();
    let mut has_native_binary = false;
    let mut has_dynamic_binary = false;

    if sidecar_path.is_file() {
        check_file(
            sidecar_path,
            &mut dynamic_libraries,
            &mut has_native_binary,
            &mut has_dynamic_binary,
            &mut warnings,
        );
    } else if sidecar_path.is_dir() {
        scan_dir(
            sidecar_path,
            &mut dynamic_libraries,
            &mut has_native_binary,
            &mut has_dynamic_binary,
            &mut warnings,
            0,
        );
    }

    if !dynamic_libraries.is_empty() {
        let sample = dynamic_libraries[0]
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        warnings.push(format!(
            "Sidecar at '{}' contains {} dynamic shared library file(s) (e.g. '{}'). Air-gapped target deployment may fail if target host GLIBC/libc ABI differs. For maximum portability, compile sidecars with static musl/single-binary linkage.",
            sidecar_path.display(),
            dynamic_libraries.len(),
            sample
        ));
    }

    let is_static = has_native_binary && !has_dynamic_binary && dynamic_libraries.is_empty();

    SidecarAbiReport {
        dynamic_libraries,
        is_static,
        warnings,
    }
}

fn scan_dir(
    dir: &Path,
    dynamic_libraries: &mut Vec<PathBuf>,
    has_native: &mut bool,
    has_dynamic: &mut bool,
    warnings: &mut Vec<String>,
    depth: usize,
) {
    if depth > 4 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') || name == "target" || name == ".git" {
            continue;
        }

        if path.is_dir() {
            scan_dir(
                &path,
                dynamic_libraries,
                has_native,
                has_dynamic,
                warnings,
                depth + 1,
            );
        } else if path.is_file() {
            check_file(&path, dynamic_libraries, has_native, has_dynamic, warnings);
        }
    }
}

fn check_file(
    path: &Path,
    dynamic_libraries: &mut Vec<PathBuf>,
    has_native: &mut bool,
    has_dynamic: &mut bool,
    _warnings: &mut Vec<String>,
) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "so" || ext == "dylib" || ext == "dll" || ext == "node" {
        dynamic_libraries.push(path.to_path_buf());
        *has_native = true;
        *has_dynamic = true;
        return;
    }

    // Inspect binary headers for executable files
    let mut header = [0u8; 1024];
    let bytes_read = match fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read(&mut header).unwrap_or(0)
        }
        Err(_) => 0,
    };

    if bytes_read >= 4 {
        let is_elf = &header[0..4] == b"\x7fELF";
        let is_macho = header[0..4] == [0xfe, 0xed, 0xfa, 0xce]
            || header[0..4] == [0xfe, 0xed, 0xfa, 0xcf]
            || header[0..4] == [0xce, 0xfa, 0xed, 0xfe]
            || header[0..4] == [0xcf, 0xfa, 0xed, 0xfe]
            || header[0..4] == [0xca, 0xfe, 0xba, 0xbe];

        if is_elf {
            *has_native = true;
            let header_slice = &header[..bytes_read];
            // Check for dynamic interpreter string
            if header_slice
                .windows(10)
                .any(|w| w == b"/ld-linux-" || w == b"/lib/ld-li" || w == b"/lib64/ld-")
            {
                *has_dynamic = true;
            }
        } else if is_macho {
            *has_native = true;
            let header_slice = &header[..bytes_read];
            if header_slice
                .windows(7)
                .any(|w| w == b"/usr/li" || w == b"dylib\0")
            {
                *has_dynamic = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_socket_path_length_guard() {
        let short_path = Path::new("/tmp/dmz_short.sock");
        assert!(check_socket_path_length(short_path).is_none());

        let long_str = format!("/tmp/{}.sock", "a".repeat(110));
        let long_path = Path::new(&long_str);
        assert!(check_socket_path_length(long_path).is_some());
    }

    #[test]
    fn test_analyze_sidecar_abi_with_dylib() {
        let temp_dir = tempfile::tempdir().unwrap();
        let lib_path = temp_dir.path().join("addon.node");
        fs::write(&lib_path, b"mock addon binary").unwrap();

        let report = analyze_sidecar_abi(temp_dir.path());
        assert_eq!(report.dynamic_libraries.len(), 1);
        assert!(!report.warnings.is_empty());
        assert!(!report.is_static);
    }
}
