//! Lightweight .pow file parser for preview display.
//!
//! Extracts header info, SF_ scaling factors, and formula strings from
//! Diablo 4 power files. SF_ values are positionally indexed in the
//! ptScriptFormulas array: SF_0 = entry[0], SF_5 = entry[5], etc.
//!
//! Binary layout (discovered via binary analysis + d4data definitions):
//! - 0x00: magic (0xDEADBEEF)
//! - 0x10: power_id (u32)
//! - 0x78: struct_size (u32) — PowerDefinition fixed struct size (typically 3232)
//! - struct starts at 0x10, variable data follows at 0x10 + struct_size
//! - First formula descriptor at struct+0x0258 points to contiguous formula data
//! - Each formula entry: [text (4-byte aligned)][12-byte bytecode (type_tag + value)]

use std::collections::HashMap;
use std::fmt::Write;

const POW_MAGIC: u32 = 0xDEADBEEF;

/// Parsed .pow preview data for display.
pub struct PowPreview {
    pub power_id: u32,
    pub file_size: usize,
    pub magic_ok: bool,
    /// SF values indexed by number, extracted from the formula data block.
    pub sf_values: HashMap<u32, String>,
    pub formulas: Vec<Formula>,
}

pub struct TypedValue {
    pub kind: &'static str,
    pub display: String,
}

pub struct Formula {
    pub text: String,
    pub classification: &'static str,
    pub offset: usize,
    /// Inline typed values parsed after the formula string.
    pub values: Vec<TypedValue>,
    /// SF_ references found in the formula text.
    pub sf_refs: Vec<String>,
    /// Extracted numeric coefficient (e.g. 1.75 from "1.75 * Table(34,...)").
    pub coefficient: Option<f64>,
    /// Table ID if present (34 = damage, 35 = cooldown).
    pub table_id: Option<u32>,
}

fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

impl PowPreview {
    /// Try to parse a .pow file from raw bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 16 {
            return None;
        }

        let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
        let magic_ok = magic == POW_MAGIC;

        let power_id = if magic_ok && data.len() >= 0x14 {
            u32::from_le_bytes(data[0x10..0x14].try_into().unwrap())
        } else {
            0
        };

        let sf_values = if magic_ok {
            extract_sf_values(data)
        } else {
            HashMap::new()
        };
        let formulas = extract_formulas(data);

        if sf_values.is_empty() && formulas.is_empty() && !magic_ok {
            return None;
        }

        Some(PowPreview {
            power_id,
            file_size: data.len(),
            magic_ok,
            sf_values,
            formulas,
        })
    }

    /// Format as a human-readable summary for the preview panel.
    pub fn summary(&self) -> String {
        let mut out = String::new();

        if self.magic_ok {
            writeln!(
                out,
                "=== Power 0x{:08X} ({}) ===",
                self.power_id, self.power_id
            )
            .ok();
        }
        writeln!(out, "Size: {} bytes", self.file_size).ok();

        // Collect all SF refs used in formulas
        let mut used_sfs: Vec<u32> = Vec::new();
        for f in &self.formulas {
            for r in &f.sf_refs {
                if let Some(n) = r.strip_prefix("SF_").and_then(|s| s.parse::<u32>().ok()) {
                    if !used_sfs.contains(&n) {
                        used_sfs.push(n);
                    }
                }
            }
        }
        used_sfs.sort();

        // Show SF values that are referenced in formulas
        if !used_sfs.is_empty() {
            writeln!(out, "\n--- Scaling Factors ({}) ---", used_sfs.len()).ok();
            for &n in &used_sfs {
                let val = self
                    .sf_values
                    .get(&n)
                    .map(|s| format_sf_display(s))
                    .unwrap_or_else(|| "???".to_string());
                writeln!(out, "  SF_{n}  = {val}").ok();
            }
        }

        // Formulas grouped by classification
        if !self.formulas.is_empty() {
            let mut damage: Vec<&Formula> = Vec::new();
            let mut cooldown: Vec<&Formula> = Vec::new();
            let mut other: Vec<&Formula> = Vec::new();

            for f in &self.formulas {
                match f.classification {
                    "damage_scalar" => damage.push(f),
                    "cooldown_scalar" => cooldown.push(f),
                    _ => other.push(f),
                }
            }

            if !damage.is_empty() {
                writeln!(out, "\n--- Damage Formulas ({}) ---", damage.len()).ok();
                for f in &damage {
                    self.write_formula(&mut out, f);
                }
            }

            if !cooldown.is_empty() {
                writeln!(out, "\n--- Cooldown/Duration ({}) ---", cooldown.len()).ok();
                for f in &cooldown {
                    self.write_formula(&mut out, f);
                }
            }

            if !other.is_empty() {
                writeln!(out, "\n--- Other Expressions ({}) ---", other.len()).ok();
                for f in &other {
                    self.write_formula(&mut out, f);
                }
            }
        }

        if used_sfs.is_empty() && self.formulas.is_empty() {
            writeln!(out, "\n(No formulas or SF definitions found)").ok();
        }

        out
    }

    fn write_formula(&self, out: &mut String, f: &Formula) {
        // Inline-resolve SF values in the formula display
        let mut display = f.text.clone();
        for r in &f.sf_refs {
            if let Some(n) = r.strip_prefix("SF_").and_then(|s| s.parse::<u32>().ok()) {
                if let Some(val) = self.sf_values.get(&n) {
                    if !val.is_empty() && val != "0" {
                        let resolved = format!("{r}({val})");
                        display = display.replace(r.as_str(), &resolved);
                    }
                }
            }
        }
        writeln!(out, "  {display}").ok();

        // Interpret damage formulas.
        if let Some(coeff) = f.coefficient {
            let pct = coeff * 100.0;
            match f.table_id {
                Some(34) => {
                    writeln!(out, "    → {pct:.0}% weapon damage (level-scaled)").ok();
                }
                Some(35) => {
                    writeln!(out, "    → {coeff} sec base cooldown (level-scaled)").ok();
                }
                Some(id) => {
                    writeln!(out, "    → {coeff} × Table({id})").ok();
                }
                None => {}
            }
        }
    }
}

// ── SF value extraction from formula data block ──────────────────────────────

/// Walk the contiguous formula data block and extract SF values.
fn extract_sf_values(data: &[u8]) -> HashMap<u32, String> {
    let mut values = HashMap::new();

    let struct_start = 0x10usize;
    let struct_size = read_u32(data, 0x78).unwrap_or(0) as usize;
    if struct_size == 0 || struct_size > data.len() {
        return values;
    }
    let var_start = struct_start + struct_size;

    // First formula descriptor at struct+0x0258
    let desc_pos = struct_start + 0x0258;
    if desc_pos + 8 > data.len() {
        return values;
    }
    let first_text_off = read_u32(data, desc_pos).unwrap_or(0) as usize;

    if first_text_off < var_start || first_text_off >= data.len() {
        return values;
    }

    // Walk contiguous formula entries: [text(4-byte aligned)][bytecode(12 bytes)]
    let mut pos = first_text_off;
    let mut idx = 0u32;
    let mut bad_streak = 0u32;

    while idx < 80 && pos + 16 <= data.len() && bad_streak < 3 {
        let text_start = pos;
        let mut text_end = pos;
        while text_end < data.len() && data[text_end] != 0 && (32..127).contains(&data[text_end]) {
            text_end += 1;
        }

        if text_end >= data.len() || data[text_end] != 0 {
            bad_streak += 1;
            pos += 16;
            idx += 1;
            continue;
        }

        let text = String::from_utf8_lossy(&data[text_start..text_end]).into_owned();
        let after_text = text_end + 1;
        let aligned = (after_text + 3) & !3;

        if aligned + 12 > data.len() {
            break;
        }

        let type_tag = read_u32(data, aligned).unwrap_or(0);
        if type_tag != 0 && type_tag != 5 && type_tag != 6 {
            bad_streak += 1;
            pos += 16;
            idx += 1;
            continue;
        }

        bad_streak = 0;
        values.insert(idx, text);
        pos = aligned + 12;
        idx += 1;
    }

    values
}

// ── SF display formatting ────────────────────────────────────────────────────

fn format_sf_display(formula: &str) -> String {
    if formula.is_empty() {
        return "(empty)".to_string();
    }
    if let Ok(v) = formula.parse::<f64>() {
        if v == 0.0 {
            return "0".to_string();
        } else if v.abs() < 10.0 && v.fract() != 0.0 {
            let pct = v * 100.0;
            return format!("{v}  ({pct:.1}%)");
        } else if v == v.trunc() {
            return format!("{v:.0}");
        } else {
            return format!("{v}");
        }
    }
    formula.to_string()
}

// ── Formula extractor with typed values ───────────────────────────────────────

const FORMULA_INDICATORS: &[&str] = &[
    "SF_",
    "Table(",
    "Affix",
    "Attacks_Per_Second",
    "Owner.",
    "Min(",
    "Max(",
    "Chance_For_",
    "AoE_Size",
    " * ",
    " / ",
];

fn is_formula(s: &str) -> bool {
    FORMULA_INDICATORS.iter().any(|ind| s.contains(ind))
}

fn classify(s: &str) -> &'static str {
    let sl = s.to_lowercase();
    if sl.contains("table(") && sl.contains('*') {
        if sl.contains("table(34") {
            return "damage_scalar";
        }
        if sl.contains("table(35") {
            return "cooldown_scalar";
        }
        return "damage_scalar";
    }
    if sl.contains("attacks_per_second") {
        return "attack_speed";
    }
    if sl.contains("affix") && sl.contains("static value") {
        return "unique_item_affix";
    }
    if sl.contains("affix_value") {
        return "affix_modifier";
    }
    if sl.contains("aoe_size") || sl.contains("min(") || sl.contains("max(") {
        return "aoe_scaling";
    }
    if sl.contains("chance_for_double_damage") {
        return "crit_modifier";
    }
    if s.contains("SF_") {
        return "sf_expression";
    }
    "expression"
}

fn parse_typed_values(data: &[u8], offset: usize) -> Vec<TypedValue> {
    let mut values = Vec::new();
    let mut pos = offset;

    for _ in 0..8 {
        if pos + 8 > data.len() {
            break;
        }
        let type_tag = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        match type_tag {
            6 => {
                let fval = f32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
                values.push(TypedValue {
                    kind: "float",
                    display: format!("{fval:.6}"),
                });
            }
            5 => {
                let idx = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
                let sf_num = if idx >= 6 { idx - 6 } else { idx };
                values.push(TypedValue {
                    kind: "SF ref",
                    display: format!("SF_{sf_num} (idx={idx})"),
                });
            }
            _ => break,
        }
        pos += 8;
    }

    values
}

fn extract_coefficient(s: &str) -> (Option<f64>, Option<u32>) {
    if let Some(rest) = s.split("* Table(").nth(1) {
        let before = s.split("* Table(").next().unwrap_or("").trim();
        let table_id: Option<u32> = rest.split(',').next().and_then(|t| t.parse().ok());
        let coeff: Option<f64> = before
            .trim_end()
            .rsplit(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .next()
            .and_then(|n| n.parse().ok());
        return (coeff, table_id);
    }
    (None, None)
}

fn extract_sf_refs(s: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len().saturating_sub(3) {
        if bytes[i] == b'S' && bytes[i + 1] == b'F' && bytes[i + 2] == b'_' {
            let start = i;
            i += 3;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let name = &s[start..i];
            if !refs.contains(&name.to_string()) {
                refs.push(name.to_string());
            }
            continue;
        }
        i += 1;
    }
    refs
}

fn extract_formulas(data: &[u8]) -> Vec<Formula> {
    let mut formulas = Vec::new();
    let strings = extract_printable_strings(data, 4);

    for (offset, s) in &strings {
        if !is_formula(s) {
            continue;
        }
        if s.starts_with("ue ") || s.starts_with("atic") || s.starts_with("oves") {
            continue;
        }

        let null_pos = data[*offset..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(s.len());
        let full_str = String::from_utf8_lossy(&data[*offset..*offset + null_pos]).into_owned();

        let formula_end = *offset + null_pos + 1;
        let aligned_end = (formula_end + 3) & !3;

        let values = parse_typed_values(data, aligned_end);
        let (coefficient, table_id) = extract_coefficient(&full_str);
        let sf_refs = extract_sf_refs(&full_str);

        formulas.push(Formula {
            text: full_str,
            classification: classify(s),
            offset: *offset,
            values,
            sf_refs,
            coefficient,
            table_id,
        });
    }

    formulas.dedup_by_key(|f| f.offset);
    formulas
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_printable_strings(data: &[u8], min_len: usize) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let mut current = Vec::new();
    let mut start = 0;

    for (i, &b) in data.iter().enumerate() {
        if (32..127).contains(&b) {
            if current.is_empty() {
                start = i;
            }
            current.push(b);
        } else {
            if current.len() >= min_len {
                results.push((start, String::from_utf8_lossy(&current).into_owned()));
            }
            current.clear();
        }
    }
    if current.len() >= min_len {
        results.push((start, String::from_utf8_lossy(&current).into_owned()));
    }

    results
}
