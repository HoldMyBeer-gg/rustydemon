//! Root handler for Messiah engine MPK packages (Diablo Immortal).
//!
//! MPK has no content keys, so each entry gets a synthetic one: the bytes
//! `MPK\0` followed by the entry's index in the archive.  The handler's
//! storage side turns that back into an index with [`entry_index`].

use std::collections::HashMap;

use rustydemon_mpk::MpkArchive;

use crate::{
    jenkins96::jenkins96,
    types::{ContentFlags, LocaleFlags, Md5Hash, RootEntry},
};

use super::RootHandler;

const KEY_TAG: [u8; 4] = *b"MPK\0";

/// Synthetic content key for MPK entry `index`.
pub fn key_for(index: usize) -> Md5Hash {
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&KEY_TAG);
    b[4..12].copy_from_slice(&(index as u64).to_le_bytes());
    Md5Hash(b)
}

/// Entry index carried by a key from [`key_for`], or `None` for any other key.
pub fn entry_index(key: &Md5Hash) -> Option<usize> {
    if key.0[..4] != KEY_TAG {
        return None;
    }
    usize::try_from(u64::from_le_bytes(key.0[4..12].try_into().ok()?)).ok()
}

pub struct MpkRootHandler {
    entries_by_hash: HashMap<u64, Vec<RootEntry>>,
    file_paths: HashMap<u64, String>,
}

impl MpkRootHandler {
    pub fn new(archive: &MpkArchive) -> Self {
        let mut entries_by_hash: HashMap<u64, Vec<RootEntry>> = HashMap::new();
        let mut file_paths = HashMap::new();
        for (i, e) in archive.entries().iter().enumerate() {
            let hash = jenkins96(&e.path);
            entries_by_hash.entry(hash).or_default().push(RootEntry {
                ckey: key_for(i),
                locale: LocaleFlags::ALL,
                content: ContentFlags::NONE,
            });
            file_paths.entry(hash).or_insert_with(|| e.path.clone());
        }
        Self {
            entries_by_hash,
            file_paths,
        }
    }
}

impl RootHandler for MpkRootHandler {
    fn count(&self) -> usize {
        self.entries_by_hash.len()
    }

    fn get_all_entries(&self, hash: u64) -> &[RootEntry] {
        self.entries_by_hash
            .get(&hash)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_> {
        Box::new(
            self.entries_by_hash
                .iter()
                .flat_map(|(&h, v)| v.iter().map(move |e| (h, e))),
        )
    }

    fn hash_for_file_data_id(&self, _id: u32) -> Option<u64> {
        None
    }

    fn file_data_id_for_hash(&self, _hash: u64) -> Option<u32> {
        None
    }

    fn builtin_paths(&self) -> Vec<(u64, String)> {
        self.file_paths
            .iter()
            .map(|(&h, p)| (h, p.clone()))
            .collect()
    }

    fn has_builtin_paths(&self) -> bool {
        !self.file_paths.is_empty()
    }

    fn type_name(&self) -> &'static str {
        "MPK"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        assert_eq!(entry_index(&key_for(737_192)), Some(737_192));
        assert_eq!(entry_index(&Md5Hash([0xAB; 16])), None);
    }
}
