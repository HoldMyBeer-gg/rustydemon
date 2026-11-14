use std::io::Read;

use flate2::read::ZlibDecoder;
use md5::{Digest, Md5};

use crate::{error::CascError, key_service, salsa20::Salsa20, types::Md5Hash};

// Safety limits to prevent decompression bombs.
const MAX_BLTE_BYTES: usize = 512 * 1024 * 1024; // 512 MiB

pub(crate) const BLTE_MAGIC: u32 = 0x4554_4C42; // 'BLTE' LE

/// Decode a BLTE-encoded byte slice into raw file data.
///
/// `ekey` is the encoding key whose first 9 bytes the BLTE header hash is
/// compared against (when `validate` is `true`).
///
/// # Errors
///
/// Returns [`CascError::Blte`] for any structural problem, [`CascError::MissingKey`]
/// when an encrypted block uses a TACT key not in our table, and
/// [`CascError::HashMismatch`] on integrity check failures (only when `validate`
/// is `true`).
pub fn decode(data: &[u8], ekey: &Md5Hash, validate: bool) -> Result<Vec<u8>, CascError> {
    decode_inner(data, ekey, validate, None)
}

/// `missing`: `None` to fail on a block whose key we lack; `Some` to fill
/// such a block with zeros and record its key there.
fn decode_inner(
    data: &[u8],
    ekey: &Md5Hash,
    validate: bool,
    mut missing: Option<&mut Vec<u64>>,
) -> Result<Vec<u8>, CascError> {
    if data.len() < 8 {
        return Err(CascError::Blte("data too short for BLTE header".into()));
    }

    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic != BLTE_MAGIC {
        return Err(CascError::Blte(format!(
            "invalid BLTE magic: {magic:#010X}"
        )));
    }

    let header_size = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
    let has_header = header_size > 0;

    // Optional header integrity check.
    if validate {
        let check_len = if has_header { header_size } else { data.len() };
        let hash: [u8; 16] = Md5::digest(&data[..check_len]).into();
        if hash[..9] != ekey.0[..9] {
            return Err(CascError::HashMismatch(format!(
                "BLTE header hash mismatch for ekey {ekey}"
            )));
        }
    }

    // Parse block descriptors.
    let blocks: Vec<BlockDesc> = if has_header {
        parse_header(data, header_size)?
    } else {
        // No header → single implicit block.
        let payload_size = data.len() - 8;
        vec![BlockDesc {
            comp_size: payload_size,
            decomp_size: payload_size.saturating_sub(1),
            hash: Md5Hash::default(),
        }]
    };

    // Allocate output buffer.
    let total_decomp: usize = blocks.iter().map(|b| b.decomp_size).sum();
    if total_decomp > MAX_BLTE_BYTES {
        return Err(CascError::Blte(format!(
            "decompressed size {total_decomp} exceeds limit {MAX_BLTE_BYTES}"
        )));
    }

    let mut out = Vec::with_capacity(total_decomp);
    let mut pos = header_size.max(8); // start of block data

    for (idx, block) in blocks.iter().enumerate() {
        let end = pos
            .checked_add(block.comp_size)
            .ok_or(CascError::Overflow("BLTE block end"))?;
        if end > data.len() {
            return Err(CascError::Blte(format!(
                "block {idx}: compressed range {pos}..{end} exceeds data length {}",
                data.len()
            )));
        }
        let block_data = &data[pos..end];
        pos = end;

        // Per-block hash validation.
        if validate && has_header && !block.hash.is_zero() {
            let hash: [u8; 16] = Md5::digest(block_data).into();
            if hash != block.hash.0 {
                return Err(CascError::HashMismatch(format!(
                    "BLTE block {idx} hash mismatch"
                )));
            }
        }

        let start = out.len();
        match decode_block(block_data, idx, &mut out) {
            Ok(()) => {}
            // Only a block table records how many bytes the block decodes to.
            Err(CascError::MissingKey(key)) if has_header && missing.is_some() => {
                out.truncate(start);
                out.resize(start + block.decomp_size, 0);
                if let Some(keys) = missing.as_deref_mut() {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }

    Ok(out)
}

/// A file decoded by [`decode_partial`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialDecode {
    /// The file, with zeros where a block could not be decrypted.
    pub data: Vec<u8>,
    /// TACT key names of the blocks that were filled with zeros, each once,
    /// in the order met.  Empty when the whole file was decoded.
    pub missing_keys: Vec<u64>,
}

/// [`decode`], but a block encrypted with a key we do not hold is replaced
/// by zeros of the block's decoded size and decoding carries on.
///
/// Game data tables (`.db2`) keep rows for unreleased content in encrypted
/// blocks; the rest of the table is readable, and readers of that format
/// know to skip an all-zero section.  A file with no block table (a single
/// implicit block) has no recorded size to fill, so a missing key there is
/// still [`CascError::MissingKey`].
pub fn decode_partial(
    data: &[u8],
    ekey: &Md5Hash,
    validate: bool,
) -> Result<PartialDecode, CascError> {
    let mut missing_keys = Vec::new();
    let data = decode_inner(data, ekey, validate, Some(&mut missing_keys))?;
    Ok(PartialDecode { data, missing_keys })
}

// ── Block descriptor ───────────────────────────────────────────────────────────

struct BlockDesc {
    comp_size: usize,
    decomp_size: usize,
    hash: Md5Hash,
}

fn parse_header(data: &[u8], header_size: usize) -> Result<Vec<BlockDesc>, CascError> {
    if data.len() < 12 {
        return Err(CascError::Blte("BLTE header too short".into()));
    }

    let flag = data[8];
    if flag != 0x0F {
        return Err(CascError::Blte(format!(
            "unexpected frame-count flag: {flag:#04X} (expected 0x0F)"
        )));
    }

    // 3-byte big-endian block count at bytes 9–11.
    let num_blocks = ((data[9] as usize) << 16) | ((data[10] as usize) << 8) | (data[11] as usize);

    if num_blocks == 0 {
        return Err(CascError::Blte("BLTE block count is zero".into()));
    }

    let expected_header = 12 + num_blocks * 24;
    if header_size != expected_header {
        return Err(CascError::Blte(format!(
            "header size {header_size} != expected {expected_header}"
        )));
    }
    if data.len() < expected_header {
        return Err(CascError::Blte(
            "BLTE data truncated before block table".into(),
        ));
    }

    let mut blocks = Vec::with_capacity(num_blocks);
    let mut off = 12usize;
    for _ in 0..num_blocks {
        let comp_size = u32::from_be_bytes(data[off..off + 4].try_into().unwrap()) as usize;
        let decomp_size = u32::from_be_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
        let mut hash_bytes = [0u8; 16];
        hash_bytes.copy_from_slice(&data[off + 8..off + 24]);
        blocks.push(BlockDesc {
            comp_size,
            decomp_size,
            hash: Md5Hash(hash_bytes),
        });
        off += 24;
    }

    Ok(blocks)
}

// ── Block decoding ─────────────────────────────────────────────────────────────

fn decode_block(data: &[u8], block_idx: usize, out: &mut Vec<u8>) -> Result<(), CascError> {
    if data.is_empty() {
        return Err(CascError::Blte(format!("block {block_idx} is empty")));
    }

    match data[0] {
        0x4E => {
            // 'N', not compressed, raw copy.
            out.extend_from_slice(&data[1..]);
        }
        0x5A => {
            // 'Z': zlib/deflate compressed.
            let mut dec = ZlibDecoder::new(&data[1..]);
            let mut buf = Vec::new();
            dec.read_to_end(&mut buf)
                .map_err(|e| CascError::Blte(format!("block {block_idx} zlib error: {e}")))?;
            out.extend_from_slice(&buf);
        }
        0x45 => {
            // 'E': encrypted; decrypt then recurse.
            let decrypted = decrypt_block(&data[1..], block_idx)?;
            if let Some(dec) = decrypted {
                decode_block(&dec, block_idx, out)?;
            } else {
                // Key unknown: emit zeros for the expected size.
                // (Caller may handle this differently if desired.)
            }
        }
        0x46 => {
            // 'F': frame (recursive BLTE); not implemented.
            return Err(CascError::Blte(
                "BLTE frame blocks ('F') are not supported".into(),
            ));
        }
        t => {
            return Err(CascError::Blte(format!(
                "block {block_idx}: unknown block type 0x{t:02X} ('{}')",
                if t.is_ascii_graphic() { t as char } else { '?' }
            )));
        }
    }

    Ok(())
}

fn decrypt_block(data: &[u8], block_idx: usize) -> Result<Option<Vec<u8>>, CascError> {
    if data.is_empty() {
        return Err(CascError::Blte("encrypted block too short".into()));
    }

    let mut pos = 0usize;

    // Key name size (must be 8).
    let key_name_size = data[pos] as usize;
    pos += 1;
    if key_name_size != 8 {
        return Err(CascError::Blte(format!(
            "encrypted block: key name size {key_name_size} (expected 8)"
        )));
    }
    if data.len() < pos + key_name_size {
        return Err(CascError::Blte(
            "encrypted block: truncated key name".into(),
        ));
    }
    let key_name = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
    pos += key_name_size;

    // IV size (4 or 8).
    if pos >= data.len() {
        return Err(CascError::Blte(
            "encrypted block: truncated after key name".into(),
        ));
    }
    let iv_size = data[pos] as usize;
    pos += 1;
    if iv_size != 4 && iv_size != 8 {
        return Err(CascError::Blte(format!(
            "encrypted block: IV size {iv_size} (expected 4 or 8)"
        )));
    }
    if data.len() < pos + iv_size {
        return Err(CascError::Blte("encrypted block: truncated IV".into()));
    }
    let mut iv = [0u8; 8];
    iv[..iv_size].copy_from_slice(&data[pos..pos + iv_size]);
    pos += iv_size;

    // Encryption type.
    if pos >= data.len() {
        return Err(CascError::Blte("encrypted block: missing enc type".into()));
    }
    let enc_type = data[pos];
    pos += 1;

    // XOR the block index into the first 4 bytes of the IV.
    for (i, byte) in iv[..4].iter_mut().enumerate() {
        *byte ^= ((block_idx >> (i * 8)) & 0xFF) as u8;
    }

    // Look up the key.
    let Some(key) = key_service::get_key(key_name) else {
        return Err(CascError::MissingKey(key_name));
    };

    let payload = &data[pos..];

    match enc_type {
        0x53 => {
            // 'S': Salsa20.
            let mut buf = payload.to_vec();
            Salsa20::new(&key, &iv).apply_keystream(&mut buf);
            Ok(Some(buf))
        }
        0x41 => {
            // 'A': ARC4; not implemented.
            Err(CascError::Blte("ARC4 encryption not implemented".into()))
        }
        t => Err(CascError::Blte(format!(
            "unknown encryption type 0x{t:02X}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blte_n(payload: &[u8]) -> Vec<u8> {
        // BLTE with no header, single 'N' block.
        let mut out = Vec::new();
        out.extend_from_slice(&BLTE_MAGIC.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]); // headerSize = 0
        out.push(b'N');
        out.extend_from_slice(payload);
        out
    }

    /// Headerless BLTE holding a single Salsa20-encrypted ('E') block.
    ///
    /// `key_name_bytes` are written in the on-disk order used by D4's
    /// `EncryptedNameDict-0x<ID>.dat` filenames; `decode` reads them back as a
    /// little-endian u64, so the resulting key ID is byte-swapped relative to
    /// the filename hex.  See `research/d4/kmt-memory-scanning.md`.
    fn make_blte_encrypted(key_name_bytes: [u8; 8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&BLTE_MAGIC.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]); // headerSize = 0
        out.push(b'E');
        out.push(8); // key name size
        out.extend_from_slice(&key_name_bytes);
        out.push(4); // IV size
        out.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // IV
        out.push(b'S'); // Salsa20
        out.extend_from_slice(b"ciphertext-we-cannot-read");
        out
    }

    /// Regression guard for issue #2.  An encrypted block whose key we don't
    /// hold must surface as `MissingKey`: never as a storage/IO error, which
    /// is what triggers the (useless) CDN refetch in `CascHandler::open_by_ekey`.
    #[test]
    fn encrypted_block_without_key_returns_missing_key() {
        // Filename hex 0a5015374c0f9fc7 → BLTE lookup ID C79F0F4C3715500A.
        let blte = make_blte_encrypted([0x0A, 0x50, 0x15, 0x37, 0x4C, 0x0F, 0x9F, 0xC7]);

        match decode(&blte, &Md5Hash::default(), false) {
            Err(CascError::MissingKey(id)) => {
                assert_eq!(id, 0xC79F_0F4C_3715_500A, "key name must be read as LE u64");
                // The message must name the key and not imply a network fault.
                let msg = CascError::MissingKey(id).to_string();
                assert!(msg.contains("C79F0F4C3715500A"), "got: {msg}");
                assert!(!msg.to_lowercase().contains("cdn"), "got: {msg}");
            }
            other => panic!("expected MissingKey, got {other:?}"),
        }
    }

    /// The body of an 'E' block encrypted with a key nobody has.
    fn encrypted_block(key_name_bytes: [u8; 8]) -> Vec<u8> {
        make_blte_encrypted(key_name_bytes)[8..].to_vec()
    }

    /// A BLTE file with a block table: `(block bytes, decoded size)` each.
    fn make_blte_framed(blocks: &[(Vec<u8>, u32)]) -> Vec<u8> {
        let header_size = 12 + blocks.len() as u32 * 24;
        let mut out = Vec::new();
        out.extend_from_slice(&BLTE_MAGIC.to_le_bytes());
        out.extend_from_slice(&header_size.to_be_bytes());
        out.push(0x0F);
        out.extend_from_slice(&(blocks.len() as u32).to_be_bytes()[1..]);
        for (bytes, decoded) in blocks {
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            out.extend_from_slice(&decoded.to_be_bytes());
            out.extend_from_slice(&[0u8; 16]); // no per-block hash
        }
        for (bytes, _) in blocks {
            out.extend_from_slice(bytes);
        }
        out
    }

    const KEY_A: [u8; 8] = [0x0A, 0x50, 0x15, 0x37, 0x4C, 0x0F, 0x9F, 0xC7];
    const KEY_A_ID: u64 = 0xC79F_0F4C_3715_500A;

    fn mixed() -> Vec<u8> {
        make_blte_framed(&[
            (b"Nabc".to_vec(), 3),
            (encrypted_block(KEY_A), 5),
            (b"Nxyz".to_vec(), 3),
            (encrypted_block(KEY_A), 2),
        ])
    }

    #[test]
    fn partial_decode_fills_undecryptable_blocks_with_zeros() {
        let got = decode_partial(&mixed(), &Md5Hash::default(), false).unwrap();
        assert_eq!(got.data, b"abc\0\0\0\0\0xyz\0\0");
        assert_eq!(got.missing_keys, [KEY_A_ID], "each key named once");
    }

    #[test]
    fn partial_decode_of_a_readable_file_matches_decode() {
        let blte = make_blte_framed(&[(b"Nabc".to_vec(), 3), (b"Nxyz".to_vec(), 3)]);
        let got = decode_partial(&blte, &Md5Hash::default(), false).unwrap();
        assert_eq!(got.data, decode(&blte, &Md5Hash::default(), false).unwrap());
        assert!(got.missing_keys.is_empty());
    }

    /// The strict decoder must keep refusing: callers rely on `MissingKey`.
    #[test]
    fn decode_still_refuses_a_file_with_an_undecryptable_block() {
        assert!(matches!(
            decode(&mixed(), &Md5Hash::default(), false),
            Err(CascError::MissingKey(KEY_A_ID))
        ));
    }

    /// With no block table there is no size to fill.
    #[test]
    fn partial_decode_cannot_fill_a_file_without_a_block_table() {
        assert!(matches!(
            decode_partial(&make_blte_encrypted(KEY_A), &Md5Hash::default(), false),
            Err(CascError::MissingKey(KEY_A_ID))
        ));
    }

    /// Only a missing key is forgiven; a damaged block is still an error.
    #[test]
    fn partial_decode_still_fails_on_a_broken_block() {
        let blte = make_blte_framed(&[(b"Nabc".to_vec(), 3), (b"Qzzz".to_vec(), 3)]);
        assert!(matches!(
            decode_partial(&blte, &Md5Hash::default(), false),
            Err(CascError::Blte(_))
        ));
    }

    fn make_blte_z(payload: &[u8]) -> Vec<u8> {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;

        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(payload).unwrap();
        let compressed = enc.finish().unwrap();

        // Single-block BLTE with header.
        let num_blocks: u32 = 1;
        let header_size: u32 = 12 + 24;
        let comp_size = (compressed.len() + 1) as u32; // +1 for block-type byte
        let decomp_size = payload.len() as u32;

        let mut out = Vec::new();
        out.extend_from_slice(&BLTE_MAGIC.to_le_bytes());
        out.extend_from_slice(&header_size.to_be_bytes());
        // Flag byte 0x0F followed by 3-byte big-endian block count.
        out.push(0x0F);
        out.push(((num_blocks >> 16) & 0xFF) as u8);
        out.push(((num_blocks >> 8) & 0xFF) as u8);
        out.push((num_blocks & 0xFF) as u8);
        // block descriptor: comp_size, decomp_size, hash (zeros)
        out.extend_from_slice(&comp_size.to_be_bytes());
        out.extend_from_slice(&decomp_size.to_be_bytes());
        out.extend_from_slice(&[0u8; 16]); // hash (skipped when validate=false)
                                           // block data
        out.push(b'Z');
        out.extend_from_slice(&compressed);
        out
    }

    #[test]
    fn raw_block_roundtrip() {
        let payload = b"Hello, CASC!";
        let blte = make_blte_n(payload);
        let key = Md5Hash::default();
        let result = decode(&blte, &key, false).unwrap();
        assert_eq!(&result, payload);
    }

    #[test]
    fn zlib_block_roundtrip() {
        let payload = vec![0xABu8; 1024];
        let blte = make_blte_z(&payload);
        let key = Md5Hash::default();
        let result = decode(&blte, &key, false).unwrap();
        assert_eq!(result, payload);
    }

    #[test]
    fn bad_magic_returns_error() {
        let mut blte = make_blte_n(b"x");
        blte[0] = 0xFF;
        let result = decode(&blte, &Md5Hash::default(), false);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_returns_error() {
        let result = decode(&[0x42, 0x4C, 0x54], &Md5Hash::default(), false);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_block_type_returns_error() {
        let mut blte = make_blte_n(b"x");
        // Overwrite the 'N' type byte with something invalid.
        let n_pos = 8; // right after the 8-byte header
        blte[n_pos] = b'X';
        let result = decode(&blte, &Md5Hash::default(), false);
        assert!(result.is_err());
    }
}
