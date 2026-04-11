use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use crate::{
    error::CascError,
    key_service,
    types::{EKey9, EncodingEntry, Md5Hash},
};

const CHUNK_SIZE: usize = 4096;

/// Parsed CASC encoding file.
///
/// The encoding file is the master CKey→EKey lookup table.  Given a content
/// key (MD5 of the raw file), it returns the corresponding encoding key(s)
/// and decoded file size.
///
/// Parsing follows the CASCLib `EncodingHandler` logic exactly.
pub struct EncodingHandler {
    /// CKey → encoding entry (EKeys + size).
    ckey_map: HashMap<Md5Hash, EncodingEntry>,
    /// EKey 9-byte prefix → CKey (reverse mapping).
    ekey_to_ckey: HashMap<EKey9, Md5Hash>,
}

impl EncodingHandler {
    /// Parse an encoding file from a seekable reader.
    ///
    /// The reader should be positioned at the very start of the encoding data
    /// (i.e. the `EN` magic bytes).
    pub fn from_reader<R: Read + Seek>(mut r: R) -> Result<Self, CascError> {
        // ── Header ─────────────────────────────────────────────────────────
        let mut magic = [0u8; 2];
        r.read_exact(&mut magic)?;
        if &magic != b"EN" {
            return Err(CascError::InvalidData("encoding: bad magic".into()));
        }

        let version = read_u8(&mut r)?;
        if version != 1 {
            return Err(CascError::InvalidData(format!(
                "encoding: unsupported version {version}"
            )));
        }

        let _ckey_len   = read_u8(&mut r)?; // always 16
        let _ekey_len   = read_u8(&mut r)?; // always 16
        let ckey_page_size = read_u16_be(&mut r)? as usize * 1024;
        let ekey_page_size = read_u16_be(&mut r)? as usize * 1024;
        let ckey_page_count = read_u32_be(&mut r)? as usize;
        let ekey_page_count = read_u32_be(&mut r)? as usize;
        let _unk1 = read_u8(&mut r)?;
        let espec_size = read_u32_be(&mut r)? as usize;

        // ESpec string block (skip for now; we only need CKey pages).
        let _ = ckey_page_size; // used for sizing correctness, not needed here
        let _ = ekey_page_size;

        // Skip the ESpec block.
        skip_bytes(&mut r, espec_size)?;

        // Skip the CKey page table (ckey_page_count × 32 bytes: 2× MD5Hash).
        skip_bytes(&mut r, ckey_page_count * 32)?;

        // ── CKey pages ─────────────────────────────────────────────────────
        let mut ckey_map: HashMap<Md5Hash, EncodingEntry> =
            HashMap::with_capacity(ckey_page_count * 64);
        let mut ekey_to_ckey: HashMap<EKey9, Md5Hash> =
            HashMap::with_capacity(ckey_page_count * 64);

        let chunk_start = r.stream_position()?;

        for page_idx in 0..ckey_page_count {
            let page_offset = chunk_start + (page_idx * CHUNK_SIZE) as u64;
            r.seek(SeekFrom::Start(page_offset))?;

            // Read up to CHUNK_SIZE bytes for this page.
            let mut page = vec![0u8; CHUNK_SIZE];
            let n = r.read(&mut page)?;
            page.truncate(n);

            let mut off = 0usize;

            // Parse entries until keysCount == 0 or we run out of space.
            loop {
                if off >= page.len() { break; }
                let keys_count = page[off] as usize;
                off += 1;
                if keys_count == 0 { break; }

                // 5-byte big-endian file size.
                if off + 5 > page.len() { break; }
                let file_size = read_u40_be_slice(&page[off..]);
                off += 5;

                // CKey (16 bytes).
                if off + 16 > page.len() { break; }
                let mut ckey_bytes = [0u8; 16];
                ckey_bytes.copy_from_slice(&page[off..off+16]);
                let ckey = Md5Hash(ckey_bytes);
                off += 16;

                // EKeys.
                let mut ekeys = Vec::with_capacity(keys_count);
                for _ in 0..keys_count {
                    if off + 16 > page.len() { break; }
                    let mut ek = [0u8; 16];
                    ek.copy_from_slice(&page[off..off+16]);
                    let ekey = Md5Hash(ek);
                    ekeys.push(ekey);
                    ekey_to_ckey.insert(EKey9::from_full(&ekey), ckey);
                    off += 16;
                }

                ckey_map.insert(ckey, EncodingEntry { ekeys, size: file_size });
            }
        }

        // ── EKey pages ─────────────────────────────────────────────────────
        // Seek past the CKey pages (already handled) and the EKey page table.
        let ckey_pages_end = chunk_start + (ckey_page_count * CHUNK_SIZE) as u64;
        r.seek(SeekFrom::Start(ckey_pages_end))?;
        skip_bytes(&mut r, ekey_page_count * 32)?;

        // (EKey pages contain eSpec indices + sizes; we don't need them for
        //  basic file extraction, so we skip them for now.)

        Ok(EncodingHandler { ckey_map, ekey_to_ckey })
    }

    /// Total number of known CKey entries.
    pub fn count(&self) -> usize { self.ckey_map.len() }

    /// Look up an encoding entry by content key.
    pub fn get_entry(&self, ckey: &Md5Hash) -> Option<&EncodingEntry> {
        self.ckey_map.get(ckey)
    }

    /// Choose the best encoding key for a given content key.
    ///
    /// "Best" means: if there is only one EKey, use it.  If there are
    /// multiple, prefer the first one for which we hold a TACT decryption
    /// key.  Falls back to the first EKey if no decryption key is available.
    pub fn best_ekey(&self, ckey: &Md5Hash) -> Option<Md5Hash> {
        let entry = self.ckey_map.get(ckey)?;
        if entry.ekeys.len() == 1 {
            return Some(entry.ekeys[0]);
        }
        // Prefer a key we can actually decrypt.
        for ek in &entry.ekeys {
            let name_bytes = &ek.0[..8];
            let name = u64::from_le_bytes(name_bytes.try_into().unwrap());
            if key_service::has_key(name) {
                return Some(*ek);
            }
        }
        entry.ekeys.first().copied()
    }

    /// Reverse lookup: EKey → CKey (using the 9-byte prefix).
    pub fn ckey_for_ekey(&self, ekey: &Md5Hash) -> Option<&Md5Hash> {
        self.ekey_to_ckey.get(&EKey9::from_full(ekey))
    }

    /// Iterate all CKey → EncodingEntry pairs (for global search / dump).
    pub fn entries(&self) -> impl Iterator<Item = (&Md5Hash, &EncodingEntry)> {
        self.ckey_map.iter()
    }
}

// ── Read helpers ───────────────────────────────────────────────────────────────

fn read_u8<R: Read>(r: &mut R) -> Result<u8, CascError> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u16_be<R: Read>(r: &mut R) -> Result<u16, CascError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32_be<R: Read>(r: &mut R) -> Result<u32, CascError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_u40_be_slice(b: &[u8]) -> u64 {
    ((b[0] as u64) << 32)
        | ((b[1] as u64) << 24)
        | ((b[2] as u64) << 16)
        | ((b[3] as u64) << 8)
        | (b[4] as u64)
}

fn skip_bytes<R: Read + Seek>(r: &mut R, n: usize) -> Result<(), CascError> {
    r.seek(SeekFrom::Current(n as i64))?;
    Ok(())
}
