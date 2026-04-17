use std::io::Read;

use flate2::read::ZlibDecoder;
use md5::{Digest, Md5};

use crate::{error::CascError, key_service, salsa20::Salsa20, types::Md5Hash};

// Safety limits to prevent decompression bombs.
const MAX_BLTE_BYTES: usize = 512 * 1024 * 1024; // 512 MiB

const BLTE_MAGIC: u32 = 0x4554_4C42; // 'BLTE' LE

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

        decode_block(block_data, idx, &mut out)?;
    }

    Ok(out)
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
            // 'N' — not compressed, raw copy.
            out.extend_from_slice(&data[1..]);
        }
        0x5A => {
            // 'Z' — zlib/deflate compressed.
            let mut dec = ZlibDecoder::new(&data[1..]);
            let mut buf = Vec::new();
            dec.read_to_end(&mut buf)
                .map_err(|e| CascError::Blte(format!("block {block_idx} zlib error: {e}")))?;
            out.extend_from_slice(&buf);
        }
        0x45 => {
            // 'E' — encrypted; decrypt then recurse.
            let decrypted = decrypt_block(&data[1..], block_idx)?;
            if let Some(dec) = decrypted {
                decode_block(&dec, block_idx, out)?;
            } else {
                // Key unknown — emit zeros for the expected size.
                // (Caller may handle this differently if desired.)
            }
        }
        0x46 => {
            // 'F' — frame (recursive BLTE); not implemented.
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
            // 'S' — Salsa20.
            let mut buf = payload.to_vec();
            Salsa20::new(&key, &iv).apply_keystream(&mut buf);
            Ok(Some(buf))
        }
        0x41 => {
            // 'A' — ARC4; not implemented.
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
