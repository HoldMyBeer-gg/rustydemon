//! Parser for CDN archive-index files (`<hash>.index`), as used by
//! modern CASC installations — most notably D2R 3.1.2+.
//!
//! # Format reference
//!
//! Ported from CascLib's `CascIndexFiles.cpp` (`CaptureArchiveIndexFooter`,
//! `CaptureIndexEntry`, `VerifyIndexSize`, `LoadArchiveIndexFile`) and the
//! `FILE_INDEX_FOOTER` / `CASC_ARCINDEX_FOOTER` structs in `CascStructs.h` /
//! `CascCommon.h`.  That code is vendored in this repo under `CascLib/src/`
//! for reference.
//!
//! ## File layout
//!
//! Pages are stored **contiguously**, followed by all per-page hashes in
//! one block, followed by a TOC page, followed by the footer.  I had this
//! wrong on the first pass (assumed interleaved `[page][hash][page][hash]`
//! like a lot of on-disk table formats) and the first real-world probe
//! against D2R's 361 `.index` files caught the misalignment via ~300
//! entry-count underruns.  Check `CascLib/src/CascIndexFiles.cpp`
//! `LoadArchiveIndexFile`: the advance is `pbIndexFile += PageLength`,
//! no hash skip, confirming pages are contiguous.
//!
//! ```text
//!   ┌──────────────────────────────┐
//!   │ Data page 0  (PageLength B)  │   entries, padded at end with
//!   ├──────────────────────────────┤   zeros when they don't fit the
//!   │ Data page 1  (PageLength B)  │   last entry evenly
//!   ├──────────────────────────────┤
//!   │ …                            │
//!   ├──────────────────────────────┤
//!   │ Data page N-1                │
//!   ├──────────────────────────────┤
//!   │ Per-page MD5 hashes          │   16 × N bytes (we don't verify
//!   ├──────────────────────────────┤   them — strictly optional)
//!   │ TOC page                     │   first ekey of each data page;
//!   │                              │   only needed for binary search,
//!   │                              │   ignored here since we load every
//!   │                              │   entry up front
//!   ├──────────────────────────────┤
//!   │ Footer (36 B, v1 w/ chk=8)   │
//!   └──────────────────────────────┘
//! ```
//!
//! ## Footer (36 bytes, version 1, `FooterHashBytes = 8`)
//!
//! | Offset | Size | Field             | Notes                                  |
//! |--------|------|-------------------|----------------------------------------|
//! | 0      | 16   | `TocHash`         | MD5 of the TOC page                    |
//! | 16     | 1    | `Version`         | == 1                                   |
//! | 17     | 2    | `Reserved[2]`     | == 0, 0                                |
//! | 19     | 1    | `PageSizeKB`      | page length in kilobytes (4 ⇒ 4096)    |
//! | 20     | 1    | `OffsetBytes`     | width of archive-offset field (4)      |
//! | 21     | 1    | `SizeBytes`       | width of size field (4)                |
//! | 22     | 1    | `EKeyLength`      | bytes per ekey (16)                    |
//! | 23     | 1    | `FooterHashBytes` | width of the trailing footer hash (8)  |
//! | 24     | 4    | `ElementCount`    | little-endian total entry count        |
//! | 28     | 8    | `FooterHash`      | MD5 of the footer-fields above, trunc. |
//!
//! ## Entry layout (ItemLength = EKeyLength + SizeBytes + OffsetBytes)
//!
//! ```text
//!   [ ekey : EKeyLength bytes ][ size : SizeBytes BE ][ archive_offset : OffsetBytes BE ]
//! ```
//!
//! Note the `size` field comes **before** `archive_offset`, not after.  This
//! is easy to get wrong by eyeballing hex dumps and cost me most of a
//! session — match `CaptureIndexEntry` in `CascIndexFiles.cpp` exactly.

use std::{fs, io, path::Path};

use crate::{error::CascError, types::Md5Hash};

/// Size, in bytes, of the fixed footer at the end of an archive-index file
/// (version 1, `FooterHashBytes = 8` — the only variant in use as of D2R 3.1.2).
pub const FOOTER_BYTES: usize = 36;

/// MD5 hash width used for both the per-page hashes and the TOC hash.
const HASH_BYTES: usize = 16;

/// Decoded contents of an archive-index footer.
///
/// Layout matches `CASC_ARCINDEX_FOOTER` in `CascLib/src/CascCommon.h`.
#[derive(Debug, Clone, Copy)]
pub struct ArchiveIndexFooter {
    pub version: u8,
    /// Width (in bytes) of an entry's `archive_offset` field.  Normally `4`
    /// for archive indices, `6` for group indices, `0` for loose indices.
    pub offset_bytes: u8,
    /// Width (in bytes) of an entry's `size` field.  Normally `4`.
    pub size_bytes: u8,
    /// Width (in bytes) of an entry's ekey.  Normally `16`.
    pub ekey_length: u8,
    /// Width (in bytes) of the trailing footer hash.  Normally `8`.
    pub footer_hash_bytes: u8,
    /// Total number of entries in the file (from the footer's `ElementCount`).
    /// This value is advisory — we parse entries by walking pages, not by
    /// trusting the count.
    pub element_count: u32,
    /// Size, in bytes, of one entry on disk.
    pub item_length: usize,
    /// Size, in bytes, of one data page (not counting its trailing page hash).
    pub page_length: usize,
}

/// One decoded (ekey → archive location) mapping.
#[derive(Debug, Clone, Copy)]
pub struct ArchiveIndexEntry {
    pub ekey: Md5Hash,
    /// Byte offset into the archive blob that this `.index` file indexes
    /// (the archive's hash is the `.index` file's stem — caller's
    /// responsibility to track).
    pub archive_offset: u64,
    /// Encoded (BLTE-wrapped) size of the file at that offset.
    pub encoded_size: u32,
}

/// Parse an archive-index file from disk.
///
/// Reads the whole file into memory (these are typically 10–200 KiB),
/// validates the footer, and walks every page collecting entries.  Returns
/// the decoded footer plus every entry.  All-zero ekeys at the tail of a
/// page are skipped — CascLib treats those as page padding.
pub fn parse_file(path: &Path) -> Result<(ArchiveIndexFooter, Vec<ArchiveIndexEntry>), CascError> {
    let data = fs::read(path).map_err(|e| io_err(path, e))?;
    parse_bytes(&data)
}

/// Parse an archive-index file from an in-memory buffer.
///
/// Exposed separately so tests can feed fixtures without touching disk.
pub fn parse_bytes(data: &[u8]) -> Result<(ArchiveIndexFooter, Vec<ArchiveIndexEntry>), CascError> {
    let footer = parse_footer(data)?;
    let entries = parse_entries(data, &footer)?;
    Ok((footer, entries))
}

// ── Footer ────────────────────────────────────────────────────────────────────

fn parse_footer(data: &[u8]) -> Result<ArchiveIndexFooter, CascError> {
    if data.len() < FOOTER_BYTES {
        return Err(CascError::InvalidData(format!(
            "archive index too short for footer: {} < {}",
            data.len(),
            FOOTER_BYTES
        )));
    }
    let f = &data[data.len() - FOOTER_BYTES..];

    // Layout matches FILE_INDEX_FOOTER<0x08> in CascLib/src/CascStructs.h.
    //
    //  0..16  TocHash
    //  16     Version
    //  17..19 Reserved[2]
    //  19     PageSizeKB
    //  20     OffsetBytes
    //  21     SizeBytes
    //  22     EKeyLength
    //  23     FooterHashBytes
    //  24..28 ElementCount (LE)
    //  28..36 FooterHash (truncated MD5)
    let version = f[16];
    let reserved = [f[17], f[18]];
    let page_size_kb = f[19];
    let offset_bytes = f[20];
    let size_bytes = f[21];
    let ekey_length = f[22];
    let footer_hash_bytes = f[23];
    let element_count = u32::from_le_bytes(f[24..28].try_into().unwrap());

    if version != 1 || reserved != [0, 0] || footer_hash_bytes != 8 {
        return Err(CascError::InvalidData(format!(
            "archive index footer: unexpected \
             version={version} reserved={reserved:?} footer_hash_bytes={footer_hash_bytes} \
             (only version 1 with FooterHashBytes == 8 is supported)"
        )));
    }

    if ekey_length == 0 || ekey_length as usize > HASH_BYTES {
        return Err(CascError::InvalidData(format!(
            "archive index footer: implausible EKeyLength {ekey_length}"
        )));
    }
    if offset_bytes > 8 {
        return Err(CascError::InvalidData(format!(
            "archive index footer: implausible OffsetBytes {offset_bytes}"
        )));
    }
    // OffsetBytes == 0 is valid: CascLib notes "0 for loose file indices"
    // in CascStructs.h. Those entries represent a whole archive file with
    // no per-entry offset — the ekey IS the archive. D2R ships 8 such
    // loose indices out of 361 total.
    if size_bytes == 0 || size_bytes > 4 {
        return Err(CascError::InvalidData(format!(
            "archive index footer: implausible SizeBytes {size_bytes}"
        )));
    }

    let item_length = ekey_length as usize + offset_bytes as usize + size_bytes as usize;
    let page_length = (page_size_kb as usize) << 10;
    if page_length < item_length {
        return Err(CascError::InvalidData(format!(
            "archive index footer: page_length {page_length} < item_length {item_length}"
        )));
    }

    Ok(ArchiveIndexFooter {
        version,
        offset_bytes,
        size_bytes,
        ekey_length,
        footer_hash_bytes,
        element_count,
        item_length,
        page_length,
    })
}

// ── Page walk + entry decode ──────────────────────────────────────────────────

fn parse_entries(
    data: &[u8],
    footer: &ArchiveIndexFooter,
) -> Result<Vec<ArchiveIndexEntry>, CascError> {
    // CascLib's VerifyIndexSize strips the footer, then divides by
    // `(PageLength + HashBytes)` to get the whole-page count.  That
    // denominator is the _total_ space one logical page takes up on disk
    // (PageLength of entries plus its 16-byte MD5 stored in the trailing
    // hash block).  LoadArchiveIndexFile then walks pages CONTIGUOUSLY:
    // `pbIndexFile += PageLength`, with no stride gap, starting at offset
    // 0. That means all N pages of entries are packed together at the
    // start of the file, followed by the N per-page hashes, followed by
    // the TOC, followed by the footer.  We can ignore the hashes and TOC
    // entirely — we load every entry up front instead of binary-searching.
    let body = data
        .get(..data.len().saturating_sub(FOOTER_BYTES))
        .ok_or_else(|| CascError::InvalidData("archive index body empty".into()))?;
    let page_count = body.len() / (footer.page_length + HASH_BYTES);
    let pages_total = page_count * footer.page_length;

    if pages_total > body.len() {
        return Err(CascError::InvalidData(format!(
            "archive index: computed page region {pages_total} > body {}",
            body.len()
        )));
    }

    let mut entries =
        Vec::with_capacity(footer.element_count as usize + page_count.saturating_mul(2));

    for page_idx in 0..page_count {
        let page_start = page_idx * footer.page_length;
        let page_end = page_start + footer.page_length;
        parse_page_entries(&body[page_start..page_end], footer, &mut entries);
    }

    Ok(entries)
}

fn parse_page_entries(page: &[u8], footer: &ArchiveIndexFooter, out: &mut Vec<ArchiveIndexEntry>) {
    let item = footer.item_length;
    let ekey_len = footer.ekey_length as usize;
    let mut pos = 0usize;
    while pos + item <= page.len() {
        // CascLib's CaptureIndexEntry delegates validity to CascIsValidMD5,
        // which rejects all-zero keys.  End-of-page padding manifests as
        // all-zero trailing bytes, so once we see a zero ekey there are no
        // more real entries in this page.
        //
        // CascLib additionally rejects archive offsets >= 0x10000000 as
        // bogus, but that guard is specific to its `ConvertBytesToInteger_X`
        // helper which truncates to 4 bytes regardless of field width —
        // any large offset that fits in 5+ bytes looks broken to CascLib's
        // truncated read.  We read the full field width correctly, so
        // there's no equivalent sanity cap to apply: D2R ships 8 archive
        // indices with `OffsetBytes = 5` whose real offsets legitimately
        // exceed 256 MiB, and capping them here drops ~2.7 million real
        // entries across those files.
        if page[pos..pos + ekey_len].iter().all(|&b| b == 0) {
            break;
        }

        let ekey_end = pos + ekey_len;
        let size_end = ekey_end + footer.size_bytes as usize;
        let offset_end = size_end + footer.offset_bytes as usize;

        let mut ekey_bytes = [0u8; 16];
        let copy_len = ekey_len.min(16);
        ekey_bytes[..copy_len].copy_from_slice(&page[pos..pos + copy_len]);

        // Per CaptureIndexEntry in CascIndexFiles.cpp, the size field comes
        // before the archive_offset field.  Both are big-endian via
        // ConvertBytesToInteger_X (for CascLib) / read_be_uint (for us).
        let encoded_size =
            read_be_uint(&page[ekey_end..size_end], footer.size_bytes as usize) as u32;
        let archive_offset =
            read_be_uint(&page[size_end..offset_end], footer.offset_bytes as usize);

        out.push(ArchiveIndexEntry {
            ekey: Md5Hash(ekey_bytes),
            archive_offset,
            encoded_size,
        });

        pos += item;
    }
}

/// Variable-width big-endian unsigned integer read, matching
/// `ConvertBytesToInteger_X` in `CascLib/src/common/Common.h`.
fn read_be_uint(bytes: &[u8], width: usize) -> u64 {
    let mut value: u64 = 0;
    for &b in bytes.iter().take(width) {
        value = (value << 8) | b as u64;
    }
    value
}

// ── Error helper ──────────────────────────────────────────────────────────────

fn io_err(path: &Path, e: io::Error) -> CascError {
    CascError::Io(io::Error::new(
        e.kind(),
        format!("reading archive index {}: {e}", path.display()),
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid v1 footer by hand so we can unit-test the parser
    /// without needing a real `.index` fixture.  Matches
    /// `FILE_INDEX_FOOTER<0x08>` layout exactly.
    fn synth_footer(
        page_size_kb: u8,
        offset_bytes: u8,
        size_bytes: u8,
        ekey_length: u8,
        element_count: u32,
    ) -> Vec<u8> {
        let mut f = vec![0u8; FOOTER_BYTES];
        // TocHash 0..16 left zero
        f[16] = 1; // version
                   // Reserved[2] 17..19 left zero
        f[19] = page_size_kb;
        f[20] = offset_bytes;
        f[21] = size_bytes;
        f[22] = ekey_length;
        f[23] = 8; // FooterHashBytes (must be 8)
        f[24..28].copy_from_slice(&element_count.to_le_bytes());
        // FooterHash 28..36 left zero — we don't verify it
        f
    }

    fn be_bytes(value: u64, width: usize) -> Vec<u8> {
        let mut out = vec![0u8; width];
        let mut v = value;
        for i in (0..width).rev() {
            out[i] = (v & 0xFF) as u8;
            v >>= 8;
        }
        out
    }

    fn synth_entry(ekey: [u8; 16], size: u32, offset: u32) -> Vec<u8> {
        let mut e = Vec::with_capacity(24);
        e.extend_from_slice(&ekey);
        e.extend_from_slice(&be_bytes(size as u64, 4));
        e.extend_from_slice(&be_bytes(offset as u64, 4));
        e
    }

    /// Build one PageLength-byte page full of entries plus zero padding
    /// up to `page_len`.  Page hashes are stored in a separate block AFTER
    /// all the pages, not interleaved — callers compose the final file via
    /// [`synth_archive_index`].
    fn synth_page(entries: &[Vec<u8>], page_len: usize) -> Vec<u8> {
        let mut page = Vec::with_capacity(page_len);
        for e in entries {
            page.extend_from_slice(e);
        }
        page.resize(page_len, 0);
        page
    }

    /// Assemble a full archive-index file matching the real disk layout:
    /// all page bodies concatenated, followed by N page hashes (zeroed),
    /// followed by a TOC page (we fill it with zeros — we never parse it),
    /// followed by the 36-byte footer.  `toc_bytes` lets tests control the
    /// exact trailing-region size since CascLib's VerifyIndexSize lumps
    /// the TOC together with leftover bytes before the footer.
    fn synth_archive_index(pages: &[Vec<u8>], page_len: usize, footer: Vec<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        for page in pages {
            assert_eq!(page.len(), page_len, "page must be page_len bytes");
            out.extend_from_slice(page);
        }
        // N per-page hashes (zeroed — we never verify them)
        out.resize(out.len() + pages.len() * HASH_BYTES, 0);
        // Optional TOC — CascLib uses it for binary search, we skip it.
        // Size one entry per page to match real layout roughly.
        let toc_entries = pages.len();
        out.resize(out.len() + toc_entries * 24, 0);
        out.extend_from_slice(&footer);
        out
    }

    #[test]
    fn footer_roundtrip_defaults() {
        let raw = synth_footer(4, 4, 4, 16, 42);
        let footer = parse_footer(&raw).unwrap();
        assert_eq!(footer.version, 1);
        assert_eq!(footer.offset_bytes, 4);
        assert_eq!(footer.size_bytes, 4);
        assert_eq!(footer.ekey_length, 16);
        assert_eq!(footer.footer_hash_bytes, 8);
        assert_eq!(footer.element_count, 42);
        assert_eq!(footer.item_length, 24);
        assert_eq!(footer.page_length, 4096);
    }

    #[test]
    fn footer_rejects_bad_version() {
        let mut raw = synth_footer(4, 4, 4, 16, 0);
        raw[16] = 2; // version
        assert!(parse_footer(&raw).is_err());
    }

    #[test]
    fn footer_rejects_wrong_footer_hash_size() {
        let mut raw = synth_footer(4, 4, 4, 16, 0);
        raw[23] = 4;
        assert!(parse_footer(&raw).is_err());
    }

    #[test]
    fn footer_rejects_zero_ekey_length() {
        let mut raw = synth_footer(4, 4, 4, 16, 0);
        raw[22] = 0;
        assert!(parse_footer(&raw).is_err());
    }

    #[test]
    fn footer_rejects_too_short_buffer() {
        let raw = vec![0u8; FOOTER_BYTES - 1];
        assert!(parse_footer(&raw).is_err());
    }

    #[test]
    fn entries_single_page_two_entries() {
        let ekey_a = [0xAA; 16];
        let ekey_b = [0xBB; 16];
        let entry_a = synth_entry(ekey_a, 0x1234, 0x0000_0100);
        let entry_b = synth_entry(ekey_b, 0x5678_9ABC, 0x0010_0000);
        let page = synth_page(&[entry_a, entry_b], 4096);
        let footer = synth_footer(4, 4, 4, 16, 2);
        let data = synth_archive_index(&[page], 4096, footer);

        let (f, entries) = parse_bytes(&data).unwrap();
        assert_eq!(f.item_length, 24);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].ekey.0, ekey_a);
        assert_eq!(entries[0].encoded_size, 0x1234);
        assert_eq!(entries[0].archive_offset, 0x0000_0100);

        assert_eq!(entries[1].ekey.0, ekey_b);
        assert_eq!(entries[1].encoded_size, 0x5678_9ABC);
        assert_eq!(entries[1].archive_offset, 0x0010_0000);
    }

    #[test]
    fn entries_stop_on_zero_padding() {
        let ekey_a = [0xAA; 16];
        let ekey_b = [0xBB; 16];
        let entries = vec![synth_entry(ekey_a, 1, 2), synth_entry(ekey_b, 3, 4)];
        let page = synth_page(&entries, 4096);
        let footer = synth_footer(4, 4, 4, 16, 2);
        let data = synth_archive_index(&[page], 4096, footer);

        let (_, parsed) = parse_bytes(&data).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn entries_multiple_pages() {
        let footer = synth_footer(4, 4, 4, 16, 3);

        let page1 = synth_page(
            &[
                synth_entry([0x01; 16], 10, 100),
                synth_entry([0x02; 16], 20, 200),
            ],
            4096,
        );
        let page2 = synth_page(&[synth_entry([0x03; 16], 30, 300)], 4096);
        let data = synth_archive_index(&[page1, page2], 4096, footer);

        let (_, entries) = parse_bytes(&data).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ekey.0[0], 0x01);
        assert_eq!(entries[1].ekey.0[0], 0x02);
        assert_eq!(entries[2].ekey.0[0], 0x03);
        assert_eq!(entries[2].encoded_size, 30);
        assert_eq!(entries[2].archive_offset, 300);
    }

    #[test]
    fn entries_five_byte_offsets_allow_values_above_256mib() {
        // D2R ships 8 archive indices with OffsetBytes = 5 whose real
        // offsets legitimately exceed 256 MiB.  CascLib's truncation-based
        // "offset >= 0x10000000" guard wrongly drops those — make sure we
        // don't.
        let footer = synth_footer(4, 5, 4, 16, 1);

        let ekey = [0x77; 16];
        let huge_offset: u64 = 0x0000_0004_0000_0000; // 16 GiB, fits in 5 bytes
        let mut entry = Vec::with_capacity(16 + 4 + 5);
        entry.extend_from_slice(&ekey);
        entry.extend_from_slice(&be_bytes(0xABCD, 4));
        entry.extend_from_slice(&be_bytes(huge_offset, 5));

        let page = synth_page(&[entry], 4096);
        let data = synth_archive_index(&[page], 4096, footer);

        let (f, entries) = parse_bytes(&data).unwrap();
        assert_eq!(f.offset_bytes, 5);
        assert_eq!(f.item_length, 25);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_offset, huge_offset);
        assert_eq!(entries[0].encoded_size, 0xABCD);
    }

    #[test]
    fn entries_loose_index_zero_offset_bytes() {
        // Loose-file archive indices have OffsetBytes = 0: the ekey _is_
        // the archive, no per-entry offset.  D2R ships 8 of these out of
        // 361 total.
        let footer = synth_footer(4, 0, 4, 16, 1);

        let ekey = [0x42; 16];
        let mut entry = Vec::with_capacity(20);
        entry.extend_from_slice(&ekey);
        entry.extend_from_slice(&be_bytes(0xDEAD_BEEF, 4)); // size
                                                            // no offset bytes

        let page = synth_page(&[entry], 4096);
        let data = synth_archive_index(&[page], 4096, footer);

        let (f, entries) = parse_bytes(&data).unwrap();
        assert_eq!(f.offset_bytes, 0);
        assert_eq!(f.item_length, 20);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ekey.0, ekey);
        assert_eq!(entries[0].encoded_size, 0xDEAD_BEEF);
        assert_eq!(entries[0].archive_offset, 0);
    }

    #[test]
    fn read_be_uint_widths() {
        assert_eq!(read_be_uint(&[0x12, 0x34], 2), 0x1234);
        assert_eq!(read_be_uint(&[0x12, 0x34, 0x56, 0x78], 4), 0x1234_5678);
        assert_eq!(read_be_uint(&[0xFF; 8], 8), 0xFFFF_FFFF_FFFF_FFFF);
        // width > slice length: take what's available
        assert_eq!(read_be_uint(&[0xAB], 1), 0xAB);
    }
}
