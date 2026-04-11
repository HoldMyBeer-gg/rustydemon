use std::{
    collections::HashMap,
    fs,
    io::{BufReader, Read},
    path::Path,
};

use crate::{
    error::CascError,
    types::{EKey9, IndexEntry, Md5Hash},
};

/// Local CASC index handler.
///
/// Reads all `NN*.idx` files from the game's data directory and builds a map
/// from 9-byte EKey prefix → [`IndexEntry`] (archive number + offset + size).
pub struct LocalIndexHandler {
    /// 9-byte EKey prefix → location in local data archives.
    index: HashMap<EKey9, IndexEntry>,
}

impl LocalIndexHandler {
    /// Load all local index files found in `data_dir` (e.g. `<game>/Data/data/`).
    ///
    /// CASC stores up to 16 archive series (`00*.idx` … `0F*.idx`).  We pick
    /// the most-recently-modified file in each series.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self, CascError> {
        let data_dir = data_dir.as_ref();

        let mut idx_files: Vec<std::path::PathBuf> = Vec::new();

        for series in 0x00u8..=0x0Fu8 {
            let prefix = format!("{series:02x}");

            // Enumerate *.idx files matching the series prefix.
            let mut candidates: Vec<std::path::PathBuf> = fs::read_dir(data_dir)?
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "idx")
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with(&prefix))
                })
                .collect();

            // Sort so the last entry is the newest by name (version number).
            candidates.sort();

            if let Some(latest) = candidates.last() {
                idx_files.push(latest.clone());
            }
        }

        if idx_files.is_empty() {
            return Err(CascError::FileNotFound(format!(
                "no *.idx files found in {}",
                data_dir.display()
            )));
        }

        let mut handler = LocalIndexHandler {
            index: HashMap::new(),
        };

        for path in &idx_files {
            handler.parse_index(path)?;
        }

        Ok(handler)
    }

    /// Number of indexed entries.
    pub fn count(&self) -> usize {
        self.index.len()
    }

    /// Look up an encoding key, returning its archive location (or `None`).
    pub fn get_entry(&self, ekey: &Md5Hash) -> Option<&IndexEntry> {
        self.index.get(&EKey9::from_full(ekey))
    }

    // ── Parsing ───────────────────────────────────────────────────────────────

    fn parse_index(&mut self, path: &Path) -> Result<(), CascError> {
        let file = fs::File::open(path)?;
        let mut r = BufReader::new(file);

        // ── Header ────────────────────────────────────────────────────────
        let header_hash_size = read_u32_le(&mut r)? as usize;
        let _header_hash = read_u32_le(&mut r)?;

        // Skip h2 bytes + alignment to 16-byte boundary.
        let after_h2 = 8 + header_hash_size;
        let padded = (after_h2 + 0x0F) & !0x0F;
        skip_bytes(&mut r, padded - 8)?; // we already read 8 bytes

        let entries_size = read_u32_le(&mut r)? as usize;
        let _entries_hash = read_u32_le(&mut r)?;

        if !entries_size.is_multiple_of(18) {
            return Err(CascError::InvalidData(format!(
                "idx entries_size {entries_size} is not a multiple of 18"
            )));
        }

        let num_entries = entries_size / 18;

        // ── Entries ───────────────────────────────────────────────────────
        for _ in 0..num_entries {
            // 9-byte key prefix.
            let mut key9 = [0u8; 9];
            r.read_exact(&mut key9)?;

            // index: 1-byte high + 4-byte big-endian low.
            let index_high = read_u8(&mut r)?;
            let index_low = read_u32_be(&mut r)?;

            let archive_index = ((index_high as u32) << 2) | ((index_low & 0xC000_0000) >> 30);
            let offset = index_low & 0x3FFF_FFFF;

            let size = read_u32_le(&mut r)?;

            let entry = IndexEntry {
                index: archive_index,
                offset,
                size,
            };

            // Pad the 9-byte key to 16 bytes for EKey9 construction.
            let mut padded16 = [0u8; 16];
            padded16[..9].copy_from_slice(&key9);
            let ekey9 = EKey9::from_full(&Md5Hash(padded16));

            // Only record the first occurrence (matches CASCLib behaviour).
            self.index.entry(ekey9).or_insert(entry);
        }

        Ok(())
    }
}

// ── Read helpers ───────────────────────────────────────────────────────────────

fn read_u8<R: Read>(r: &mut R) -> Result<u8, CascError> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u32_le<R: Read>(r: &mut R) -> Result<u32, CascError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u32_be<R: Read>(r: &mut R) -> Result<u32, CascError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn skip_bytes<R: Read>(r: &mut R, n: usize) -> Result<(), CascError> {
    let mut remaining = n;
    let mut buf = [0u8; 256];
    while remaining > 0 {
        let take = remaining.min(buf.len());
        r.read_exact(&mut buf[..take])?;
        remaining -= take;
    }
    Ok(())
}
