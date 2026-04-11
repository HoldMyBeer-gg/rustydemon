//! Lightweight .pow file parser for preview display.
//!
//! Ported from d4builder/tools/pow_to_json.py — extracts header info,
//! SF_ definitions, formula strings, and inline typed values from
//! Diablo 4 power files.

use std::fmt::Write;

const POW_MAGIC: u32 = 0xDEADBEEF;

/// Parsed .pow preview data for display.
pub struct PowPreview {
    pub power_id: u32,
    pub file_size: usize,
    pub magic_ok: bool,
    pub sf_definitions: Vec<SfDef>,
    pub formulas: Vec<Formula>,
}

pub struct SfDef {
    pub name: String,
    pub sf_number: u32,
    pub index: u32,
    pub verified: bool,
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

        let sf_definitions = extract_sf_defs(data);
        let formulas = extract_formulas(data);

        if sf_definitions.is_empty() && formulas.is_empty() && !magic_ok {
            return None;
        }

        Some(PowPreview {
            power_id,
            file_size: data.len(),
            magic_ok,
            sf_definitions,
            formulas,
        })
    }

    /// Format as a human-readable summary for the preview panel.
    pub fn summary(&self) -> String {
        let mut out = String::new();

        if self.magic_ok {
            writeln!(out, "=== Power 0x{:08X} ({}) ===", self.power_id, self.power_id).ok();
        }
        writeln!(out, "Size: {} bytes", self.file_size).ok();

        // SF definitions
        if !self.sf_definitions.is_empty() {
            writeln!(out, "\n--- Scaling Factors ({}) ---", self.sf_definitions.len()).ok();
            for sf in &self.sf_definitions {
                let tag = if sf.verified { "OK" } else { "??" };
                writeln!(out, "  [{tag}] {:<8}  internal_idx={}", sf.name, sf.index).ok();
            }
        }

        // Formulas grouped by classification
        if !self.formulas.is_empty() {
            // Group by classification
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

        if self.sf_definitions.is_empty() && self.formulas.is_empty() {
            writeln!(out, "\n(No formulas or SF definitions found)").ok();
        }

        out
    }

    fn write_formula(&self, out: &mut String, f: &Formula) {
        writeln!(out, "  {}", f.text).ok();

        // Show coefficient and table info
        if let Some(coeff) = f.coefficient {
            let table_name = match f.table_id {
                Some(34) => "Damage",
                Some(35) => "Cooldown",
                Some(id) => {
                    writeln!(out, "    Table: {id}").ok();
                    "Unknown"
                }
                None => "",
            };
            if !table_name.is_empty() {
                writeln!(out, "    Coefficient: {coeff} x {table_name} Table").ok();
            }
        }

        // Show SF references
        if !f.sf_refs.is_empty() {
            let refs: String = f.sf_refs.join(", ");
            writeln!(out, "    Uses: {refs}").ok();
        }

        // Show inline typed values
        for v in &f.values {
            writeln!(out, "    {}: {}", v.kind, v.display).ok();
        }
    }
}

// ── SF definition extractor ───────────────────────────────────────────────────

fn extract_sf_defs(data: &[u8]) -> Vec<SfDef> {
    let mut defs = Vec::new();
    let mut i = 0;

    while i < data.len().saturating_sub(8) {
        if data[i] == b'S' && data.get(i + 1) == Some(&b'F') && data.get(i + 2) == Some(&b'_') {
            if i > 0 && data[i - 1] != 0 {
                i += 1;
                continue;
            }

            let s = read_cstring(data, i);

            if let Some(num_str) = s.strip_prefix("SF_") {
                if let Ok(sf_num) = num_str.parse::<u32>() {
                    let meta_start = i + 8;
                    if meta_start + 8 <= data.len() {
                        let type_tag =
                            u32::from_le_bytes(data[meta_start..meta_start + 4].try_into().unwrap());
                        let index = u32::from_le_bytes(
                            data[meta_start + 4..meta_start + 8].try_into().unwrap(),
                        );

                        let verified = type_tag == 5 && index == sf_num + 6;

                        if !defs.iter().any(|d: &SfDef| d.name == s) {
                            defs.push(SfDef {
                                name: s.clone(),
                                sf_number: sf_num,
                                index,
                                verified,
                            });
                        }
                    }

                    i += s.len() + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    defs
}

// ── Formula extractor with typed values ───────────────────────────────────────

const FORMULA_INDICATORS: &[&str] = &[
    "SF_", "Table(", "Affix", "Attacks_Per_Second", "Owner.", "Min(", "Max(",
    "Chance_For_", "AoE_Size", " * ", " / ",
];

fn is_formula(s: &str) -> bool {
    FORMULA_INDICATORS.iter().any(|ind| s.contains(ind))
}

fn classify(s: &str) -> &'static str {
    let sl = s.to_lowercase();
    if sl.contains("table(") && sl.contains('*') {
        if sl.contains("table(34") { return "damage_scalar"; }
        if sl.contains("table(35") { return "cooldown_scalar"; }
        return "damage_scalar";
    }
    if sl.contains("attacks_per_second") { return "attack_speed"; }
    if sl.contains("affix") && sl.contains("static value") { return "unique_item_affix"; }
    if sl.contains("affix_value") { return "affix_modifier"; }
    if sl.contains("aoe_size") || sl.contains("min(") || sl.contains("max(") { return "aoe_scaling"; }
    if sl.contains("chance_for_double_damage") { return "crit_modifier"; }
    if s.contains("SF_") { return "sf_expression"; }
    "expression"
}

/// Parse typed (type_tag, value) pairs after a formula string.
/// type 6 = float literal, type 5 = SF reference (index = SF_N + 6).
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
            _ => break, // not a recognized type pair
        }
        pos += 8;
    }

    values
}

/// Extract coefficient and table ID from a damage formula string.
fn extract_coefficient(s: &str) -> (Option<f64>, Option<u32>) {
    // Try "1.75 * Table(34,sLevel)"
    if let Some(rest) = s.split("* Table(").nth(1) {
        let before = s.split("* Table(").next().unwrap_or("").trim();
        let table_id: Option<u32> = rest.split(',').next().and_then(|t| t.parse().ok());

        // Try parsing the coefficient before "* Table("
        let coeff: Option<f64> = before
            .trim_end()
            .rsplit(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .next()
            .and_then(|n| n.parse().ok());

        return (coeff, table_id);
    }
    (None, None)
}

/// Extract SF_N references from formula text.
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

        // Find actual null terminator for full string
        let null_pos = data[*offset..].iter().position(|&b| b == 0).unwrap_or(s.len());
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

fn read_cstring(data: &[u8], offset: usize) -> String {
    let mut end = offset;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    String::from_utf8_lossy(&data[offset..end]).into_owned()
}

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
