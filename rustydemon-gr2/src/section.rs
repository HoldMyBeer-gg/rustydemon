//! Sector header parsing + decompression + fixup-table application.
//!
//! A Granny file is a header + N sector descriptors + N payloads.
//! Each sector payload is independently compressed (`compression_type`
//! 0 = raw, 4 = Bitknit2 — the cases D2R uses) and carries a fixup
//! table at `fixup_offset..fixup_offset+fixup_size*12` in the *file*
//! (not the sector).  Each 12-byte fixup entry relocates a pointer
//! from one (sector, position) to another (sector, position).
//!
//! For our preview use case we don't actually need to rewrite the
//! decompressed bytes — opengr2 stashes the fixup entries in a side
//! table and the element parser looks up pointers by (sector, offset)
//! via [`Section::resolve_pointer`].  We take the same approach so
//! the element parser port is mechanical.

use crate::error::{GrannyError, Result};
use crate::header::{read_u32, Endian};

/// Compression type enum values used in the on-disk sector header.
pub mod compression {
    pub const NONE: u32 = 0;
    #[allow(dead_code)]
    pub const OODLE0: u32 = 1;
    #[allow(dead_code)]
    pub const OODLE1: u32 = 2;
    #[allow(dead_code)]
    pub const BITKNIT1: u32 = 3;
    pub const BITKNIT2: u32 = 4;
}

/// On-disk sector descriptor.  44 bytes wide, one per sector in the
/// file, laid out immediately after the (padded) file-info block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionInfo {
    pub compression_type: u32,
    pub data_offset: u32,
    pub compressed_length: u32,
    pub decompressed_length: u32,
    pub alignment: u32,
    /// Byte offset inside the *compressed* sector where the first
    /// Bitknit chunk ends.  Only used by Bitknit-compressed sectors;
    /// we decode the whole sector as one stream so we don't consult
    /// it, but we keep it in the struct so a debug dump can show it.
    pub oodle_stop_0: u32,
    pub oodle_stop_1: u32,
    pub fixup_offset: u32,
    pub fixup_size: u32,
    pub marshall_offset: u32,
    pub marshall_size: u32,
}

pub const SECTION_INFO_SIZE: usize = 44;

/// Parse a single SectionInfo out of `data[off..off+44]`.  Returns the
/// parsed struct and the offset of the next sector header.
pub fn parse_section_info(data: &[u8], off: usize, endian: Endian) -> Result<(SectionInfo, usize)> {
    if off + SECTION_INFO_SIZE > data.len() {
        return Err(GrannyError::OutOfRange {
            start: off,
            end: off + SECTION_INFO_SIZE,
            have: data.len(),
        });
    }
    let s = &data[off..off + SECTION_INFO_SIZE];
    let u = |i: usize| read_u32(&s[i..i + 4], endian);
    let info = SectionInfo {
        compression_type: u(0),
        data_offset: u(4),
        compressed_length: u(8),
        decompressed_length: u(12),
        alignment: u(16),
        oodle_stop_0: u(20),
        oodle_stop_1: u(24),
        fixup_offset: u(28),
        fixup_size: u(32),
        marshall_offset: u(36),
        marshall_size: u(40),
    };
    Ok((info, off + SECTION_INFO_SIZE))
}

/// One fully loaded sector: decompressed payload plus its relocated
/// pointer table.
#[derive(Debug)]
pub struct Section {
    pub info: SectionInfo,
    pub data: Vec<u8>,
    pub pointer_table: Vec<PointerFixup>,
}

/// 12-byte entry in the per-sector fixup table.  Reads as three u32s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerFixup {
    /// Byte offset inside *this* sector's decompressed data where the
    /// pointer lives.
    pub src_offset: u32,
    /// Destination sector index.
    pub dst_sector: u32,
    /// Byte offset inside the destination sector's decompressed data
    /// where the pointer should resolve to.
    pub dst_offset: u32,
}

pub const POINTER_FIXUP_SIZE: usize = 12;

/// Load a single sector: decompress the payload and parse its fixup
/// table.  The file bytes are borrowed for decompression but the
/// decompressed output is owned by the returned [`Section`].
pub fn load_section(file: &[u8], info: SectionInfo, endian: Endian) -> Result<Section> {
    let data_end = (info.data_offset as u64 + info.compressed_length as u64) as usize;
    if data_end > file.len() {
        return Err(GrannyError::OutOfRange {
            start: info.data_offset as usize,
            end: data_end,
            have: file.len(),
        });
    }
    let compressed = &file[info.data_offset as usize..data_end];

    // Empty sectors (compressed_length == 0) are legitimate — Granny
    // reserves sector slots for asset classes it didn't use (animations,
    // extra skeletons, etc.) and we should treat them as a zero-byte
    // decompressed payload regardless of compression type.
    let data = if info.compressed_length == 0 {
        Vec::new()
    } else {
        match info.compression_type {
            compression::NONE => compressed.to_vec(),
            compression::BITKNIT2 => {
                crate::bitknit::decode_sector(compressed, info.decompressed_length as usize)?
            }
            other => return Err(GrannyError::UnsupportedCompression(other)),
        }
    };

    // Parse the fixup (pointer relocation) table.
    //
    // For Bitknit2-compressed sectors, the fixup table is itself
    // Bitknit2-compressed at `fixup_offset`: the layout is
    //
    //   u32 compressed_size
    //   compressed_size bytes of Bitknit2 payload
    //
    // which decompresses to exactly `fixup_size * 12` bytes of raw
    // (src_offset, dst_sector, dst_offset) triples.  Uncompressed
    // sectors store the triples directly.
    let pointer_table = if info.fixup_size == 0 {
        Vec::new()
    } else if info.compression_type == compression::BITKNIT2 {
        let fixup_bytes = info.fixup_size as usize * POINTER_FIXUP_SIZE;
        let header_start = info.fixup_offset as usize;
        if header_start + 4 > file.len() {
            return Err(GrannyError::OutOfRange {
                start: header_start,
                end: header_start + 4,
                have: file.len(),
            });
        }
        let compressed_size = read_u32(&file[header_start..header_start + 4], endian) as usize;
        let comp_start = header_start + 4;
        let comp_end = comp_start + compressed_size;
        if comp_end > file.len() {
            return Err(GrannyError::OutOfRange {
                start: comp_start,
                end: comp_end,
                have: file.len(),
            });
        }
        let decoded = crate::bitknit::decode_sector(&file[comp_start..comp_end], fixup_bytes)?;
        parse_fixup_blob(&decoded, info.fixup_size as usize, endian)?
    } else {
        let fixup_bytes = info.fixup_size as usize * POINTER_FIXUP_SIZE;
        let start = info.fixup_offset as usize;
        let end = start + fixup_bytes;
        if end > file.len() {
            return Err(GrannyError::OutOfRange {
                start,
                end,
                have: file.len(),
            });
        }
        parse_fixup_blob(&file[start..end], info.fixup_size as usize, endian)?
    };

    Ok(Section {
        info,
        data,
        pointer_table,
    })
}

/// Parse an already-decompressed (or uncompressed-in-place) fixup
/// blob into a vector of [`PointerFixup`] entries.
fn parse_fixup_blob(blob: &[u8], count: usize, endian: Endian) -> Result<Vec<PointerFixup>> {
    if blob.len() < count * POINTER_FIXUP_SIZE {
        return Err(GrannyError::OutOfRange {
            start: 0,
            end: count * POINTER_FIXUP_SIZE,
            have: blob.len(),
        });
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let base = i * POINTER_FIXUP_SIZE;
        let u = |j: usize| read_u32(&blob[base + j..base + j + 4], endian);
        out.push(PointerFixup {
            src_offset: u(0),
            dst_sector: u(4),
            dst_offset: u(8),
        });
    }
    Ok(out)
}

impl Section {
    /// Find the pointer that was stored at byte offset `src` in this
    /// sector's decompressed data, if the fixup table carries it.
    pub fn resolve_pointer(&self, src: usize) -> Option<PointerFixup> {
        self.pointer_table
            .iter()
            .copied()
            .find(|p| p.src_offset as usize == src)
    }
}
