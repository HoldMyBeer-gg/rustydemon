//! Deep-search plug-in for Diablo IV `.pow` (Power/Skill) binary files.
//!
//! The format overview (derived from `d4builder/tools/pow_to_json.py`):
//!
//! ```text
//! +0x00  magic       u32 LE  = 0xDEADBEEF
//! +0x10  power_id    u32 LE
//! +0x64  hash        u32 LE
//! +0x80  section_table   4 × (offset u32, size u32)
//!            [0]  sf_definitions
//!            [1]  helper_formulas
//!            [2]  payload_data
//!            [3]  scaling_tables
//! ```
//!
//! The searcher extracts SF definition names and formula strings, then
//! filters them by the query substring.

use super::{ContentMatch, ContentSearcher};

const POW_MAGIC: u32 = 0xDEAD_BEEF;
const SECTION_TABLE_OFFSET: usize = 0x80;
const NUM_SECTIONS: usize = 4;

pub struct PowSearcher;

impl ContentSearcher for PowSearcher {
    fn can_search(&self, filename: &str) -> bool {
        filename.to_lowercase().ends_with(".pow")
    }

    fn format_name(&self) -> &str {
        ".pow (D4 skill)"
    }

    fn search(&self, data: &[u8], query: &str) -> Vec<ContentMatch> {
        parse_pow(data, query).unwrap_or_default()
    }
}

// ── Parser ─────────────────────────────────────────────────────────────────────

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn parse_pow(data: &[u8], query: &str) -> Result<Vec<ContentMatch>, &'static str> {
    if data.len() < SECTION_TABLE_OFFSET + NUM_SECTIONS * 8 {
        return Err("too short");
    }

    let magic = read_u32_le(data, 0).ok_or("read magic")?;
    if magic != POW_MAGIC {
        return Err("bad magic");
    }

    let power_id = read_u32_le(data, 0x10).ok_or("read power_id")?;
    let needle = query.to_lowercase();

    // Section table: 4 × (offset u32 LE, size u32 LE)
    let mut sections = [(0usize, 0usize); NUM_SECTIONS];
    for (i, sect) in sections.iter_mut().enumerate() {
        let base = SECTION_TABLE_OFFSET + i * 8;
        let off = read_u32_le(data, base).ok_or("read section offset")? as usize;
        let sz = read_u32_le(data, base + 4).ok_or("read section size")? as usize;
        *sect = (off, sz);
    }

    let mut matches = Vec::new();

    // ── Section 0: SF_N definitions (null-terminated ASCII strings) ────────────
    let (sf_off, sf_sz) = sections[0];
    if sf_off > 0 && sf_sz > 0 {
        if let Some(sf_data) = data.get(sf_off..sf_off + sf_sz) {
            for name in split_c_strings(sf_data) {
                if needle.is_empty() || name.to_lowercase().contains(&needle) {
                    matches.push(ContentMatch {
                        inner_path: format!("sf_def/{name}"),
                        offset: Some((sf_off) as u64),
                        kind: "SF definition".into(),
                    });
                }
            }
        }
    }

    // ── Section 1: helper/damage formula strings ───────────────────────────────
    let (frm_off, frm_sz) = sections[1];
    if frm_off > 0 && frm_sz > 0 {
        if let Some(frm_data) = data.get(frm_off..frm_off + frm_sz) {
            for formula in split_c_strings(frm_data) {
                if needle.is_empty() || formula.to_lowercase().contains(&needle) {
                    let kind = classify_formula(&formula);
                    matches.push(ContentMatch {
                        inner_path: format!("formula/{formula}"),
                        offset: Some(frm_off as u64),
                        kind: kind.into(),
                    });
                }
            }
        }
    }

    // Add a top-level entry for the power itself when searching with no filter
    // or when the power_id matches the query.
    let id_str = format!("{power_id}");
    if needle.is_empty() || id_str.contains(&needle) {
        matches.insert(
            0,
            ContentMatch {
                inner_path: format!("power/{power_id}"),
                offset: Some(0x10),
                kind: "power id".into(),
            },
        );
    }

    Ok(matches)
}

/// Classify a formula string by its damage table reference.
fn classify_formula(formula: &str) -> &'static str {
    if formula.contains("Table(34,") {
        "damage formula"
    } else if formula.contains("Table(35,") {
        "cooldown formula"
    } else {
        "formula"
    }
}

/// Split a byte slice on null bytes, returning non-empty valid UTF-8 strings.
fn split_c_strings(data: &[u8]) -> impl Iterator<Item = String> + '_ {
    data.split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok())
        .filter(|s| s.chars().all(|c| c.is_ascii_graphic() || c == ' '))
        .map(|s| s.to_string())
}
