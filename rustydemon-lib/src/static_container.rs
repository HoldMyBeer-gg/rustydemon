//! Static container storage backend.
//!
//! Used by games that encode the physical storage location of each file
//! directly inside the encoding key, rather than storing it in separate
//! `*.idx` index files.  This matches TACTLib's `StaticContainerHandler` and
//! is the format used by Steam builds of Diablo IV and Overwatch.
//!
//! ## Key-layout format
//!
//! The build config provides `key-layout-index-bits = N` plus one or more
//! `key-layout-K = chunkBits archiveBits offsetBits flags` entries.  For each
//! 16-byte EKey, the top 8 bytes are read as a big-endian u64 and sliced into
//! bit fields from the top down:
//!
//! ```text
//!   [ N index bits | chunkBits | archiveBits | offsetBits | padding ]
//! ```
//!
//! The 4th value (`flags`) is specific to the D4 Steam format and is the
//! physical-offset alignment — TACTLib's Overwatch handler ignores it.  Two
//! values are known:
//!
//! | flags | Meaning                                          |
//! |-------|--------------------------------------------------|
//! | 0     | Byte offset into a `-meta.dat` file              |
//! | 4096  | 4 KiB-aligned offset into a `-payload.dat` file  |
//!
//! Layout 1 (offsetBits = 0) describes a whole loose file — child content
//! stored as `{chunk}/{archive}-child.dat` or `-payload.dat`.

use std::{
    fs,
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use flate2::bufread::ZlibDecoder;

use crate::{blte, config::CascConfig, error::CascError, types::Md5Hash};

// ── Key layout ────────────────────────────────────────────────────────────────

/// One parsed `key-layout-N = chunkBits archiveBits offsetBits flags` entry.
#[derive(Clone, Copy, Debug)]
pub struct KeyLayout {
    pub chunk_bits: u8,
    pub archive_bits: u8,
    pub offset_bits: u8,
    /// Offset alignment: `0` = byte offset (meta.dat), `4096` = 4 KiB blocks (payload.dat).
    pub flags: u32,
}

/// Storage location extracted from an EKey.
#[derive(Clone, Copy, Debug)]
pub struct StorageLocation {
    pub layout_index: u8,
    pub chunk: u32,
    pub archive: u64,
    /// Byte offset into the target file (already scaled by the layout's `flags`).
    pub byte_offset: u64,
    pub layout: KeyLayout,
}

// ── Static container ──────────────────────────────────────────────────────────

/// Reads files from a static-container storage directory.
pub struct StaticContainer {
    /// Directory containing `000/`, `001/`, … sub-directories.
    container_dir: PathBuf,
    /// Number of top bits of the EKey's high u64 used to pick a layout.
    index_bits: u8,
    /// Layout table, indexed by `layout_index` (may contain gaps).
    layouts: Vec<Option<KeyLayout>>,
}

impl StaticContainer {
    /// Parse key-layouts from the build config and prepare a static container
    /// rooted at `container_dir`.
    pub fn from_config(container_dir: PathBuf, config: &CascConfig) -> Result<Self, CascError> {
        let index_bits = config.key_layout_index_bits().ok_or_else(|| {
            CascError::Config("static container: key-layout-index-bits missing".into())
        })?;

        if index_bits > 8 {
            return Err(CascError::Config(format!(
                "static container: key-layout-index-bits {index_bits} > 8"
            )));
        }

        let slots = 1usize << index_bits;
        let mut layouts: Vec<Option<KeyLayout>> = vec![None; slots];

        for (idx, vals) in config.key_layouts() {
            if (idx as usize) >= slots {
                return Err(CascError::Config(format!(
                    "static container: key-layout-{idx} out of range for index-bits {index_bits}"
                )));
            }
            let chunk_bits = vals[0] as u8;
            let archive_bits = vals[1] as u8;
            let offset_bits = vals[2] as u8;
            let flags = vals.get(3).copied().unwrap_or(0);

            let total =
                index_bits as u32 + chunk_bits as u32 + archive_bits as u32 + offset_bits as u32;
            if total > 64 {
                return Err(CascError::Config(format!(
                    "static container: key-layout-{idx} uses {total} bits (> 64)"
                )));
            }

            layouts[idx as usize] = Some(KeyLayout {
                chunk_bits,
                archive_bits,
                offset_bits,
                flags,
            });
        }

        Ok(Self {
            container_dir,
            index_bits,
            layouts,
        })
    }

    /// Root directory of this container (containing `000/`, `001/`, …).
    pub fn container_dir(&self) -> &Path {
        &self.container_dir
    }

    /// Parse the storage-location bit fields out of an EKey.
    pub fn extract_location(&self, ekey: &Md5Hash) -> Result<StorageLocation, CascError> {
        // Top 8 bytes of the ekey as a big-endian u64.  Matching TACTLib's
        // `StaticContainerHandler.ExtractStorageLocation`, the very top byte
        // (bits 56..63) is unused padding — the layout-index field starts at
        // bit `56 - index_bits`, giving a 56-bit location budget.
        let hi = u64::from_be_bytes(ekey.0[8..16].try_into().unwrap());

        let layout_idx_off = 56u32 - self.index_bits as u32;
        let layout_index = extract_bits(hi, layout_idx_off as u8, self.index_bits) as u8;

        let layout = self
            .layouts
            .get(layout_index as usize)
            .and_then(|l| l.as_ref())
            .copied()
            .ok_or_else(|| {
                CascError::Config(format!(
                    "static container: no key-layout defined for index {layout_index}"
                ))
            })?;

        let chunk_off = layout_idx_off - layout.chunk_bits as u32;
        let chunk = extract_bits(hi, chunk_off as u8, layout.chunk_bits) as u32;

        let archive_off = chunk_off - layout.archive_bits as u32;
        let archive = extract_bits(hi, archive_off as u8, layout.archive_bits);

        let offset_off = archive_off - layout.offset_bits as u32;
        let raw_offset = extract_bits(hi, offset_off as u8, layout.offset_bits);

        // D4 Steam: flags encodes the block size for offset scaling.
        // 0 means byte offsets; 4096 means 4 KiB-aligned offsets.
        let byte_offset = if layout.flags == 0 {
            raw_offset
        } else {
            raw_offset
                .checked_mul(layout.flags as u64)
                .ok_or(CascError::Overflow("static container: scaled offset"))?
        };

        Ok(StorageLocation {
            layout_index,
            chunk,
            archive,
            byte_offset,
            layout,
        })
    }

    /// Resolve a storage location to one or more candidate file paths on disk.
    ///
    /// Layouts with `flags == 0` and non-zero `offset_bits` point into
    /// `-meta.dat` files.  Layouts with `flags == 4096` point into
    /// `-payload.dat` files.  Layout 1 (offset_bits == 0) is a whole loose
    /// file whose suffix is unknown without probing — we try both
    /// `-child.dat` and `-payload.dat`.
    pub fn candidate_paths(&self, loc: &StorageLocation) -> Vec<PathBuf> {
        let chunk_dir = format!("{:03}", loc.chunk);
        let base = self.container_dir.join(&chunk_dir);

        if loc.layout.offset_bits == 0 {
            // Loose whole-file: archive acts as a file id; try known suffixes.
            let a = loc.archive;
            vec![
                base.join(format!("{a:x}-child.dat")),
                base.join(format!("{a:x}-payload.dat")),
                base.join(format!("0x{a:04x}-child.dat")),
                base.join(format!("0x{a:04x}-payload.dat")),
            ]
        } else if loc.layout.flags == 0 {
            // Byte-offset into a meta.dat file.
            vec![base.join(format!("0x{:04x}-meta.dat", loc.archive))]
        } else {
            // Block-aligned offset into a payload.dat file.
            vec![base.join(format!("0x{:04x}-payload.dat", loc.archive))]
        }
    }

    /// Open the data file for `loc`, returning the first candidate that exists.
    fn open_data_file(&self, loc: &StorageLocation) -> Result<fs::File, CascError> {
        let candidates = self.candidate_paths(loc);
        for path in &candidates {
            match fs::File::open(path) {
                Ok(f) => return Ok(f),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(CascError::Io(e)),
            }
        }
        Err(CascError::IndexNotFound(format!(
            "static container: no file on disk for chunk={} archive=0x{:x} (tried {})",
            loc.chunk,
            loc.archive,
            candidates.len()
        )))
    }

    /// Decode and return the logical bytes of a file at `ekey`'s storage
    /// location.
    ///
    /// The blob format is autodetected by peeking at the first two bytes:
    ///
    /// - **`BLTE`** — standard CASC wrapper, decoded with [`crate::blte::decode`].
    /// - **`0x78 ..`** — a raw zlib stream (used for TVFS VFS roots in D4,
    ///   where `vfs-*-espec = z`).  Streamed through [`ZlibDecoder`] until
    ///   the stream ends naturally, so we don't need the compressed size.
    /// - Anything else — returned as-is (caller decides what to do with it).
    ///
    /// Layout 1 files (`offset_bits == 0`) are whole loose files: the entire
    /// on-disk file is read, then the same format autodetection is applied.
    pub fn open_by_ekey(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let loc = self.extract_location(ekey)?;
        let mut file = self.open_data_file(&loc)?;

        if loc.layout.offset_bits == 0 {
            // Whole loose file — read everything, then decode.
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return decode_blob(&buf, ekey);
        }

        file.seek(SeekFrom::Start(loc.byte_offset))?;

        // Peek the first two bytes to pick BLTE vs zlib without rewinding
        // (the file handle is owned, so we can seek back).
        let mut head = [0u8; 2];
        file.read_exact(&mut head)?;
        file.seek(SeekFrom::Start(loc.byte_offset))?;

        match head {
            // BLTE magic, little-endian first two bytes: 'B','L'.
            [b'B', b'L'] => {
                let raw = read_blte_stream(&mut file)?;
                blte::decode(&raw, ekey, false)
            }
            // Zlib header — deflate stream, no BLTE wrapper.  Stream the
            // whole thing through the decoder; it stops at the end marker
            // so we don't need the compressed size up front.
            [0x78, _] => {
                let mut dec = ZlibDecoder::new(BufReader::new(&mut file));
                let mut out = Vec::new();
                dec.read_to_end(&mut out)
                    .map_err(|e| CascError::Blte(format!("static zlib decode: {e}")))?;
                Ok(out)
            }
            _ => Err(CascError::Blte(format!(
                "static container: unrecognised blob header {head:02X?} at \
                 chunk={} archive=0x{:x} offset={}",
                loc.chunk, loc.archive, loc.byte_offset
            ))),
        }
    }

    /// Read the raw (still-encoded) BLTE bytes at an EKey's location.
    ///
    /// Only valid for BLTE-wrapped files.  Used by callers that need the
    /// original bytes for hash validation or re-export.
    pub fn read_raw(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let loc = self.extract_location(ekey)?;
        let mut file = self.open_data_file(&loc)?;

        if loc.layout.offset_bits == 0 {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }

        file.seek(SeekFrom::Start(loc.byte_offset))?;
        read_blte_stream(&mut file)
    }
}

/// Decode a loose-file blob using the same format autodetection as
/// [`StaticContainer::open_by_ekey`].
fn decode_blob(buf: &[u8], ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
    if buf.len() >= 2 {
        match &buf[..2] {
            b"BL" => return blte::decode(buf, ekey, false),
            &[0x78, _] => {
                let mut dec = ZlibDecoder::new(buf);
                let mut out = Vec::new();
                dec.read_to_end(&mut out)
                    .map_err(|e| CascError::Blte(format!("static zlib decode: {e}")))?;
                return Ok(out);
            }
            _ => {}
        }
    }
    // Unknown format: return the bytes untouched so the caller can inspect.
    Ok(buf.to_vec())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract `count` bits from `value`, where bit position `start` is the
/// least-significant bit of the extracted field.  (Equivalent to
/// `(value >> start) & ((1 << count) - 1)`, with `count == 0` → 0.)
#[inline]
fn extract_bits(value: u64, start: u8, count: u8) -> u64 {
    if count == 0 {
        return 0;
    }
    let mask: u64 = if count >= 64 {
        u64::MAX
    } else {
        (1u64 << count) - 1
    };
    (value >> start) & mask
}

/// Read one BLTE-framed blob from the current file position.
///
/// The BLTE header is parsed just enough to determine the total on-disk size,
/// then the entire blob (header + block payloads) is read into a `Vec<u8>` so
/// it can be handed to [`crate::blte::decode`].
fn read_blte_stream(file: &mut fs::File) -> Result<Vec<u8>, CascError> {
    // First 8 bytes: magic + header-size.
    let mut head = [0u8; 8];
    file.read_exact(&mut head)?;

    let magic = u32::from_le_bytes(head[0..4].try_into().unwrap());
    if magic != 0x4554_4C42 {
        return Err(CascError::Blte(format!(
            "static container: expected BLTE magic, got {magic:#010X}"
        )));
    }

    let header_size = u32::from_be_bytes(head[4..8].try_into().unwrap()) as usize;
    if header_size == 0 {
        return Err(CascError::Blte(
            "static container: headerless BLTE blocks are not supported".into(),
        ));
    }
    if header_size < 12 {
        return Err(CascError::Blte(format!(
            "static container: BLTE header size {header_size} too small"
        )));
    }

    // Read the rest of the header into a buffer that will eventually hold
    // the full on-disk blob.
    let mut buf = vec![0u8; header_size];
    buf[..8].copy_from_slice(&head);
    file.read_exact(&mut buf[8..])?;

    if buf[8] != 0x0F {
        return Err(CascError::Blte(format!(
            "static container: BLTE frame flag {:#04X} != 0x0F",
            buf[8]
        )));
    }

    let num_blocks = ((buf[9] as usize) << 16) | ((buf[10] as usize) << 8) | (buf[11] as usize);
    let expected = 12 + num_blocks * 24;
    if expected != header_size {
        return Err(CascError::Blte(format!(
            "static container: BLTE header size {header_size} != expected {expected}"
        )));
    }

    let mut total_payload: usize = 0;
    for i in 0..num_blocks {
        let off = 12 + i * 24;
        let comp = u32::from_be_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        total_payload = total_payload
            .checked_add(comp)
            .ok_or(CascError::Overflow("BLTE total payload size"))?;
    }

    let header_len = buf.len();
    buf.resize(header_len + total_payload, 0);
    file.read_exact(&mut buf[header_len..])?;

    Ok(buf)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bits_basic() {
        // 0xF000_0000_0000_0000, pull top 4 bits.
        assert_eq!(extract_bits(0xF000_0000_0000_0000, 60, 4), 0xF);
        // 0x0000_0000_0000_00FF, pull low 8 bits.
        assert_eq!(extract_bits(0x0000_0000_0000_00FF, 0, 8), 0xFF);
        assert_eq!(extract_bits(0xDEAD_BEEF_CAFE_BABE, 16, 16), 0xCAFE);
        assert_eq!(extract_bits(0x1234_5678_9ABC_DEF0, 0, 0), 0);
    }
}
