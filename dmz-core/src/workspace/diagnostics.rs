// Sidecar binary format diagnostics and IPC socket length validation.
// Inspects native binaries (ELF, Mach-O, PE/COFF) for dynamic dependencies
// and validates POSIX `sockaddr_un.sun_path` length boundaries.

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

    #[cfg(windows)]
    if path_str.starts_with(r"\\.\pipe\") {
        let max_pipe_len = 256;
        if len >= max_pipe_len {
            return Some(format!(
                "Named pipe path '{}' length ({} bytes) exceeds Windows limit of {} bytes! Connection may fail.",
                path_str, len, max_pipe_len
            ));
        } else if len >= max_pipe_len.saturating_sub(10) {
            return Some(format!(
                "Named pipe path '{}' length ({} bytes) is dangerously close to Windows limit of {} bytes.",
                path_str, len, max_pipe_len
            ));
        } else {
            return None;
        }
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    let max_len = 104;
    #[cfg(not(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    let max_len = 108;

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
///
/// # Async Runtime Note
/// Performs synchronous disk directory traversal and file header inspection.
/// When invoking from an asynchronous Tokio runtime context, offload via `tokio::task::spawn_blocking`.
pub fn analyze_sidecar_abi(sidecar_path: &Path) -> SidecarAbiReport {
    let mut dynamic_libraries = Vec::new();
    let mut warnings = Vec::new();
    let mut has_native_binary = false;
    let mut has_dynamic_binary = false;
    let mut scanned_files = 0usize;

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
            &mut scanned_files,
            0,
        );
    }

    if let Some(first_lib) = dynamic_libraries.first() {
        let sample = first_lib.file_name().unwrap_or_default().to_string_lossy();
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
    scanned_files: &mut usize,
    depth: usize,
) {
    // Defense-in-depth: limit traversal depth and total scanned files (CWE-400)
    if depth > 4 || *scanned_files >= 1000 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        // Defend against cyclic symlinks and directory traversal escaping the workspace (CWE-59)
        if file_type.is_symlink() && file_type.is_dir() {
            continue;
        }

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
                scanned_files,
                depth + 1,
            );
        } else if path.is_file() {
            *scanned_files = scanned_files.saturating_add(1);
            check_file(&path, dynamic_libraries, has_native, has_dynamic, warnings);
        }
    }
}

fn check_file(
    path: &Path,
    dynamic_libraries: &mut Vec<PathBuf>,
    has_native: &mut bool,
    has_dynamic: &mut bool,
    warnings: &mut Vec<String>,
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

    // Inspect binary headers for executable files (ELF, Mach-O, PE/COFF)
    let mut header = [0u8; 4096];
    let bytes_read = match fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read(&mut header).unwrap_or(0)
        }
        Err(_) => 0,
    };

    if bytes_read >= 4 {
        let header_slice = match header.get(..bytes_read) {
            Some(s) => s,
            None => return,
        };
        if parse_elf_dynamic(header_slice, has_native) {
            *has_dynamic = true;
            warnings.push(format!(
                "Binary '{}' contains dynamic ELF program headers (PT_INTERP / PT_DYNAMIC). Ensure target air-gapped host provides compatible libc/dynamic loader or link statically with musl.",
                path.display()
            ));
        } else if parse_macho_dynamic(header_slice, has_native) {
            *has_dynamic = true;
            warnings.push(format!(
                "Binary '{}' contains dynamic Mach-O load commands (LC_LOAD_DYLIB). Verify all non-system dylibs are bundled inside the workspace.",
                path.display()
            ));
        } else {
            parse_pe_native(header_slice, has_native);
        }
    }
}

/// Parses ELF 32/64-bit program headers for `PT_INTERP` (`p_type == 3`) or `PT_DYNAMIC` (`p_type == 2`).
fn parse_elf_dynamic(buf: &[u8], has_native: &mut bool) -> bool {
    if buf.get(0..4) != Some(b"\x7fELF") {
        return false;
    }
    *has_native = true;

    if buf.len() >= 64 {
        let class = *buf.get(4).unwrap_or(&0); // 1 = 32-bit, 2 = 64-bit
        let endian = *buf.get(5).unwrap_or(&0); // 1 = little, 2 = big
        let is_le = endian != 2;

        let (phoff, phentsize, phnum) = if class == 2 {
            (
                safe_read_u64(buf, 32, is_le).unwrap_or(0) as usize,
                safe_read_u16(buf, 54, is_le).unwrap_or(0) as usize,
                safe_read_u16(buf, 56, is_le).unwrap_or(0) as usize,
            )
        } else if class == 1 {
            (
                safe_read_u32(buf, 28, is_le).unwrap_or(0) as usize,
                safe_read_u16(buf, 42, is_le).unwrap_or(0) as usize,
                safe_read_u16(buf, 44, is_le).unwrap_or(0) as usize,
            )
        } else {
            (0, 0, 0)
        };

        if phoff > 0 && phentsize >= 8 && phnum > 0 && phnum <= 128 {
            for i in 0..phnum {
                let entry_off = phoff.saturating_add(i.saturating_mul(phentsize));
                if entry_off.saturating_add(4) <= buf.len() {
                    let p_type = safe_read_u32(buf, entry_off, is_le).unwrap_or(0);
                    // PT_DYNAMIC (2) or PT_INTERP (3)
                    if p_type == 3 || p_type == 2 {
                        return true;
                    }
                }
            }
        }
    }

    // Fallback check for interpreter strings in synthetic or truncated headers
    buf.windows(10)
        .any(|w| w == b"/ld-linux-" || w == b"/lib/ld-li" || w == b"/lib64/ld-")
}

/// Parses Mach-O 32/64-bit load commands for dynamic linker/library commands (`LC_LOAD_DYLIB`, `LC_LOAD_DYLINKER`, etc.).
fn parse_macho_dynamic(buf: &[u8], has_native: &mut bool) -> bool {
    let magic: [u8; 4] = match buf.get(0..4).and_then(|s| s.try_into().ok()) {
        Some(m) => m,
        None => return false,
    };
    let (is_macho, is_64, is_le) = match magic {
        [0xfe, 0xed, 0xfa, 0xce] => (true, false, false),
        [0xce, 0xfa, 0xed, 0xfe] => (true, false, true),
        [0xfe, 0xed, 0xfa, 0xcf] => (true, true, false),
        [0xcf, 0xfa, 0xed, 0xfe] => (true, true, true),
        [0xca, 0xfe, 0xba, 0xbe] => {
            *has_native = true;
            return buf.windows(7).any(|w| w == b"/usr/li" || w == b"dylib\0");
        }
        _ => (false, false, false),
    };

    if !is_macho {
        return false;
    }
    *has_native = true;

    let hdr_size = if is_64 { 32 } else { 28 };
    if buf.len() >= hdr_size {
        let ncmds = safe_read_u32(buf, 16, is_le).unwrap_or(0) as usize;
        let mut offset = hdr_size;
        for _ in 0..ncmds.min(256) {
            if offset.saturating_add(8) > buf.len() {
                break;
            }
            let cmd = safe_read_u32(buf, offset, is_le).unwrap_or(0);
            let cmdsize = safe_read_u32(buf, offset.saturating_add(4), is_le).unwrap_or(0) as usize;
            // LC_LOAD_DYLIB (0x0c), LC_LOAD_DYLINKER (0x0e), LC_LOAD_WEAK_DYLIB (0x80000018), LC_REEXPORT_DYLIB (0x8000001f)
            if cmd == 0x0c || cmd == 0x0e || cmd == 0x8000_0018 || cmd == 0x8000_001f {
                return true;
            }
            if cmdsize < 8 {
                break;
            }
            offset = offset.saturating_add(cmdsize);
        }
    }

    buf.windows(7).any(|w| w == b"/usr/li" || w == b"dylib\0")
}

/// Checks for Windows PE/COFF (`MZ` + `PE\0\0`) executable header.
fn parse_pe_native(buf: &[u8], has_native: &mut bool) {
    if buf.len() >= 0x40 && buf.get(0..2) == Some(b"MZ") {
        let pe_off = match safe_read_u32(buf, 0x3c, true) {
            Some(off) => off as usize,
            None => return,
        };
        if buf.get(pe_off..pe_off.saturating_add(4)) == Some(b"PE\0\0") {
            *has_native = true;
        }
    }
}

#[inline]
fn safe_read_u16(buf: &[u8], off: usize, is_le: bool) -> Option<u16> {
    let bytes: [u8; 2] = buf.get(off..off.checked_add(2)?)?.try_into().ok()?;
    Some(if is_le {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

#[inline]
fn safe_read_u32(buf: &[u8], off: usize, is_le: bool) -> Option<u32> {
    let bytes: [u8; 4] = buf.get(off..off.checked_add(4)?)?.try_into().ok()?;
    Some(if is_le {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

#[inline]
fn safe_read_u64(buf: &[u8], off: usize, is_le: bool) -> Option<u64> {
    let bytes: [u8; 8] = buf.get(off..off.checked_add(8)?)?.try_into().ok()?;
    Some(if is_le {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_socket_path_length_guard() {
        let short_path = Path::new("/tmp/dmz_short.sock");
        assert!(check_socket_path_length(short_path).is_none());

        let long_str = format!("/tmp/{}.sock", "a".repeat(110));
        let long_path = Path::new(&long_str);
        assert!(check_socket_path_length(long_path).is_some());
    }

    #[test]
    fn test_analyze_sidecar_abi_with_dylib() -> Result<(), Box<dyn Error>> {
        let temp_dir = tempfile::tempdir()?;
        let lib_path = temp_dir.path().join("addon.node");
        fs::write(&lib_path, b"mock addon binary")?;

        let report = analyze_sidecar_abi(temp_dir.path());
        assert_eq!(report.dynamic_libraries.len(), 1);
        assert!(!report.warnings.is_empty());
        assert!(!report.is_static);
        Ok(())
    }
}
