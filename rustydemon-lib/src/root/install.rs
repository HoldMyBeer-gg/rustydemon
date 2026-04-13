//! INSTALL manifest root handler.
//!
//! Used as a fallback for games whose main root manifest is not recognised
//! (e.g. StarCraft Remastered). Every CASC installation has an INSTALL
//! manifest that lists installed files with their content keys.
//!
//! Binary format:
//! - Header: "IN" (2B), version (u8), hash_size (u8), num_tags (u16 BE), num_entries (u32 BE)
//! - Tags: for each tag — name (C-string), type (u16 BE), bitmask bytes
//! - Entries: for each entry — name (C-string), ckey (hash_size B), size (u32 BE)

use std::collections::HashMap;
use std::io::{Cursor, Read};

use crate::{
    error::CascError,
    jenkins96::jenkins96,
    types::{ContentFlags, LocaleFlags, Md5Hash, RootEntry},
};

use super::RootHandler;

pub struct InstallRootHandler {
    entries_by_hash: HashMap<u64, Vec<RootEntry>>,
    file_paths: HashMap<u64, String>,
}

impl InstallRootHandler {
    pub fn parse(data: &[u8]) -> Result<Self, CascError> {
        if data.len() < 10 || &data[..2] != b"IN" {
            return Err(CascError::InvalidData(
                "INSTALL: missing 'IN' signature".into(),
            ));
        }

        let mut r = Cursor::new(data);
        let mut sig = [0u8; 2];
        r.read_exact(&mut sig)?;

        let mut version_b = [0u8; 1];
        r.read_exact(&mut version_b)?;
        let _version = version_b[0];

        let mut hash_size_b = [0u8; 1];
        r.read_exact(&mut hash_size_b)?;
        let hash_size = hash_size_b[0] as usize;
        if hash_size != 16 {
            return Err(CascError::InvalidData(format!(
                "INSTALL: unsupported hash size {hash_size}"
            )));
        }

        let mut u16_buf = [0u8; 2];
        r.read_exact(&mut u16_buf)?;
        let num_tags = u16::from_be_bytes(u16_buf) as usize;

        let mut u32_buf = [0u8; 4];
        r.read_exact(&mut u32_buf)?;
        let num_entries = u32::from_be_bytes(u32_buf) as usize;

        // Skip tags.
        let bitmask_bytes = num_entries.div_ceil(8);
        for _ in 0..num_tags {
            read_cstring(&mut r)?;
            let mut ty = [0u8; 2];
            r.read_exact(&mut ty)?;
            let mut bits = vec![0u8; bitmask_bytes];
            r.read_exact(&mut bits)?;
        }

        // Read entries.
        let mut entries_by_hash: HashMap<u64, Vec<RootEntry>> = HashMap::new();
        let mut file_paths: HashMap<u64, String> = HashMap::new();

        for _ in 0..num_entries {
            let name = read_cstring(&mut r)?;
            let mut ckey_bytes = [0u8; 16];
            r.read_exact(&mut ckey_bytes)?;
            let mut size_buf = [0u8; 4];
            r.read_exact(&mut size_buf)?;
            let _size = u32::from_be_bytes(size_buf);

            let hash = jenkins96(&name);
            file_paths.insert(hash, name);
            entries_by_hash.entry(hash).or_default().push(RootEntry {
                ckey: Md5Hash::from_bytes(ckey_bytes),
                locale: LocaleFlags::ALL,
                content: ContentFlags::NONE,
            });
        }

        Ok(InstallRootHandler {
            entries_by_hash,
            file_paths,
        })
    }
}

fn read_cstring<R: Read>(r: &mut R) -> Result<String, CascError> {
    let mut bytes = Vec::new();
    let mut b = [0u8; 1];
    loop {
        r.read_exact(&mut b)?;
        if b[0] == 0 {
            break;
        }
        bytes.push(b[0]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl RootHandler for InstallRootHandler {
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
        "INSTALL"
    }
}
