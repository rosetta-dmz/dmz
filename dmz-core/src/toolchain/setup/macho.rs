// Pure-Rust bounded Mach-O header and load-command inspector (zero subprocess / zero unsafe).

use std::fs::File;
use std::io::Read;
use std::path::Path;

const MH_MAGIC: u32 = 0xfeed_face;
const MH_CIGAM: u32 = 0xcefa_edfe;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;

const MH_DYLIB: u32 = 0x6;
const LC_REQ_DYLD: u32 = 0x8000_0000;
const LC_LOAD_DYLIB: u32 = 0xc;
const LC_ID_DYLIB: u32 = 0xd;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
const LC_RPATH: u32 = 0x1c | LC_REQ_DYLD;
const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;

const MAX_MACH_O_INSPECT_BYTES: usize = 4 * 1024 * 1024;
const MAX_LOAD_COMMANDS: u32 = 4096;

/// Parsed Mach-O load-command metadata needed for bundle relocation checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachOLoadInfo {
    pub is_dylib: bool,
    pub rpaths: Vec<String>,
    pub dylib_deps: Vec<String>,
    pub install_id: Option<String>,
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

fn read_u32(bytes: &[u8], offset: usize, endian: Endian) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    let arr: [u8; 4] = slice.try_into().ok()?;
    Some(match endian {
        Endian::Little => u32::from_le_bytes(arr),
        Endian::Big => u32::from_be_bytes(arr),
    })
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    read_u32(bytes, offset, Endian::Big)
}

fn read_cstr(bytes: &[u8], offset: usize, max_end: usize) -> Option<String> {
    if offset >= max_end || max_end > bytes.len() {
        return None;
    }
    let region = bytes.get(offset..max_end)?;
    let nul_pos = region.iter().position(|&b| b == 0).unwrap_or(region.len());
    let s = std::str::from_utf8(region.get(..nul_pos)?).ok()?;
    Some(s.to_string())
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
        let cmdsize = read_u32(slice, cursor + 4, endian)? as usize;
        if cmdsize < 8 {
            break;
        }
        let cmd_end = cursor.saturating_add(cmdsize).min(cmds_end);

        match cmd {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_ID_DYLIB => {
                if let Some(str_off) = read_u32(slice, cursor + 8, endian) {
                    if let Some(val) =
                        read_cstr(slice, cursor.saturating_add(str_off as usize), cmd_end)
                    {
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
                if let Some(str_off) = read_u32(slice, cursor + 8, endian) {
                    if let Some(val) =
                        read_cstr(slice, cursor.saturating_add(str_off as usize), cmd_end)
                    {
                        if !out.rpaths.contains(&val) {
                            out.rpaths.push(val);
                        }
                    }
                }
            }
            _ => {}
        }

        cursor = cmd_end;
    }

    Some(())
}

/// Parses Mach-O headers and load commands from `path` without spawning external `otool` processes.
pub fn inspect_macho_load_info(path: &Path) -> Option<MachOLoadInfo> {
    let file = File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(MAX_MACH_O_INSPECT_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    inspect_macho_bytes(&buf)
}

/// Parses Mach-O headers and load commands from an in-memory buffer.
pub fn inspect_macho_bytes(buf: &[u8]) -> Option<MachOLoadInfo> {
    let magic_be = read_u32_be(buf, 0)?;
    let mut info = MachOLoadInfo::default();

    if magic_be == FAT_MAGIC || magic_be == FAT_CIGAM {
        // Fat/Universal header uses big-endian counts (magic 0xcafebabe) or little-endian (0xbebafeca).
        let fat_endian = if magic_be == FAT_MAGIC {
            Endian::Big
        } else {
            Endian::Little
        };
        let nfat = read_u32(buf, 4, fat_endian)?.min(16);
        let mut parsed_any = false;
        for idx in 0..nfat {
            let arch_off = 8usize.checked_add((idx as usize).checked_mul(20)?)?;
            let slice_off = read_u32(buf, arch_off.checked_add(8)?, fat_endian)? as usize;
            let slice_size = read_u32(buf, arch_off.checked_add(12)?, fat_endian)? as usize;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_synthetic_macho64(rpath: &str, dep: &str) -> Vec<u8> {
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
}
