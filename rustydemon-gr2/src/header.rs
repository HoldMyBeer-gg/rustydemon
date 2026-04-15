//! The fixed 32-byte Granny3D file header.
//!
//! Layout:
//!
//! ```text
//!   0x00  16 bytes  magic GUID (identifies endianness / pointer size / format)
//!   0x10   4 bytes  size field (total of header + 16-byte extra region)
//!   0x14   4 bytes  format version inside the magic family
//!   0x18   8 bytes  reserved / zero
//! ```
//!
//! The magic GUID is the real discriminator — there is no ASCII magic.
//! Six variants are known in the wild:
//!
//! | Variant                    | Endian | Ptr size | File format |
//! |----------------------------|--------|----------|-------------|
//! | Little-endian 32-bit fmt 6 | LE     | 32-bit   | 6           |
//! | Big-endian 32-bit fmt 6    | BE     | 32-bit   | 6           |
//! | Little-endian 32-bit fmt 7 | LE     | 32-bit   | 7           |
//! | Little-endian 64-bit fmt 7 | LE     | 64-bit   | 7 *(D2R)*   |
//! | Big-endian 32-bit fmt 7    | BE     | 32-bit   | 7           |
//! | Big-endian 64-bit fmt 7    | BE     | 64-bit   | 7           |
//!
//! D2R `.model` files are always `LittleEndian64v1` (file format 7), which
//! is the variant we care about most and the one Rusty Demon actually
//! stress-tests against.  The others are handled for completeness so a
//! future WoW or Overwatch drop-in can reuse the same crate without a
//! second round of reverse engineering.

use crate::error::{GrannyError, Result};

/// Endianness of all multi-byte fields in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

/// Parsed 32-byte header.  All the downstream parsers take the
/// `endian` and `bits_64` out of this struct.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub endian: Endian,
    /// True for 64-bit-pointer variants.  Affects pointer-sized reads
    /// inside elements (the file has separate magic for 32/64 but the
    /// same section table layout).
    pub bits_64: bool,
    /// Set on the "extra 16" variants where the file_info block is
    /// padded out to a multiple of 16 bytes.  Derived from the magic
    /// family but kept here for the parser's convenience.
    pub extra_16: bool,
    /// Sum of the on-disk header + the 16-byte padding region that
    /// follows it.  Always 456 for LE64 file format 7 (D2R) and 440
    /// for LE32 file format 6.
    pub size: u32,
    /// Granny file format — 6 or 7.  D2R uses 7.
    pub format: u32,
}

const MAGIC_LE32_F6: [u8; 16] = [
    0xB8, 0x67, 0xB0, 0xCA, 0xF8, 0x6D, 0xB1, 0x0F, 0x84, 0x72, 0x8C, 0x7E, 0x5E, 0x19, 0x00, 0x1E,
];
const MAGIC_BE32_F6: [u8; 16] = [
    0xCA, 0xB0, 0x67, 0xB6, 0x0F, 0xB1, 0xDB, 0xF8, 0x7E, 0x8C, 0x72, 0x84, 0x1E, 0x00, 0x19, 0x5E,
];
const MAGIC_LE32_F7: [u8; 16] = [
    0x29, 0xDE, 0x6C, 0xC0, 0xBA, 0xA4, 0x53, 0x2B, 0x25, 0xF5, 0xB7, 0xA5, 0xF6, 0x66, 0xE2, 0xEE,
];
/// D2R: Little-endian, 64-bit pointers, Granny 2.9 file format 7.
pub const MAGIC_LE64_F7: [u8; 16] = [
    0xE5, 0x9B, 0x49, 0x5E, 0x6F, 0x63, 0x1F, 0x14, 0x1E, 0x13, 0xEB, 0xA9, 0x90, 0xBE, 0xED, 0xC4,
];
const MAGIC_BE32_F7: [u8; 16] = [
    0xB5, 0x95, 0x11, 0x0E, 0x4B, 0xB5, 0xA5, 0x6A, 0x50, 0x28, 0x28, 0xEB, 0x04, 0xB3, 0x78, 0x25,
];
const MAGIC_BE64_F7: [u8; 16] = [
    0xE3, 0xD4, 0x95, 0x31, 0x62, 0x4F, 0xDC, 0x20, 0x3A, 0xD0, 0x36, 0xCC, 0x89, 0xFF, 0x82, 0xB1,
];

/// Check whether `data` starts with a known Granny magic.  Cheap — used
/// by the preview plugin's `can_preview` to claim `.model` files.
pub fn has_granny_magic(data: &[u8]) -> bool {
    if data.len() < 16 {
        return false;
    }
    let m = &data[..16];
    m == MAGIC_LE32_F6
        || m == MAGIC_BE32_F6
        || m == MAGIC_LE32_F7
        || m == MAGIC_LE64_F7
        || m == MAGIC_BE32_F7
        || m == MAGIC_BE64_F7
}

/// Decode the 32-byte header.  Returns the parsed struct plus the
/// offset where the file-info block starts (always 32).
pub fn parse_header(data: &[u8]) -> Result<(Header, usize)> {
    if data.len() < 32 {
        return Err(GrannyError::TooShort(data.len(), 32));
    }
    let magic: [u8; 16] = data[..16].try_into().unwrap();

    let (endian, bits_64, extra_16) = if magic == MAGIC_LE32_F6 {
        (Endian::Little, false, false)
    } else if magic == MAGIC_BE32_F6 {
        (Endian::Big, false, false)
    } else if magic == MAGIC_LE32_F7 {
        (Endian::Little, false, true)
    } else if magic == MAGIC_LE64_F7 {
        (Endian::Little, true, true)
    } else if magic == MAGIC_BE32_F7 {
        (Endian::Big, false, true)
    } else if magic == MAGIC_BE64_F7 {
        (Endian::Big, true, true)
    } else {
        return Err(GrannyError::BadMagic);
    };

    let size = read_u32(&data[16..20], endian);
    let format = read_u32(&data[20..24], endian);
    // Bytes 24..32 are reserved; we don't care what's there.

    Ok((
        Header {
            endian,
            bits_64,
            extra_16,
            size,
            format,
        },
        32,
    ))
}

/// Read a little/big-endian u32 from a 4-byte slice.  Panics on the
/// wrong length, which is a programmer error, not a file error.
#[inline]
pub fn read_u32(b: &[u8], endian: Endian) -> u32 {
    let arr: [u8; 4] = b.try_into().expect("slice must be 4 bytes");
    match endian {
        Endian::Little => u32::from_le_bytes(arr),
        Endian::Big => u32::from_be_bytes(arr),
    }
}

/// Read a little/big-endian u64 from an 8-byte slice.
#[inline]
pub fn read_u64(b: &[u8], endian: Endian) -> u64 {
    let arr: [u8; 8] = b.try_into().expect("slice must be 8 bytes");
    match endian {
        Endian::Little => u64::from_le_bytes(arr),
        Endian::Big => u64::from_be_bytes(arr),
    }
}

/// Read an i32 from a 4-byte slice.
#[inline]
pub fn read_i32(b: &[u8], endian: Endian) -> i32 {
    read_u32(b, endian) as i32
}

/// Read an f32 from a 4-byte slice.
#[inline]
pub fn read_f32(b: &[u8], endian: Endian) -> f32 {
    f32::from_bits(read_u32(b, endian))
}

/// Read a pointer-sized unsigned integer (32 or 64 bits depending on
/// the header variant).  Granny stores both widths little-/big-endian.
#[inline]
pub fn read_usize(b: &[u8], endian: Endian, bits_64: bool) -> u64 {
    if bits_64 {
        read_u64(b, endian)
    } else {
        read_u32(b, endian) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le64_f7_header_parses() {
        let mut buf = [0u8; 64];
        buf[..16].copy_from_slice(&MAGIC_LE64_F7);
        buf[16..20].copy_from_slice(&456u32.to_le_bytes());
        buf[20..24].copy_from_slice(&7u32.to_le_bytes());
        let (h, off) = parse_header(&buf).unwrap();
        assert_eq!(h.endian, Endian::Little);
        assert!(h.bits_64);
        assert!(h.extra_16);
        assert_eq!(h.size, 456);
        assert_eq!(h.format, 7);
        assert_eq!(off, 32);
    }

    #[test]
    fn bad_magic_rejected() {
        let buf = [0u8; 32];
        assert!(matches!(parse_header(&buf), Err(GrannyError::BadMagic)));
    }

    #[test]
    fn short_input_rejected() {
        let buf = [0u8; 10];
        assert!(matches!(
            parse_header(&buf),
            Err(GrannyError::TooShort(10, 32))
        ));
    }
}
