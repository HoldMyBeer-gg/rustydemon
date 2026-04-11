use std::collections::HashMap;

use crate::{
    error::CascError,
    jenkins96::file_data_id_hash,
    types::{ContentFlags, LocaleFlags, Md5Hash, RootEntry},
};

use super::RootHandler;

const MFST_MAGIC: u32 = 0x4D46_5354; // 'MFST'

/// Root manifest handler for World of Warcraft (all supported manifest versions).
///
/// Parses MFST v0, v1, and v2 as well as the older flat format.
/// Maps FileDataId → Jenkins96 hash → [`RootEntry`].
///
/// The flat `entries_by_hash` map is the foundation of the global search: it
/// contains *every* file in the manifest, even those without a known filename.
pub struct WowRootHandler {
    /// hash → list of RootEntry (one per locale/content-flag variant).
    entries_by_hash: HashMap<u64, Vec<RootEntry>>,
    /// FileDataId → Jenkins96 hash.
    fdid_to_hash: HashMap<u32, u64>,
    /// Jenkins96 hash → FileDataId (reverse).
    hash_to_fdid: HashMap<u64, u32>,
}

impl WowRootHandler {
    /// Parse a BLTE-decoded root file.
    pub fn parse(data: &[u8]) -> Result<Self, CascError> {
        if data.len() < 4 {
            return Err(CascError::InvalidData("root file too short".into()));
        }

        let magic = u32::from_le_bytes(data[..4].try_into().unwrap());
        let is_mfst = magic == MFST_MAGIC;

        // Determine header size and manifest version.
        let (header_size, version) = if is_mfst {
            if data.len() < 12 {
                return Err(CascError::InvalidData("MFST root truncated".into()));
            }
            let hsz = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
            let ver = u32::from_le_bytes(data[8..12].try_into().unwrap());
            if hsz == 0x18 && (ver == 1 || ver == 2) {
                (hsz, ver)
            } else {
                // Version 0: headerSize field actually holds numFilesTotal.
                (12, 0)
            }
        } else {
            (0, u32::MAX) // old flat format
        };

        let mut handler = WowRootHandler {
            entries_by_hash: HashMap::new(),
            fdid_to_hash:    HashMap::new(),
            hash_to_fdid:    HashMap::new(),
        };

        // Flat (pre-MFST) format: fixed 28-byte records.
        if !is_mfst {
            handler.parse_flat(data)?;
            return Ok(handler);
        }

        // MFST format: variable-length blocks.
        handler.parse_mfst(data, header_size, version)?;

        Ok(handler)
    }

    // ── Flat (pre-MFST) format ────────────────────────────────────────────────
    //
    // Each record: cKey(16) + nameHash(8) + fileDataId(4) = 28 bytes.
    // (Some older clients use a slightly different layout; this handles the
    //  most common variant.)

    fn parse_flat(&mut self, data: &[u8]) -> Result<(), CascError> {
        let record_size = 28usize;
        if data.len() % record_size != 0 {
            return Err(CascError::InvalidData(
                "flat root: length is not a multiple of 28".into()
            ));
        }

        let mut off = 0usize;
        while off + record_size <= data.len() {
            let mut ck = [0u8; 16];
            ck.copy_from_slice(&data[off..off+16]);

            let hash = u64::from_le_bytes(data[off+16..off+24].try_into().unwrap());

            let entry = RootEntry {
                ckey:    Md5Hash(ck),
                locale:  LocaleFlags::ALL_WOW,
                content: ContentFlags::NONE,
            };

            self.entries_by_hash.entry(hash).or_default().push(entry);
            off += record_size;
        }

        Ok(())
    }

    // ── MFST v0 / v1 / v2 format ─────────────────────────────────────────────

    fn parse_mfst(
        &mut self,
        data: &[u8],
        header_size: usize,
        version: u32,
    ) -> Result<(), CascError> {
        if data.len() < header_size {
            return Err(CascError::InvalidData("MFST root: truncated header".into()));
        }

        let mut off = header_size;

        while off < data.len() {
            // Each block starts with a count + flags.
            if off + 12 > data.len() { break; }

            let count = u32::from_le_bytes(data[off..off+4].try_into().unwrap()) as usize;
            off += 4;

            let (locale, content) = match version {
                0 | 1 => {
                    let content_raw = u32::from_le_bytes(data[off..off+4].try_into().unwrap());
                    let locale_raw  = u32::from_le_bytes(data[off+4..off+8].try_into().unwrap());
                    off += 8;
                    (
                        LocaleFlags::from_bits_truncate(locale_raw),
                        ContentFlags::from_bits_truncate(content_raw),
                    )
                }
                2 => {
                    let locale_raw   = u32::from_le_bytes(data[off..off+4].try_into().unwrap());
                    let content1     = u32::from_le_bytes(data[off+4..off+8].try_into().unwrap());
                    let content2     = u32::from_le_bytes(data[off+8..off+12].try_into().unwrap());
                    let content3     = if off + 12 < data.len() { data[off+12] } else { 0 };
                    off += 13;
                    (
                        LocaleFlags::from_bits_truncate(locale_raw),
                        ContentFlags::from_bits_truncate(content1 | content2 | ((content3 as u32) << 17)),
                    )
                }
                _ => {
                    // Unknown version — skip.
                    break;
                }
            };

            if locale.is_empty() {
                return Err(CascError::InvalidData(
                    "MFST block has LocaleFlags::NONE".into()
                ));
            }

            // FileDataId deltas (count × i32).
            let fdid_bytes = count * 4;
            if off + fdid_bytes > data.len() { break; }

            let mut file_data_ids = Vec::with_capacity(count);
            let mut fdid_acc = 0i32;
            for i in 0..count {
                let delta = i32::from_le_bytes(data[off+i*4..off+i*4+4].try_into().unwrap());
                fdid_acc += delta;
                file_data_ids.push(fdid_acc as u32);
                fdid_acc += 1;
            }
            off += fdid_bytes;

            // CKeys (count × 16 bytes).
            let ckey_bytes = count * 16;
            if off + ckey_bytes > data.len() { break; }

            let mut ckeys = Vec::with_capacity(count);
            for i in 0..count {
                let mut ck = [0u8; 16];
                ck.copy_from_slice(&data[off+i*16..off+i*16+16]);
                ckeys.push(Md5Hash(ck));
            }
            off += ckey_bytes;

            // Name hashes (count × 8 bytes), absent when NoNameHash is set.
            let has_name_hashes = !content.contains(ContentFlags::NO_NAME_HASH);
            let mut name_hashes: Option<Vec<u64>> = None;

            if has_name_hashes {
                let hash_bytes = count * 8;
                if off + hash_bytes > data.len() { break; }

                let mut hashes = Vec::with_capacity(count);
                for i in 0..count {
                    hashes.push(u64::from_le_bytes(
                        data[off+i*8..off+i*8+8].try_into().unwrap()
                    ));
                }
                off += hash_bytes;
                name_hashes = Some(hashes);
            }

            // Insert into maps.
            for i in 0..count {
                let fdid = file_data_ids[i];
                let hash = match &name_hashes {
                    Some(h) => h[i],
                    None    => file_data_id_hash(fdid),
                };

                let entry = RootEntry {
                    ckey: ckeys[i],
                    locale,
                    content,
                };

                self.entries_by_hash.entry(hash).or_default().push(entry);
                self.fdid_to_hash.entry(fdid).or_insert(hash);
                self.hash_to_fdid.entry(hash).or_insert(fdid);
            }
        }

        Ok(())
    }
}

impl RootHandler for WowRootHandler {
    fn count(&self) -> usize { self.entries_by_hash.len() }

    fn get_all_entries(&self, hash: u64) -> &[RootEntry] {
        self.entries_by_hash.get(&hash).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_> {
        Box::new(
            self.entries_by_hash
                .iter()
                .flat_map(|(&h, v)| v.iter().map(move |e| (h, e)))
        )
    }

    fn hash_for_file_data_id(&self, id: u32) -> Option<u64> {
        self.fdid_to_hash.get(&id).copied()
    }

    fn file_data_id_for_hash(&self, hash: u64) -> Option<u32> {
        self.hash_to_fdid.get(&hash).copied()
    }
}
