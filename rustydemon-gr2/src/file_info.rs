//! Parser for the file-info struct that lives at offset 32 (right
//! after the 32-byte header).
//!
//! ```text
//!   +0   i32  format_version        (6 or 7)
//!   +4   u32  total_size            (bytes — must equal file size)
//!   +8   u32  crc32
//!   +12  u32  file_info_size        (72 for LE64 f7, 56 for LE32 f6)
//!   +16  u32  sector_count
//!   +20  u32  type_section          ┐ type_ref: Reference
//!   +24  u32  type_position         ┘
//!   +28  u32  root_section          ┐ root_ref: Reference
//!   +32  u32  root_position         ┘
//!   +36  u32  tag
//!   +40  …    reserved — padded up to file_info_size
//! ```
//!
//! The total_size field is our hard validation anchor — it matches the
//! on-disk file size exactly on every D2R sample I've looked at, so
//! every test harness should check it first.

use crate::error::{GrannyError, Result};
use crate::header::{read_i32, read_u32, Endian};
use crate::reference::Reference;

/// Parsed file-info block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    pub format_version: i32,
    pub total_size: u32,
    pub crc32: u32,
    pub file_info_size: u32,
    pub sector_count: u32,
    pub type_ref: Reference,
    pub root_ref: Reference,
    pub tag: u32,
}

/// Size of the fields we actually parse, before padding.
const BASE_SIZE: usize = 40;

/// Decode the file-info block.  Returns the parsed struct plus the
/// offset of the byte immediately after the padded block (i.e. where
/// the first sector header starts).
pub fn parse_file_info(data: &[u8], off: usize, endian: Endian) -> Result<(FileInfo, usize)> {
    check_range(data, off, BASE_SIZE)?;
    let s = &data[off..];

    let format_version = read_i32(&s[0..4], endian);
    let total_size = read_u32(&s[4..8], endian);
    let crc32 = read_u32(&s[8..12], endian);
    let file_info_size = read_u32(&s[12..16], endian);
    let sector_count = read_u32(&s[16..20], endian);
    let type_ref = Reference {
        sector: read_u32(&s[20..24], endian),
        position: read_u32(&s[24..28], endian),
    };
    let root_ref = Reference {
        sector: read_u32(&s[28..32], endian),
        position: read_u32(&s[32..36], endian),
    };
    let tag = read_u32(&s[36..40], endian);

    if file_info_size < BASE_SIZE as u32 {
        return Err(GrannyError::OutOfRange {
            start: off,
            end: off + BASE_SIZE,
            have: data.len(),
        });
    }

    let next = off + file_info_size as usize;
    check_range(data, 0, next)?;

    Ok((
        FileInfo {
            format_version,
            total_size,
            crc32,
            file_info_size,
            sector_count,
            type_ref,
            root_ref,
            tag,
        },
        next,
    ))
}

fn check_range(data: &[u8], start: usize, len: usize) -> Result<()> {
    let end = start.checked_add(len).ok_or(GrannyError::OutOfRange {
        start,
        end: usize::MAX,
        have: data.len(),
    })?;
    if end > data.len() {
        return Err(GrannyError::OutOfRange {
            start,
            end,
            have: data.len(),
        });
    }
    Ok(())
}
