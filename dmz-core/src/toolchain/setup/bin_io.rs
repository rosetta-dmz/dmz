// Shared bounded endian-aware binary readers for Mach-O and ELF parsers.

#[derive(Clone, Copy)]
pub(super) enum Endian {
    Little,
    Big,
}

pub(super) fn read_u16(bytes: &[u8], off: usize, endian: Endian) -> Option<u16> {
    let slice = bytes.get(off..off.checked_add(2)?)?;
    let arr: [u8; 2] = slice.try_into().ok()?;
    Some(match endian {
        Endian::Little => u16::from_le_bytes(arr),
        Endian::Big => u16::from_be_bytes(arr),
    })
}

pub(super) fn read_u32(bytes: &[u8], off: usize, endian: Endian) -> Option<u32> {
    let slice = bytes.get(off..off.checked_add(4)?)?;
    let arr: [u8; 4] = slice.try_into().ok()?;
    Some(match endian {
        Endian::Little => u32::from_le_bytes(arr),
        Endian::Big => u32::from_be_bytes(arr),
    })
}

pub(super) fn read_u64(bytes: &[u8], off: usize, endian: Endian) -> Option<u64> {
    let slice = bytes.get(off..off.checked_add(8)?)?;
    let arr: [u8; 8] = slice.try_into().ok()?;
    Some(match endian {
        Endian::Little => u64::from_le_bytes(arr),
        Endian::Big => u64::from_be_bytes(arr),
    })
}

pub(super) fn read_cstr_bounded(bytes: &[u8], offset: usize, max_end: usize) -> Option<String> {
    if offset >= max_end || max_end > bytes.len() {
        return None;
    }
    let region = bytes.get(offset..max_end)?;
    let nul_pos = region.iter().position(|&b| b == 0).unwrap_or(region.len());
    let s = std::str::from_utf8(region.get(..nul_pos)?).ok()?;
    Some(s.to_string())
}
