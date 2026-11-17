// Pure-Rust bounded ELF header and dynamic-section inspector (zero subprocess / zero unsafe).

use super::bin_io::{read_cstr_bounded, read_u16, read_u32, read_u64, Endian};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ELFDATA2MSB: u8 = 2;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;

const SHT_STRTAB: u32 = 3;
const SHT_DYNAMIC: u32 = 6;

const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_RPATH: u64 = 15;
const DT_RUNPATH: u64 = 29;

const MAX_PHDRS: usize = 128;
const MAX_SHDRS: usize = 512;
const MAX_SECTION_READ: usize = 512 * 1024;
const MAX_STRTAB_READ: usize = 4 * 1024 * 1024;
const MAX_DYN_ENTRIES: usize = 4096;

/// Parsed ELF metadata needed for Linux `$ORIGIN` relocation and dependency discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElfLoadInfo {
    pub is_dynamic_binary: bool,
    pub needed: Vec<String>,
    pub rpaths: Vec<String>,
    pub has_dt_rpath: bool,
}

#[derive(Clone, Copy)]
enum ElfClass {
    Elf32,
    Elf64,
}

#[derive(Clone, Copy)]
struct DynRange {
    offset: u64,
    size: usize,
    strtab_offset: u64,
    strtab_size: usize,
}

fn read_addr(bytes: &[u8], off: usize, class: ElfClass, endian: Endian) -> Option<u64> {
    match class {
        ElfClass::Elf32 => read_u32(bytes, off, endian).map(u64::from),
        ElfClass::Elf64 => read_u64(bytes, off, endian),
    }
}

fn read_cstr(bytes: &[u8], off: usize) -> Option<String> {
    read_cstr_bounded(bytes, off, bytes.len())
}

fn read_bounded_at(file: &mut File, offset: u64, size: usize, cap: usize) -> Option<Vec<u8>> {
    if size == 0 {
        return None;
    }
    let to_read = size.min(cap);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::with_capacity(to_read);
    file.take(to_read as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn find_dyn_via_sections(
    file: &mut File,
    class: ElfClass,
    endian: Endian,
    e_shoff: u64,
    e_shentsize: usize,
    e_shnum: usize,
) -> Option<DynRange> {
    let min_entry = match class {
        ElfClass::Elf32 => 40,
        ElfClass::Elf64 => 64,
    };
    if e_shoff == 0 || e_shnum == 0 || e_shentsize < min_entry {
        return None;
    }
    let count = e_shnum.min(MAX_SHDRS);
    let table_bytes = count.checked_mul(e_shentsize)?;
    let shdrs = read_bounded_at(file, e_shoff, table_bytes, MAX_SECTION_READ)?;

    for idx in 0..count {
        let base = idx.checked_mul(e_shentsize)?;
        let sh_type = read_u32(&shdrs, base.checked_add(4)?, endian)?;
        if sh_type != SHT_DYNAMIC {
            continue;
        }
        let (off_fld, sz_fld, link_fld) = match class {
            ElfClass::Elf32 => (16usize, 20usize, 24usize),
            ElfClass::Elf64 => (24usize, 32usize, 40usize),
        };
        let dyn_off = read_addr(&shdrs, base.checked_add(off_fld)?, class, endian)?;
        let dyn_sz = usize::try_from(read_addr(&shdrs, base.checked_add(sz_fld)?, class, endian)?)
            .unwrap_or(MAX_SECTION_READ);
        let str_idx =
            usize::try_from(read_u32(&shdrs, base.checked_add(link_fld)?, endian)?).ok()?;
        if str_idx >= count {
            return None;
        }
        let str_base = str_idx.checked_mul(e_shentsize)?;
        let str_type = read_u32(&shdrs, str_base.checked_add(4)?, endian)?;
        if str_type != SHT_STRTAB {
            return None;
        }
        let strtab_offset = read_addr(&shdrs, str_base.checked_add(off_fld)?, class, endian)?;
        let strtab_size = usize::try_from(read_addr(
            &shdrs,
            str_base.checked_add(sz_fld)?,
            class,
            endian,
        )?)
        .unwrap_or(MAX_STRTAB_READ);
        return Some(DynRange {
            offset: dyn_off,
            size: dyn_sz,
            strtab_offset,
            strtab_size,
        });
    }
    None
}

fn vaddr_to_offset(
    phdrs: &[u8],
    count: usize,
    entsize: usize,
    class: ElfClass,
    endian: Endian,
    vaddr: u64,
) -> Option<u64> {
    for idx in 0..count {
        let base = idx.checked_mul(entsize)?;
        if read_u32(phdrs, base, endian)? != PT_LOAD {
            continue;
        }
        let (off_fld, vaddr_fld, filesz_fld) = match class {
            ElfClass::Elf32 => (4usize, 8usize, 16usize),
            ElfClass::Elf64 => (8usize, 16usize, 32usize),
        };
        let p_offset = read_addr(phdrs, base.checked_add(off_fld)?, class, endian)?;
        let p_vaddr = read_addr(phdrs, base.checked_add(vaddr_fld)?, class, endian)?;
        let p_filesz = read_addr(phdrs, base.checked_add(filesz_fld)?, class, endian)?;
        if vaddr >= p_vaddr && vaddr < p_vaddr.saturating_add(p_filesz) {
            return p_offset.checked_add(vaddr.saturating_sub(p_vaddr));
        }
    }
    None
}

fn find_dyn_via_phdrs(
    file: &mut File,
    class: ElfClass,
    endian: Endian,
    e_phoff: u64,
    e_phentsize: usize,
    e_phnum: usize,
) -> Option<DynRange> {
    let min_entry = match class {
        ElfClass::Elf32 => 32,
        ElfClass::Elf64 => 56,
    };
    if e_phoff == 0 || e_phnum == 0 || e_phentsize < min_entry {
        return None;
    }
    let count = e_phnum.min(MAX_PHDRS);
    let phdrs = read_bounded_at(
        file,
        e_phoff,
        count.checked_mul(e_phentsize)?,
        MAX_SECTION_READ,
    )?;

    let (off_fld, filesz_fld) = match class {
        ElfClass::Elf32 => (4usize, 16usize),
        ElfClass::Elf64 => (8usize, 32usize),
    };

    for idx in 0..count {
        let base = idx.checked_mul(e_phentsize)?;
        if read_u32(&phdrs, base, endian)? != PT_DYNAMIC {
            continue;
        }
        let dyn_off = read_addr(&phdrs, base.checked_add(off_fld)?, class, endian)?;
        let dyn_sz = usize::try_from(read_addr(
            &phdrs,
            base.checked_add(filesz_fld)?,
            class,
            endian,
        )?)
        .unwrap_or(MAX_SECTION_READ);
        let dyn_bytes = read_bounded_at(file, dyn_off, dyn_sz, MAX_SECTION_READ)?;
        let dyn_stride = match class {
            ElfClass::Elf32 => 8usize,
            ElfClass::Elf64 => 16usize,
        };
        let val_off = dyn_stride / 2;
        for d_idx in 0..MAX_DYN_ENTRIES {
            let pos = d_idx.checked_mul(dyn_stride)?;
            if pos.checked_add(dyn_stride)? > dyn_bytes.len() {
                break;
            }
            let tag = read_addr(&dyn_bytes, pos, class, endian)?;
            if tag == DT_NULL {
                break;
            }
            if tag == DT_STRTAB {
                let str_va = read_addr(&dyn_bytes, pos.checked_add(val_off)?, class, endian)?;
                let strtab_offset =
                    vaddr_to_offset(&phdrs, count, e_phentsize, class, endian, str_va)?;
                return Some(DynRange {
                    offset: dyn_off,
                    size: dyn_sz,
                    strtab_offset,
                    strtab_size: MAX_STRTAB_READ,
                });
            }
        }
    }
    None
}

fn push_colon_paths(raw: &str, out: &mut Vec<String>) {
    for part in raw.split(':').filter(|s| !s.is_empty()) {
        if !out.iter().any(|existing| existing == part) {
            out.push(part.to_string());
        }
    }
}

/// Parses ELF header and dynamic entries from `path` without spawning `readelf` or `ldd`.
#[must_use]
pub fn inspect_elf_load_info(path: &Path) -> Option<ElfLoadInfo> {
    let mut file = File::open(path).ok()?;
    let mut hdr = [0u8; 64];
    file.read_exact(&mut hdr).ok()?;

    if hdr.get(..4)? != ELF_MAGIC {
        return None;
    }
    let class = match *hdr.get(4)? {
        ELFCLASS32 => ElfClass::Elf32,
        ELFCLASS64 => ElfClass::Elf64,
        _ => return None,
    };
    let endian = match *hdr.get(5)? {
        ELFDATA2LSB => Endian::Little,
        ELFDATA2MSB => Endian::Big,
        _ => return None,
    };

    let e_type = read_u16(&hdr, 16, endian)?;
    let is_dynamic_binary = matches!(e_type, ET_EXEC | ET_DYN);
    let mut info = ElfLoadInfo {
        is_dynamic_binary,
        ..ElfLoadInfo::default()
    };
    if !is_dynamic_binary {
        return Some(info);
    }

    let (phoff, shoff, phentsize, phnum, shentsize, shnum) = match class {
        ElfClass::Elf32 => (
            u64::from(read_u32(&hdr, 28, endian)?),
            u64::from(read_u32(&hdr, 32, endian)?),
            usize::from(read_u16(&hdr, 42, endian)?),
            usize::from(read_u16(&hdr, 44, endian)?),
            usize::from(read_u16(&hdr, 46, endian)?),
            usize::from(read_u16(&hdr, 48, endian)?),
        ),
        ElfClass::Elf64 => (
            read_u64(&hdr, 32, endian)?,
            read_u64(&hdr, 40, endian)?,
            usize::from(read_u16(&hdr, 54, endian)?),
            usize::from(read_u16(&hdr, 56, endian)?),
            usize::from(read_u16(&hdr, 58, endian)?),
            usize::from(read_u16(&hdr, 60, endian)?),
        ),
    };

    let range = find_dyn_via_sections(&mut file, class, endian, shoff, shentsize, shnum)
        .or_else(|| find_dyn_via_phdrs(&mut file, class, endian, phoff, phentsize, phnum));

    let Some(range) = range else {
        return Some(info);
    };
    let dyn_bytes = read_bounded_at(&mut file, range.offset, range.size, MAX_SECTION_READ)?;
    let strtab = read_bounded_at(
        &mut file,
        range.strtab_offset,
        range.strtab_size,
        MAX_STRTAB_READ,
    )?;

    let dyn_stride = match class {
        ElfClass::Elf32 => 8usize,
        ElfClass::Elf64 => 16usize,
    };
    let val_off = dyn_stride / 2;

    for idx in 0..MAX_DYN_ENTRIES {
        let pos = idx.checked_mul(dyn_stride)?;
        if pos.checked_add(dyn_stride)? > dyn_bytes.len() {
            break;
        }
        let tag = read_addr(&dyn_bytes, pos, class, endian)?;
        if tag == DT_NULL {
            break;
        }
        let val = read_addr(&dyn_bytes, pos.checked_add(val_off)?, class, endian)?;
        let Ok(str_off) = usize::try_from(val) else {
            continue;
        };
        match tag {
            DT_NEEDED => {
                if let Some(name) = read_cstr(&strtab, str_off) {
                    if !name.is_empty() && !info.needed.contains(&name) {
                        info.needed.push(name);
                    }
                }
            }
            DT_RPATH => {
                info.has_dt_rpath = true;
                if let Some(raw) = read_cstr(&strtab, str_off) {
                    push_colon_paths(&raw, &mut info.rpaths);
                }
            }
            DT_RUNPATH => {
                if let Some(raw) = read_cstr(&strtab, str_off) {
                    push_colon_paths(&raw, &mut info.rpaths);
                }
            }
            _ => {}
        }
    }

    Some(info)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) fn build_synthetic_elf64(
        e_type: u16,
        needed: &[&str],
        rpath: Option<&str>,
        is_runpath: bool,
    ) -> Vec<u8> {
        let mut strtab = vec![0u8];
        let mut entries: Vec<(u64, u64)> = Vec::new();
        for lib in needed {
            let off = strtab.len() as u64;
            strtab.extend_from_slice(lib.as_bytes());
            strtab.push(0);
            entries.push((DT_NEEDED, off));
        }
        if let Some(rp) = rpath {
            let off = strtab.len() as u64;
            strtab.extend_from_slice(rp.as_bytes());
            strtab.push(0);
            entries.push((if is_runpath { DT_RUNPATH } else { DT_RPATH }, off));
        }
        entries.push((DT_NULL, 0));

        let dyn_off = 64u64;
        let dyn_size = (entries.len() * 16) as u64;
        let str_off = dyn_off + dyn_size;
        let str_size = strtab.len() as u64;
        let shoff = str_off + str_size;

        let mut buf = Vec::new();
        buf.extend_from_slice(&ELF_MAGIC);
        buf.extend_from_slice(&[ELFCLASS64, ELFDATA2LSB, 1, 0, 0]);
        buf.resize(16, 0);
        buf.extend_from_slice(&e_type.to_le_bytes());
        buf.extend_from_slice(&62u16.to_le_bytes()); // x86_64
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes()); // e_entry
        buf.extend_from_slice(&0u64.to_le_bytes()); // e_phoff
        buf.extend_from_slice(&shoff.to_le_bytes()); // e_shoff
        buf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
        buf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
        buf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
        buf.extend_from_slice(&0u16.to_le_bytes()); // e_phnum
        buf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
        buf.extend_from_slice(&3u16.to_le_bytes()); // e_shnum
        buf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

        for (tag, val) in &entries {
            buf.extend_from_slice(&tag.to_le_bytes());
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.extend_from_slice(&strtab);

        // Section 0: NULL
        buf.resize(buf.len() + 64, 0);
        // Section 1: .dynamic (sh_link = 2)
        let mut sh1 = [0u8; 64];
        sh1[4..8].copy_from_slice(&SHT_DYNAMIC.to_le_bytes());
        sh1[24..32].copy_from_slice(&dyn_off.to_le_bytes());
        sh1[32..40].copy_from_slice(&dyn_size.to_le_bytes());
        sh1[40..44].copy_from_slice(&2u32.to_le_bytes());
        buf.extend_from_slice(&sh1);
        // Section 2: .dynstr
        let mut sh2 = [0u8; 64];
        sh2[4..8].copy_from_slice(&SHT_STRTAB.to_le_bytes());
        sh2[24..32].copy_from_slice(&str_off.to_le_bytes());
        sh2[32..40].copy_from_slice(&str_size.to_le_bytes());
        buf.extend_from_slice(&sh2);

        buf
    }

    #[test]
    fn test_inspect_elf64_extracts_needed_and_rpath() {
        let bytes = build_synthetic_elf64(
            ET_DYN,
            &["libpython3.12.so.1.0", "libssl.so.3"],
            Some("$ORIGIN:$ORIGIN/../lib"),
            false,
        );
        let mut tmp = tempfile::NamedTempFile::new().expect("temp elf");
        tmp.write_all(&bytes).unwrap();
        tmp.flush().unwrap();

        let parsed = inspect_elf_load_info(tmp.path()).expect("synthetic ELF must parse");
        assert!(parsed.is_dynamic_binary);
        assert!(parsed.has_dt_rpath);
        assert_eq!(parsed.needed, vec!["libpython3.12.so.1.0", "libssl.so.3"]);
        assert_eq!(parsed.rpaths, vec!["$ORIGIN", "$ORIGIN/../lib"]);
    }

    #[test]
    fn test_inspect_elf_marks_et_rel_as_non_dynamic() {
        let bytes = build_synthetic_elf64(1, &[], None, false);
        let mut tmp = tempfile::NamedTempFile::new().expect("temp elf obj");
        tmp.write_all(&bytes).unwrap();
        tmp.flush().unwrap();

        let parsed = inspect_elf_load_info(tmp.path()).expect("ET_REL header parses");
        assert!(!parsed.is_dynamic_binary);
    }
}
