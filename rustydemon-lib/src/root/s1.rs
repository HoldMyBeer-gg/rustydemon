//! StarCraft Remastered (S1) text-format root manifest.
//!
//! SC1R's root is a plain ASCII file, one entry per line:
//!
//! ```text
//! path[:LOCALE]|hexmd5
//! ```
//!
//! Ported from CascLib's `S1RootHandler.cs`.

use std::collections::HashMap;

use crate::{
    error::CascError,
    jenkins96::jenkins96,
    types::{ContentFlags, LocaleFlags, Md5Hash, RootEntry},
};

use super::RootHandler;

pub struct S1RootHandler {
    entries_by_hash: HashMap<u64, Vec<RootEntry>>,
    file_paths: HashMap<u64, String>,
}

impl S1RootHandler {
    pub fn parse(data: &[u8]) -> Result<Self, CascError> {
        let text = std::str::from_utf8(data)
            .map_err(|e| CascError::InvalidData(format!("S1 root is not UTF-8: {e}")))?;

        let mut entries_by_hash: HashMap<u64, Vec<RootEntry>> = HashMap::new();
        let mut file_paths: HashMap<u64, String> = HashMap::new();

        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(2, '|');
            let name_part = match parts.next() {
                Some(p) => p,
                None => continue,
            };
            let hex = match parts.next() {
                Some(p) => p.trim(),
                None => continue,
            };

            let (file, locale) = match name_part.split_once(':') {
                Some((f, loc)) => (f, parse_locale(loc)),
                None => (name_part, LocaleFlags::ALL),
            };

            let ckey_bytes = match hex_to_bytes(hex) {
                Some(b) => b,
                None => continue,
            };

            let hash = jenkins96(file);
            file_paths.insert(hash, file.to_string());
            entries_by_hash.entry(hash).or_default().push(RootEntry {
                ckey: Md5Hash::from_bytes(ckey_bytes),
                locale,
                content: ContentFlags::NONE,
            });
        }

        if entries_by_hash.is_empty() {
            return Err(CascError::InvalidData("S1 root produced no entries".into()));
        }

        Ok(S1RootHandler {
            entries_by_hash,
            file_paths,
        })
    }

    /// Quick check: does this blob look like an S1 text root?
    pub fn looks_like_s1_root(data: &[u8]) -> bool {
        // Sample up to the first 2KB — must be printable ASCII with at least
        // one line containing a `|` separator.
        let sample = &data[..data.len().min(2048)];
        if !sample
            .iter()
            .all(|&b| b == b'\n' || b == b'\r' || b == b'\t' || (0x20..=0x7E).contains(&b))
        {
            return false;
        }
        sample.contains(&b'|')
    }
}

fn hex_to_bytes(hex: &str) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn parse_locale(s: &str) -> LocaleFlags {
    // Accept both comma-separated flag names and bare codes.
    let mut flags = LocaleFlags::empty();
    for tok in s.split(|c: char| c == ',' || c == '|' || c.is_whitespace()) {
        match tok.to_ascii_lowercase().as_str() {
            "" => {}
            "all" => return LocaleFlags::ALL,
            "enus" => flags |= LocaleFlags::EN_US,
            "kokr" => flags |= LocaleFlags::KO_KR,
            "frfr" => flags |= LocaleFlags::FR_FR,
            "dede" => flags |= LocaleFlags::DE_DE,
            "zhcn" => flags |= LocaleFlags::ZH_CN,
            "eses" => flags |= LocaleFlags::ES_ES,
            "zhtw" => flags |= LocaleFlags::ZH_TW,
            "engb" => flags |= LocaleFlags::EN_GB,
            "esmx" => flags |= LocaleFlags::ES_MX,
            "ruru" => flags |= LocaleFlags::RU_RU,
            "ptbr" => flags |= LocaleFlags::PT_BR,
            "itit" => flags |= LocaleFlags::IT_IT,
            "ptpt" => flags |= LocaleFlags::PT_PT,
            _ => {}
        }
    }
    if flags.is_empty() {
        LocaleFlags::ALL
    } else {
        flags
    }
}

impl RootHandler for S1RootHandler {
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
        "S1 (text)"
    }
}
