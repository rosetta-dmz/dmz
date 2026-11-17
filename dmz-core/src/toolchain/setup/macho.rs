// Pure-Rust bounded Mach-O header and load-command inspector (zero subprocess / zero unsafe).

use super::bin_io::{read_cstr_bounded, read_u32, read_u64, Endian};
use crate::toolchain::ToolchainError;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MH_MAGIC: u32 = 0xfeed_face;
const MH_CIGAM: u32 = 0xcefa_edfe;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const FAT_CIGAM_64: u32 = 0xbfba_feca;

const MH_DYLIB: u32 = 0x6;
const LC_REQ_DYLD: u32 = 0x8000_0000;
const LC_LOAD_DYLIB: u32 = 0xc;
const LC_ID_DYLIB: u32 = 0xd;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
const LC_RPATH: u32 = 0x1c | LC_REQ_DYLD;
const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
const LC_LAZY_LOAD_DYLIB: u32 = 0x20 | LC_REQ_DYLD;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x23 | LC_REQ_DYLD;

const MAX_MACH_O_INSPECT_BYTES: usize = 4 * 1024 * 1024;
const MAX_SLICE_HEADER_BYTES: usize = 512 * 1024;
const MAX_LOAD_COMMANDS: u32 = 4096;

/// Parsed Mach-O load-command metadata needed for bundle relocation checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachOLoadInfo {
    pub is_dylib: bool,
    pub rpaths: Vec<String>,
    pub dylib_deps: Vec<String>,
    pub install_id: Option<String>,
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    read_u32(bytes, offset, Endian::Big)
}

fn parse_fat_header(magic_be: u32) -> Option<(Endian, bool)> {
    match magic_be {
        FAT_MAGIC => Some((Endian::Big, false)),
        FAT_CIGAM => Some((Endian::Little, false)),
        FAT_MAGIC_64 => Some((Endian::Big, true)),
        FAT_CIGAM_64 => Some((Endian::Little, true)),
        _ => None,
    }
}

fn decode_fat_arch(table: &[u8], idx: usize, endian: Endian, is_fat64: bool) -> Option<(u64, u64)> {
    let stride = if is_fat64 { 32usize } else { 20usize };
    let base = idx.checked_mul(stride)?;
    if is_fat64 {
        let off = read_u64(table, base.checked_add(8)?, endian)?;
        let size = read_u64(table, base.checked_add(16)?, endian)?;
        Some((off, size))
    } else {
        let off = u64::from(read_u32(table, base.checked_add(8)?, endian)?);
        let size = u64::from(read_u32(table, base.checked_add(12)?, endian)?);
        Some((off, size))
    }
}

fn parse_single_arch(slice: &[u8], out: &mut MachOLoadInfo) -> Option<()> {
    let magic_be = read_u32_be(slice, 0)?;
    let (endian, header_size) = match magic_be {
        MH_MAGIC => (Endian::Big, 28usize),
        MH_CIGAM => (Endian::Little, 28usize),
        MH_MAGIC_64 => (Endian::Big, 32usize),
        MH_CIGAM_64 => (Endian::Little, 32usize),
        _ => return None,
    };

    let filetype = read_u32(slice, 12, endian)?;
    if filetype == MH_DYLIB {
        out.is_dylib = true;
    }
    let ncmds = read_u32(slice, 16, endian)?.min(MAX_LOAD_COMMANDS);
    let sizeofcmds = read_u32(slice, 20, endian)? as usize;

    let cmds_end = header_size.checked_add(sizeofcmds)?.min(slice.len());
    let mut cursor = header_size;

    for _ in 0..ncmds {
        if cursor.saturating_add(8) > cmds_end {
            break;
        }
        let cmd = read_u32(slice, cursor, endian)?;
        let cmdsize = read_u32(slice, cursor.checked_add(4)?, endian)? as usize;
        if cmdsize < 8 {
            break;
        }
        let curr_cmd_end = cursor.saturating_add(cmdsize).min(cmds_end);

        match cmd {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LAZY_LOAD_DYLIB
            | LC_LOAD_UPWARD_DYLIB | LC_ID_DYLIB => {
                if let Some(str_off) = read_u32(slice, cursor.checked_add(8)?, endian) {
                    if let Some(val) = read_cstr_bounded(
                        slice,
                        cursor.saturating_add(str_off as usize),
                        curr_cmd_end,
                    ) {
                        if cmd == LC_ID_DYLIB {
                            if out.install_id.is_none() {
                                out.install_id = Some(val);
                            }
                        } else if !out.dylib_deps.contains(&val) {
                            out.dylib_deps.push(val);
                        }
                    }
                }
            }
            LC_RPATH => {
                if let Some(str_off) = read_u32(slice, cursor.checked_add(8)?, endian) {
                    if let Some(val) = read_cstr_bounded(
                        slice,
                        cursor.saturating_add(str_off as usize),
                        curr_cmd_end,
                    ) {
                        if !out.rpaths.contains(&val) {
                            out.rpaths.push(val);
                        }
                    }
                }
            }
            _ => {}
        }

        cursor = curr_cmd_end;
    }

    Some(())
}

fn collect_fat_slice_spans_from_file(file: &mut File) -> Option<Vec<(u64, u64)>> {
    if file.seek(SeekFrom::Start(0)).is_err() {
        return None;
    }
    let mut header_probe = [0u8; 8];
    if file.read_exact(&mut header_probe).is_err() {
        return None;
    }
    let magic_be = read_u32_be(&header_probe, 0)?;
    if let Some((fat_endian, is_fat64)) = parse_fat_header(magic_be) {
        let nfat = read_u32(&header_probe, 4, fat_endian)?.min(16);
        let stride = if is_fat64 { 32usize } else { 20usize };
        let fat_table_bytes = (nfat as usize).checked_mul(stride)?;
        let mut fat_table = vec![0u8; fat_table_bytes];
        file.read_exact(&mut fat_table).ok()?;

        let mut spans = Vec::with_capacity(nfat as usize);
        for idx in 0..nfat {
            if let Some(span) = decode_fat_arch(&fat_table, idx as usize, fat_endian, is_fat64) {
                spans.push(span);
            }
        }
        Some(spans)
    } else {
        None
    }
}

/// Parses Mach-O headers and load commands from `path` without spawning external `otool` processes.
///
/// For universal (`FAT`) binaries larger than 4 MiB, seeks directly to each slice's offset so
/// `arm64` slices beyond the initial 4 MiB window are still inspected in bounded memory.
#[must_use]
pub fn inspect_macho_load_info(path: &Path) -> Option<MachOLoadInfo> {
    let mut file = File::open(path).ok()?;
    if let Some(spans) = collect_fat_slice_spans_from_file(&mut file) {
        let mut info = MachOLoadInfo::default();
        let mut parsed_any = false;
        for (slice_off, slice_size_u64) in spans {
            let slice_size = usize::try_from(slice_size_u64).unwrap_or(MAX_SLICE_HEADER_BYTES);
            let to_read = slice_size.min(MAX_SLICE_HEADER_BYTES);
            if to_read < 32 {
                continue;
            }
            if file.seek(SeekFrom::Start(slice_off)).is_err() {
                continue;
            }
            let mut slice_buf = Vec::with_capacity(to_read);
            if (&mut file)
                .take(to_read as u64)
                .read_to_end(&mut slice_buf)
                .is_ok()
                && parse_single_arch(&slice_buf, &mut info).is_some()
            {
                parsed_any = true;
            }
        }
        return parsed_any.then_some(info);
    }

    if file.seek(SeekFrom::Start(0)).is_err() {
        return None;
    }
    let mut buf = Vec::new();
    file.take(MAX_MACH_O_INSPECT_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    inspect_macho_bytes(&buf)
}

/// Parses Mach-O headers and load commands from an in-memory buffer.
#[must_use]
pub fn inspect_macho_bytes(buf: &[u8]) -> Option<MachOLoadInfo> {
    let magic_be = read_u32_be(buf, 0)?;
    let mut info = MachOLoadInfo::default();

    if let Some((fat_endian, is_fat64)) = parse_fat_header(magic_be) {
        let nfat = read_u32(buf, 4, fat_endian)?.min(16);
        let table = buf.get(8..)?;
        let mut parsed_any = false;
        for idx in 0..nfat {
            let Some((off_u64, size_u64)) =
                decode_fat_arch(table, idx as usize, fat_endian, is_fat64)
            else {
                continue;
            };
            let Ok(slice_off) = usize::try_from(off_u64) else {
                continue;
            };
            let Ok(slice_size) = usize::try_from(size_u64) else {
                continue;
            };
            let Some(end) = slice_off.checked_add(slice_size).map(|e| e.min(buf.len())) else {
                continue;
            };
            if let Some(sub) = buf.get(slice_off..end) {
                if parse_single_arch(sub, &mut info).is_some() {
                    parsed_any = true;
                }
            }
        }
        return parsed_any.then_some(info);
    }

    parse_single_arch(buf, &mut info)?;
    Some(info)
}

fn patch_slice_dylib_paths(
    slice: &[u8],
    changes: &[(&str, &str)],
    desired_id: Option<&str>,
) -> Option<Vec<(usize, Vec<u8>)>> {
    let magic_be = read_u32_be(slice, 0)?;
    let (endian, header_size) = match magic_be {
        MH_MAGIC => (Endian::Big, 28usize),
        MH_CIGAM => (Endian::Little, 28usize),
        MH_MAGIC_64 => (Endian::Big, 32usize),
        MH_CIGAM_64 => (Endian::Little, 32usize),
        _ => return None,
    };

    let filetype = read_u32(slice, 12, endian)?;
    let ncmds = read_u32(slice, 16, endian)?.min(MAX_LOAD_COMMANDS);
    let sizeofcmds = read_u32(slice, 20, endian)? as usize;
    let cmds_end = header_size.checked_add(sizeofcmds)?.min(slice.len());
    let mut cursor = header_size;
    let mut patches: Vec<(usize, Vec<u8>)> = Vec::new();

    for _ in 0..ncmds {
        if cursor.saturating_add(8) > cmds_end {
            break;
        }
        let cmd = read_u32(slice, cursor, endian)?;
        let cmdsize = read_u32(slice, cursor.checked_add(4)?, endian)? as usize;
        if cmdsize < 8 {
            break;
        }
        let curr_cmd_end = cursor.saturating_add(cmdsize).min(cmds_end);

        let is_dep_cmd = matches!(
            cmd,
            LC_LOAD_DYLIB
                | LC_LOAD_WEAK_DYLIB
                | LC_REEXPORT_DYLIB
                | LC_LAZY_LOAD_DYLIB
                | LC_LOAD_UPWARD_DYLIB
        );
        let is_id_cmd = cmd == LC_ID_DYLIB && filetype == MH_DYLIB;

        if is_dep_cmd || is_id_cmd {
            if let Some(str_off) = read_u32(slice, cursor.checked_add(8)?, endian) {
                let slot_start = cursor.saturating_add(str_off as usize);
                if let Some(current) = read_cstr_bounded(slice, slot_start, curr_cmd_end) {
                    let replacement = if is_id_cmd {
                        desired_id.filter(|&id| id != current)
                    } else {
                        changes
                            .iter()
                            .find(|(old, _)| *old == current)
                            .map(|(_, new)| *new)
                    };
                    if let Some(new_val) = replacement {
                        let slot_cap = curr_cmd_end.saturating_sub(slot_start);
                        // Strict `<` guarantees space for the trailing NUL byte.
                        if new_val.len() < slot_cap {
                            let mut patch_buf = vec![0u8; slot_cap];
                            if let Some(dst) = patch_buf.get_mut(..new_val.len()) {
                                dst.copy_from_slice(new_val.as_bytes());
                                patches.push((slot_start, patch_buf));
                            }
                        }
                    }
                }
            }
        }

        cursor = curr_cmd_end;
    }

    Some(patches)
}

/// Rewrites matching `LC_*_DYLIB` paths and optional `LC_ID_DYLIB` in place across thin and FAT
/// slices when replacements fit inside existing padded command slots (zero-`install_name_tool` fallback).
///
/// # Errors
/// Returns `ToolchainError::SetupFailed` if `path` cannot be opened, inspected, or written.
pub fn patch_macho_dylib_paths_in_place(
    path: &Path,
    changes: &[(&str, &str)],
    desired_id: Option<&str>,
) -> Result<bool, ToolchainError> {
    if changes.is_empty() && desired_id.is_none() {
        return Ok(false);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| {
            ToolchainError::SetupFailed(format!("Failed to open Mach-O for patching: {e}"))
        })?;

    let slices: Vec<(u64, u64)> = if let Some(spans) = collect_fat_slice_spans_from_file(&mut file)
    {
        spans
    } else {
        let file_len = file
            .metadata()
            .map_err(|e| ToolchainError::SetupFailed(format!("Failed to stat Mach-O: {e}")))?
            .len();
        vec![(0u64, file_len)]
    };

    let mut modified = false;
    for (slice_off, slice_size_u64) in slices {
        let to_read = usize::try_from(slice_size_u64)
            .unwrap_or(MAX_SLICE_HEADER_BYTES)
            .min(MAX_SLICE_HEADER_BYTES);
        if to_read < 32 {
            continue;
        }
        if file.seek(SeekFrom::Start(slice_off)).is_err() {
            continue;
        }
        let mut slice_buf = Vec::with_capacity(to_read);
        if (&mut file)
            .take(to_read as u64)
            .read_to_end(&mut slice_buf)
            .is_err()
        {
            continue;
        }
        let Some(patches) = patch_slice_dylib_paths(&slice_buf, changes, desired_id) else {
            continue;
        };
        for (rel_off, data) in patches {
            let Some(abs_off) = slice_off.checked_add(rel_off as u64) else {
                continue;
            };
            file.seek(SeekFrom::Start(abs_off)).map_err(|e| {
                ToolchainError::SetupFailed(format!("Failed to seek Mach-O patch offset: {e}"))
            })?;
            file.write_all(&data).map_err(|e| {
                ToolchainError::SetupFailed(format!("Failed to write Mach-O patch: {e}"))
            })?;
            modified = true;
        }
    }

    Ok(modified)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn build_synthetic_macho64(rpath: &str, dep: &str) -> Vec<u8> {
        let mut rpath_cmd = Vec::new();
        let mut rpath_bytes = rpath.as_bytes().to_vec();
        rpath_bytes.push(0);
        let rpath_len = (12 + rpath_bytes.len()).div_ceil(8) * 8;
        let rpath_len_u32 = u32::try_from(rpath_len).unwrap_or(0);
        rpath_cmd.extend_from_slice(&LC_RPATH.to_le_bytes());
        rpath_cmd.extend_from_slice(&rpath_len_u32.to_le_bytes());
        rpath_cmd.extend_from_slice(&12u32.to_le_bytes());
        rpath_cmd.extend_from_slice(&rpath_bytes);
        rpath_cmd.resize(rpath_len, 0);

        let mut dep_cmd = Vec::new();
        let mut dep_bytes = dep.as_bytes().to_vec();
        dep_bytes.push(0);
        let dep_len = (24 + dep_bytes.len()).div_ceil(8) * 8;
        let dep_len_u32 = u32::try_from(dep_len).unwrap_or(0);
        dep_cmd.extend_from_slice(&LC_LOAD_DYLIB.to_le_bytes());
        dep_cmd.extend_from_slice(&dep_len_u32.to_le_bytes());
        dep_cmd.extend_from_slice(&24u32.to_le_bytes());
        dep_cmd.extend_from_slice(&0u32.to_le_bytes()); // timestamp
        dep_cmd.extend_from_slice(&0u32.to_le_bytes()); // current_version
        dep_cmd.extend_from_slice(&0u32.to_le_bytes()); // compat_version
        dep_cmd.extend_from_slice(&dep_bytes);
        dep_cmd.resize(dep_len, 0);

        let sizeofcmds = u32::try_from(rpath_cmd.len() + dep_cmd.len()).unwrap_or(0);
        let mut out = Vec::new();
        out.extend_from_slice(&MH_CIGAM_64.to_be_bytes()); // 0xcffaedfe (LE 64-bit magic)
        out.extend_from_slice(&0u32.to_le_bytes()); // cputype
        out.extend_from_slice(&0u32.to_le_bytes()); // cpusubtype
        out.extend_from_slice(&0x2u32.to_le_bytes()); // MH_EXECUTE
        out.extend_from_slice(&2u32.to_le_bytes()); // ncmds
        out.extend_from_slice(&sizeofcmds.to_le_bytes()); // sizeofcmds
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&rpath_cmd);
        out.extend_from_slice(&dep_cmd);
        out
    }

    #[test]
    fn test_inspect_macho_bytes_extracts_rpath_and_dylibs() {
        let bytes = build_synthetic_macho64("@loader_path/../lib", "/opt/custom/libfoo.dylib");
        let parsed = inspect_macho_bytes(&bytes).expect("synthetic Mach-O must parse");
        assert!(!parsed.is_dylib);
        assert_eq!(parsed.rpaths, vec!["@loader_path/../lib"]);
        assert_eq!(parsed.dylib_deps, vec!["/opt/custom/libfoo.dylib"]);
    }

    #[test]
    fn test_inspect_macho_load_info_reads_fat_slices_beyond_4mb() {
        use std::io::Write;
        let slice_bytes = build_synthetic_macho64(
            "@loader_path/../lib",
            "/Library/Frameworks/Python.framework/Versions/3.12/Python",
        );
        let slice_offset: u32 = 5 * 1024 * 1024; // > MAX_MACH_O_INSPECT_BYTES (4 MiB)
        let slice_size = u32::try_from(slice_bytes.len()).unwrap_or(0);

        let mut tmp = tempfile::NamedTempFile::new().expect("create temp fat binary");
        // FAT_MAGIC header (big-endian): magic + nfat=1 + 20-byte arch descriptor
        tmp.write_all(&FAT_MAGIC.to_be_bytes()).unwrap();
        tmp.write_all(&1u32.to_be_bytes()).unwrap();
        tmp.write_all(&0x0100_000cu32.to_be_bytes()).unwrap(); // CPU_TYPE_ARM64
        tmp.write_all(&0u32.to_be_bytes()).unwrap();
        tmp.write_all(&slice_offset.to_be_bytes()).unwrap();
        tmp.write_all(&slice_size.to_be_bytes()).unwrap();
        tmp.write_all(&14u32.to_be_bytes()).unwrap();

        tmp.seek(SeekFrom::Start(u64::from(slice_offset))).unwrap();
        tmp.write_all(&slice_bytes).unwrap();
        tmp.flush().unwrap();

        let parsed = inspect_macho_load_info(tmp.path()).expect("must parse >4MiB fat slice");
        assert_eq!(
            parsed.dylib_deps,
            vec!["/Library/Frameworks/Python.framework/Versions/3.12/Python"]
        );
    }

    #[test]
    fn test_inspect_fat64_and_patch_dylib_in_place() {
        use std::io::Write;
        let old_dep = "/Library/Frameworks/Python.framework/Versions/3.12/Python";
        let new_dep = "@loader_path/../lib/Python";
        let slice_bytes = build_synthetic_macho64("@loader_path/../lib", old_dep);
        let slice_off: u64 = 4096;
        let slice_sz = u64::try_from(slice_bytes.len()).unwrap_or(0);

        let mut tmp = tempfile::NamedTempFile::new().expect("create temp fat64");
        tmp.write_all(&FAT_MAGIC_64.to_be_bytes()).unwrap();
        tmp.write_all(&1u32.to_be_bytes()).unwrap();
        tmp.write_all(&0x0100_000cu32.to_be_bytes()).unwrap(); // CPU_TYPE_ARM64
        tmp.write_all(&0u32.to_be_bytes()).unwrap();
        tmp.write_all(&slice_off.to_be_bytes()).unwrap();
        tmp.write_all(&slice_sz.to_be_bytes()).unwrap();
        tmp.write_all(&14u32.to_be_bytes()).unwrap();
        tmp.write_all(&0u32.to_be_bytes()).unwrap();
        tmp.seek(SeekFrom::Start(slice_off)).unwrap();
        tmp.write_all(&slice_bytes).unwrap();
        tmp.flush().unwrap();

        let modified = patch_macho_dylib_paths_in_place(tmp.path(), &[(old_dep, new_dep)], None)
            .expect("in-place patch must succeed");
        assert!(modified);

        let after = inspect_macho_load_info(tmp.path()).expect("fat64 must parse after patch");
        assert_eq!(after.dylib_deps, vec![new_dep]);

        // Replacement exceeding existing slot capacity must leave binary unchanged.
        let too_long =
            "/very/long/replacement/path/that/exceeds/the/existing/load/command/slot/Python";
        let unchanged = patch_macho_dylib_paths_in_place(tmp.path(), &[(new_dep, too_long)], None)
            .expect("oversized patch skips cleanly");
        assert!(!unchanged);
    }
}
