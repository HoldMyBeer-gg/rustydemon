//! D4 `Texture-Base-Global.dat` parser.
//!
//! Steam D4 installs do not ship per-file `meta/Texture/*.tex` descriptors
//! that the public Noesis script (`d4Tex`) and similar tools expect. Instead
//! the texture metadata for ~140k textures lives in a single consolidated
//! file at `base/Texture-Base-Global.dat`. This module parses it and exposes
//! a SNO-id → [`TextureDescriptor`] lookup so the texture preview can decode
//! at exact dimensions and format instead of brute-forcing.
//!
//! Layout, derived from the d4-asset-extractor project's
//! `texture_extractor.py::_build_texture_index` and cross-checked against
//! d4data's `parse.js::parseCombinedMetaFile`:
//!
//! ```text
//! offset 0:  u32 LE magic = 0x44CF00F5
//! offset 4:  u32 LE entry_count
//! offset 8:  entry_count × { u32 sno_id, u32 def_size }   ← the index
//! then:      contiguous descriptor blobs, each entry's offset computed by
//!            walking the index from the start: align prev_end to 8 bytes,
//!            then add 8 bytes (texture-group convention).
//! ```
//!
//! Each descriptor blob's named fields (offsets relative to blob start):
//!
//! ```text
//! 0  : u32 sno_id              (validates the entry)
//! 12 : u32 eTexFormat          (D4 internal format enum)
//! 20 : u16 dwWidth             (logical pixel width)
//! 22 : u16 dwHeight            (logical pixel height)
//! 24 : u32 dwDepth             (1 for 2D textures)
//! 28 : u8  dwFaceCount         (1 for 2D, 6 for cubemap)
//! 29 : u8  dwMipMapLevelMin
//! 30 : u8  dwMipMapLevelMax    (paylow stores mips 1..max)
//! ```

use std::collections::HashMap;

use crate::error::CascError;

const TEXBASE_MAGIC: u32 = 0x44CF00F5;
const MIN_DESCRIPTOR_SIZE: usize = 31;

/// One texture's metadata read out of `Texture-Base-Global.dat`.
#[derive(Debug, Clone, Copy)]
pub struct TextureDescriptor {
    pub sno_id: i32,
    pub format_id: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u32,
    pub face_count: u8,
    pub mipmap_min: u8,
    pub mipmap_max: u8,
}

impl TextureDescriptor {
    /// Map the D4 internal format enum to a printable label and the bytes
    /// per 4×4 BC block (or `None` for uncompressed / unknown formats).
    pub fn block_compression(&self) -> Option<BlockFormat> {
        BlockFormat::from_d4_format(self.format_id)
    }
}

/// BC variants we currently know how to decode in the texture preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFormat {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc6h,
    Bc7,
}

impl BlockFormat {
    pub fn label(self) -> &'static str {
        match self {
            BlockFormat::Bc1 => "BC1",
            BlockFormat::Bc2 => "BC2",
            BlockFormat::Bc3 => "BC3",
            BlockFormat::Bc4 => "BC4",
            BlockFormat::Bc5 => "BC5",
            BlockFormat::Bc6h => "BC6H",
            BlockFormat::Bc7 => "BC7",
        }
    }

    /// Bytes per 4×4 block.
    pub fn bytes_per_block(self) -> usize {
        match self {
            BlockFormat::Bc1 | BlockFormat::Bc4 => 8,
            BlockFormat::Bc2
            | BlockFormat::Bc3
            | BlockFormat::Bc5
            | BlockFormat::Bc6h
            | BlockFormat::Bc7 => 16,
        }
    }

    /// Map D4's `eTexFormat` enum to a [`BlockFormat`]. Returns `None` for
    /// uncompressed formats (BGRA8, A8, RGBA16F) and anything we haven't
    /// catalogued — callers can fall back to the brute-force decoder.
    fn from_d4_format(fmt: u32) -> Option<Self> {
        Some(match fmt {
            9 | 10 | 46 | 47 => BlockFormat::Bc1,
            48 => BlockFormat::Bc2,
            12 | 49 => BlockFormat::Bc3,
            41 => BlockFormat::Bc4,
            42 => BlockFormat::Bc5,
            43 | 51 => BlockFormat::Bc6h,
            44 | 50 => BlockFormat::Bc7,
            _ => return None,
        })
    }
}

/// Eagerly-parsed Texture-Base-Global descriptor table. We hold ~140k
/// `TextureDescriptor` values (~32 bytes each) which weighs in at ~5 MB
/// resident — far smaller than the 34 MB source file, which is dropped
/// after parsing.
#[derive(Debug)]
pub struct TextureBaseIndex {
    descriptors: HashMap<i32, TextureDescriptor>,
}

impl TextureBaseIndex {
    /// Parse the full file. The input buffer is consumed and all descriptors
    /// are read into the map up front so callers don't need to keep the
    /// original 34 MB blob around.
    pub fn parse(data: &[u8]) -> Result<Self, CascError> {
        if data.len() < 8 {
            return Err(CascError::InvalidData(
                "Texture-Base-Global.dat: smaller than header".into(),
            ));
        }
        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        if magic != TEXBASE_MAGIC {
            return Err(CascError::InvalidData(format!(
                "Texture-Base-Global.dat: bad magic {magic:#010x}"
            )));
        }
        let entry_count = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let index_end = 8usize
            .checked_add(entry_count.checked_mul(8).ok_or_else(|| {
                CascError::InvalidData("Texture-Base-Global.dat: entry count overflow".into())
            })?)
            .ok_or_else(|| {
                CascError::InvalidData("Texture-Base-Global.dat: index extends past usize".into())
            })?;
        if index_end > data.len() {
            return Err(CascError::InvalidData(
                "Texture-Base-Global.dat: index extends past end of file".into(),
            ));
        }

        let mut descriptors: HashMap<i32, TextureDescriptor> = HashMap::with_capacity(entry_count);
        // Definition section starts immediately after the index table.
        // Each entry's actual offset = align_up(prev_end, 8) + 8 (the +8 is
        // a per-entry padding specific to texture-group entries; see d4data
        // parse.js parseCombinedMetaFile).
        let mut cursor: usize = index_end;
        for i in 0..entry_count {
            let idx_pos = 8 + i * 8;
            let sno_id = i32::from_le_bytes(data[idx_pos..idx_pos + 4].try_into().unwrap());
            let def_size =
                u32::from_le_bytes(data[idx_pos + 4..idx_pos + 8].try_into().unwrap()) as usize;
            let aligned = (cursor + 7) & !7;
            let actual = aligned + 8;
            let end = actual.checked_add(def_size).ok_or_else(|| {
                CascError::InvalidData("Texture-Base-Global.dat: descriptor offset overflow".into())
            })?;
            if end > data.len() {
                return Err(CascError::InvalidData(format!(
                    "Texture-Base-Global.dat: descriptor #{i} extends past file \
                     ({actual}+{def_size} > {})",
                    data.len()
                )));
            }
            cursor = end;
            if def_size < MIN_DESCRIPTOR_SIZE {
                // Tiny / placeholder entry — skip. The lookup just won't
                // return anything for this SNO and the preview falls back.
                continue;
            }
            let blob = &data[actual..end];
            let stored_sno = i32::from_le_bytes(blob[0..4].try_into().unwrap());
            if stored_sno != sno_id {
                // Index/data drift — bail rather than handing back a wrong
                // descriptor for some other SNO.
                return Err(CascError::InvalidData(format!(
                    "Texture-Base-Global.dat: descriptor at offset {actual} has SNO \
                     {stored_sno}, expected {sno_id}"
                )));
            }
            descriptors.insert(
                sno_id,
                TextureDescriptor {
                    sno_id,
                    format_id: u32::from_le_bytes(blob[12..16].try_into().unwrap()),
                    width: u16::from_le_bytes(blob[20..22].try_into().unwrap()),
                    height: u16::from_le_bytes(blob[22..24].try_into().unwrap()),
                    depth: u32::from_le_bytes(blob[24..28].try_into().unwrap()),
                    face_count: blob[28],
                    mipmap_min: blob[29],
                    mipmap_max: blob[30],
                },
            );
        }
        Ok(Self { descriptors })
    }

    pub fn get(&self, sno_id: i32) -> Option<&TextureDescriptor> {
        self.descriptors.get(&sno_id)
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic Texture-Base-Global.dat with two entries and verify
    /// the offset-walking math (align-to-8 + 8) hands back the right fields.
    #[test]
    fn parses_synthetic_two_entry_file() {
        let mut buf: Vec<u8> = Vec::new();
        // Magic + entry_count.
        buf.extend_from_slice(&TEXBASE_MAGIC.to_le_bytes());
        buf.extend_from_slice(&2u32.to_le_bytes());
        // Index: (sno=42, size=64), (sno=7, size=48)
        buf.extend_from_slice(&42i32.to_le_bytes());
        buf.extend_from_slice(&64u32.to_le_bytes());
        buf.extend_from_slice(&7i32.to_le_bytes());
        buf.extend_from_slice(&48u32.to_le_bytes());

        // Definitions section starts at offset 8 + 2*8 = 24.
        // First entry: align_up(24, 8) = 24, +8 = 32. Pad up to 32.
        buf.resize(32, 0);
        let blob1_start = buf.len();
        // Build a 64-byte descriptor for SNO 42, BC7 (44), 256×128.
        let mut blob1 = vec![0u8; 64];
        blob1[0..4].copy_from_slice(&42i32.to_le_bytes());
        blob1[12..16].copy_from_slice(&44u32.to_le_bytes()); // eTexFormat = BC7
        blob1[20..22].copy_from_slice(&256u16.to_le_bytes());
        blob1[22..24].copy_from_slice(&128u16.to_le_bytes());
        blob1[24..28].copy_from_slice(&1u32.to_le_bytes());
        blob1[28] = 1; // face_count
        blob1[29] = 0; // mipmap_min
        blob1[30] = 8; // mipmap_max
        buf.extend_from_slice(&blob1);
        assert_eq!(buf.len(), blob1_start + 64);

        // Second entry: prev_end = 32+64 = 96; align_up(96,8)=96; +8 = 104.
        buf.resize(104, 0);
        let mut blob2 = vec![0u8; 48];
        blob2[0..4].copy_from_slice(&7i32.to_le_bytes());
        blob2[12..16].copy_from_slice(&41u32.to_le_bytes()); // BC4
        blob2[20..22].copy_from_slice(&64u16.to_le_bytes());
        blob2[22..24].copy_from_slice(&64u16.to_le_bytes());
        blob2[28] = 1;
        blob2[30] = 6;
        buf.extend_from_slice(&blob2);

        let idx = TextureBaseIndex::parse(&buf).expect("parse should succeed");
        assert_eq!(idx.len(), 2);

        let d42 = idx.get(42).unwrap();
        assert_eq!(d42.format_id, 44);
        assert_eq!(d42.width, 256);
        assert_eq!(d42.height, 128);
        assert_eq!(d42.mipmap_max, 8);
        assert_eq!(d42.block_compression(), Some(BlockFormat::Bc7));

        let d7 = idx.get(7).unwrap();
        assert_eq!(d7.format_id, 41);
        assert_eq!(d7.width, 64);
        assert_eq!(d7.height, 64);
        assert_eq!(d7.mipmap_max, 6);
        assert_eq!(d7.block_compression(), Some(BlockFormat::Bc4));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = vec![0u8; 16];
        buf[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        let err = TextureBaseIndex::parse(&buf).unwrap_err();
        assert!(matches!(err, CascError::InvalidData(_)));
    }

    #[test]
    fn rejects_truncated_index() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&TEXBASE_MAGIC.to_le_bytes());
        buf.extend_from_slice(&100u32.to_le_bytes()); // claims 100 entries
        buf.resize(20, 0); // but only 12 bytes of index
        let err = TextureBaseIndex::parse(&buf).unwrap_err();
        assert!(matches!(err, CascError::InvalidData(_)));
    }

    #[test]
    fn format_table_covers_known_d4_codes() {
        // Spot-check the codes we observed in real warlock_sigilOfSummons textures.
        assert_eq!(BlockFormat::from_d4_format(46), Some(BlockFormat::Bc1));
        assert_eq!(BlockFormat::from_d4_format(41), Some(BlockFormat::Bc4));
        assert_eq!(BlockFormat::from_d4_format(42), Some(BlockFormat::Bc5));
        assert_eq!(BlockFormat::from_d4_format(44), Some(BlockFormat::Bc7));
        // Uncompressed / unknown returns None.
        assert_eq!(BlockFormat::from_d4_format(0), None); // BGRA8
        assert_eq!(BlockFormat::from_d4_format(999), None);
    }
}
