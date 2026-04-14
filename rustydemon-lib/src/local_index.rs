use std::{
    collections::HashMap,
    fs,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use crate::{
    error::CascError,
    types::{EKey9, IndexEntry, Md5Hash},
};

/// Local CASC index handler.
///
/// Reads `NN*.idx` files from one or more data directories and builds a
/// unified map from 9-byte EKey prefix → [`IndexEntry`] (archive number +
/// offset + size + storage id).  Each entry is tagged with the index of
/// the storage it came from, so [`read_block`](Self::read_block) can route
/// reads to the correct `data.NNN` file.
///
/// Single-storage games (WoW, Overwatch, etc.) use the [`load`](Self::load)
/// shortcut.  D2R 3.1.2 and newer split their CASC across a **primary**
/// `Data/data/` storage (game assets) and a **secondary** `Data/ecache/`
/// storage (loose metadata like ENCODING, DOWNLOAD, root manifests, TVFS
/// tables) — for those you call [`load_multi`](Self::load_multi) with both
/// directories and the handler transparently resolves lookups to whichever
/// storage owns the ekey.
pub struct LocalIndexHandler {
    /// Data directories, in the order they were passed to [`load_multi`].
    /// [`IndexEntry::storage`] indexes into this vector.
    storages: Vec<PathBuf>,
    /// 9-byte EKey prefix → location in local data archives.
    index: HashMap<EKey9, IndexEntry>,
}

impl LocalIndexHandler {
    /// Load a single storage directory.
    ///
    /// Convenience wrapper around [`load_multi`] for games that only have
    /// one CASC storage (everything except D2R 3.1.2+).
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Self, CascError> {
        Self::load_multi(&[data_dir.as_ref()])
    }

    /// Load one or more storage directories and merge their indices.
    ///
    /// Skips directories that don't exist or contain no `.idx` files, so
    /// you can pass `&[primary, ecache]` unconditionally and the ecache
    /// slot is simply omitted when absent.  Returns an error only when
    /// **none** of the given directories yield any usable idx files.
    ///
    /// Entries from later directories are tagged with their storage index
    /// (0 for primary, 1 for secondary, …) so subsequent reads know which
    /// `data.NNN` file to open.  If the same ekey appears in multiple
    /// storages only the first occurrence wins, matching CASCLib's
    /// first-seen behaviour.
    pub fn load_multi(data_dirs: &[&Path]) -> Result<Self, CascError> {
        let mut handler = LocalIndexHandler {
            storages: Vec::with_capacity(data_dirs.len()),
            index: HashMap::new(),
        };

        for dir in data_dirs {
            let files = match collect_idx_files(dir) {
                Ok(files) if !files.is_empty() => files,
                _ => continue, // non-existent or empty — skip without error
            };

            let storage_id = handler.storages.len() as u8;
            handler.storages.push(dir.to_path_buf());

            for path in &files {
                handler.parse_index(path, storage_id)?;
            }
        }

        if handler.storages.is_empty() {
            return Err(CascError::FileNotFound(format!(
                "no *.idx files found in any of: {}",
                data_dirs
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        Ok(handler)
    }

    /// Number of indexed entries across all storages.
    pub fn count(&self) -> usize {
        self.index.len()
    }

    /// Directories that contributed entries, in storage-id order.
    pub fn storages(&self) -> &[PathBuf] {
        &self.storages
    }

    /// Look up an encoding key, returning its archive location (or `None`).
    pub fn get_entry(&self, ekey: &Md5Hash) -> Option<&IndexEntry> {
        self.index.get(&EKey9::from_full(ekey))
    }

    /// Resolve an ekey to its on-disk archive path: `<storage>/data.NNN`.
    pub fn archive_path_for(&self, entry: &IndexEntry) -> Option<PathBuf> {
        let dir = self.storages.get(entry.storage as usize)?;
        Some(dir.join(format!("data.{:03}", entry.index)))
    }

    /// Fetch the raw BLTE-ready block bytes for `ekey` from whichever
    /// storage owns it.
    ///
    /// Hides all the multi-storage routing from callers — they only need
    /// the ekey.  The 30-byte `data.NNN` block header is stripped so the
    /// returned bytes can be fed straight to [`crate::blte::decode`].
    pub fn read_block(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let entry = self
            .get_entry(ekey)
            .ok_or_else(|| CascError::IndexNotFound(ekey.to_hex()))?;
        let dir = self.storages.get(entry.storage as usize).ok_or_else(|| {
            CascError::InvalidData(format!(
                "ekey {} references unknown storage id {}",
                ekey.to_hex(),
                entry.storage
            ))
        })?;
        crate::handler::read_data_block(dir, entry.index, entry.offset, entry.size)
    }

    // ── Parsing ───────────────────────────────────────────────────────────────

    fn parse_index(&mut self, path: &Path, storage_id: u8) -> Result<(), CascError> {
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
                storage: storage_id,
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

/// Find `NN*.idx` files in one data directory, returning the newest
/// (highest version suffix) in each of the 16 series.
fn collect_idx_files(data_dir: &Path) -> Result<Vec<PathBuf>, CascError> {
    let mut out: Vec<PathBuf> = Vec::new();

    if !data_dir.is_dir() {
        return Ok(out);
    }

    for series in 0x00u8..=0x0Fu8 {
        let prefix = format!("{series:02x}");

        let mut candidates: Vec<PathBuf> = fs::read_dir(data_dir)?
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
            out.push(latest.clone());
        }
    }

    Ok(out)
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
