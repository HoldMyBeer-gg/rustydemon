//! Lightweight .pow file parser for preview display.
//!
//! Extracts header info, SF_ scaling factors, and formula strings from
//! Diablo 4 power files. SF_ values are positionally indexed in the
//! ptScriptFormulas array: SF_0 = entry[0], SF_5 = entry[5], etc.
//!
//! Binary layout (discovered via binary analysis + d4data definitions):
//! - 0x00: magic (0xDEADBEEF)
//! - 0x10: power_id (u32); the fixed struct starts here
//! - Current client: an (offset, size) pair at 0xBB8 locates the script
//!   formula array, offsets relative to 0x10. Each 0x20-byte entry holds
//!   the formula text's (offset, size) at +0x08 and its bytecode's at +0x18.
//! - Original layout: 0x78 holds the fixed struct size (typically 3232),
//!   variable data follows at 0x10 + struct_size, and struct+0x0258 holds
//!   the absolute offset of the first formula text. Entries are contiguous:
//!   [text (4-byte aligned)][12-byte bytecode (type_tag + value)]

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

        // The scaling factors the formulas use, after the formulas: the plain
        // lines above already carry their values.
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

        if used_sfs.is_empty() && self.formulas.is_empty() {
            writeln!(out, "\n(No formulas or SF definitions found)").ok();
        }

        out
    }

    fn write_formula(&self, out: &mut String, f: &Formula) {
        // The plain reading first, the formula as stored under it.
        match plain_formula(&f.text, &self.sf_values) {
            Some(plain) => {
                writeln!(out, "  = {plain}").ok();
                writeln!(out, "    {}", f.text).ok();
            }
            None => {
                writeln!(out, "  {}", f.text).ok();
            }
        }
    }
}

/// Up to four decimals, trailing zeros dropped (0.42 * 120 * 100 → "5040").
fn format_number(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

// ── Plain reading of a formula ───────────────────────────────────────────────

/// A formula read as plain English, with scaling factors replaced by their
/// values, `Table(34, …)` as weapon damage, `Table(35, …)` as base cooldown,
/// `sLevel` as level and constant products folded (0.35 * 100 → 35).
/// `None` when the text can't be parsed, or the reading would only repeat it.
fn plain_formula(text: &str, sf_values: &HashMap<u32, String>) -> Option<String> {
    let tokens = tokenize(text)?;
    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
        sf: sf_values,
        inlining: Vec::new(),
    };
    let expr = p.sum()?;
    if p.pos != tokens.len() {
        return None;
    }
    let shown = render(&expr, 0);
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    (squash(&shown) != squash(text)).then_some(shown)
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Sym(char),
}

fn tokenize(text: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            out.push(Tok::Num(s.parse().ok()?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
        } else if "+-*/(),".contains(c) {
            out.push(Tok::Sym(c));
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

#[derive(Clone, Debug)]
enum Expr {
    Num(f64),
    /// A name shown as is: `level`, an unresolved `SF_n`, anything else.
    Name(String),
    /// `Table(34, level)`: weapon damage; level `None` means `sLevel`.
    Weapon(Option<String>),
    /// `Table(35, level)`: base cooldown.
    Cooldown(Option<String>),
    Call(String, Vec<Expr>),
    /// Terms with their sign (true = added).
    Sum(Vec<(bool, Expr)>),
    /// Factors with their side (true = multiplied, false = divided).
    Product(Vec<(bool, Expr)>),
}

struct Parser<'a> {
    tokens: &'a [Tok],
    pos: usize,
    sf: &'a HashMap<u32, String>,
    /// Scaling factors being inlined, outermost first, so one that refers
    /// to itself stops instead of looping.
    inlining: Vec<u32>,
}

/// How deep scaling factors that are formulas get inlined.
const MAX_INLINE: usize = 4;

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Sym(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Option<Expr> {
        let mut terms = vec![(true, self.product()?)];
        loop {
            if self.eat('+') {
                terms.push((true, self.product()?));
            } else if self.eat('-') {
                terms.push((false, self.product()?));
            } else {
                break;
            }
        }
        Some(if terms.len() == 1 {
            terms.pop()?.1
        } else {
            Expr::Sum(terms)
        })
    }

    fn product(&mut self) -> Option<Expr> {
        let mut factors = vec![(true, self.unary()?)];
        loop {
            if self.eat('*') {
                factors.push((true, self.unary()?));
            } else if self.eat('/') {
                factors.push((false, self.unary()?));
            } else {
                break;
            }
        }
        Some(if factors.len() == 1 {
            factors.pop()?.1
        } else {
            Expr::Product(factors)
        })
    }

    fn unary(&mut self) -> Option<Expr> {
        if self.eat('-') {
            let inner = self.unary()?;
            return Some(Expr::Product(vec![(true, Expr::Num(-1.0)), (true, inner)]));
        }
        self.atom()
    }

    fn atom(&mut self) -> Option<Expr> {
        match self.peek()?.clone() {
            Tok::Num(n) => {
                self.pos += 1;
                Some(Expr::Num(n))
            }
            Tok::Sym('(') => {
                self.pos += 1;
                let e = self.sum()?;
                self.eat(')').then_some(e)
            }
            Tok::Ident(name) => {
                self.pos += 1;
                if self.eat('(') {
                    let mut args = Vec::new();
                    if !self.eat(')') {
                        loop {
                            args.push(self.sum()?);
                            if self.eat(')') {
                                break;
                            }
                            if !self.eat(',') {
                                return None;
                            }
                        }
                    }
                    return Some(table_or_call(name, args));
                }
                Some(self.name(name))
            }
            Tok::Sym(_) => None,
        }
    }

    fn name(&self, name: String) -> Expr {
        if name.eq_ignore_ascii_case("sLevel") {
            return Expr::Name("level".into());
        }
        let Some((n, text)) = name
            .strip_prefix("SF_")
            .and_then(|n| n.parse::<u32>().ok())
            .and_then(|n| self.sf.get(&n).map(|t| (n, t.trim())))
        else {
            return Expr::Name(name);
        };
        if let Ok(v) = text.parse::<f64>() {
            return Expr::Num(v);
        }
        // A scaling factor that is itself a formula: read that formula in
        // its place, unless it is already being read or nests too deep.
        if text.is_empty() || self.inlining.contains(&n) || self.inlining.len() >= MAX_INLINE {
            return Expr::Name(name);
        }
        let Some(tokens) = tokenize(text) else {
            return Expr::Name(name);
        };
        let mut inlining = self.inlining.clone();
        inlining.push(n);
        let mut inner = Parser {
            tokens: &tokens,
            pos: 0,
            sf: self.sf,
            inlining,
        };
        match inner.sum() {
            Some(e) if inner.pos == tokens.len() => e,
            _ => Expr::Name(name),
        }
    }
}

fn table_or_call(name: String, args: Vec<Expr>) -> Expr {
    if name == "Table" && args.len() == 2 {
        if let Expr::Num(id) = args[0] {
            let level = match &args[1] {
                Expr::Name(n) if n == "level" => None,
                other => Some(render(other, 0)),
            };
            if id == 34.0 {
                return Expr::Weapon(level);
            }
            if id == 35.0 {
                return Expr::Cooldown(level);
            }
        }
    }
    Expr::Call(name, args)
}

/// Render with folding. `ctx` is the binding strength around it:
/// 0 anywhere, 1 inside a product (sums need parentheses).
fn render(e: &Expr, ctx: u8) -> String {
    match e {
        Expr::Num(n) => format_number(*n),
        Expr::Name(n) => n.clone(),
        Expr::Weapon(level) => format!("weapon damage{}", at_level(level)),
        Expr::Cooldown(level) => format!("base cooldown{}", at_level(level)),
        Expr::Call(name, args) => {
            let args: Vec<String> = args.iter().map(|a| render(a, 0)).collect();
            format!("{name}({})", args.join(", "))
        }
        Expr::Sum(terms) => {
            // Constants fold into the place of the first one; the other
            // terms keep the formula's order.
            let constant: f64 = terms
                .iter()
                .filter_map(|(plus, t)| fold(t).map(|v| if *plus { v } else { -v }))
                .sum();
            let mut placed = false;
            let mut parts: Vec<(bool, String)> = Vec::new();
            for (plus, t) in terms {
                if fold(t).is_some() {
                    if !placed {
                        placed = true;
                        if constant != 0.0 || terms.iter().all(|(_, t)| fold(t).is_some()) {
                            parts.push((constant >= 0.0, format_number(constant.abs())));
                        }
                    }
                } else {
                    parts.push((*plus, render(t, 1)));
                }
            }
            let mut s = String::new();
            for (plus, p) in parts {
                if s.is_empty() {
                    if !plus {
                        s.push('-');
                    }
                } else {
                    s.push_str(if plus { " + " } else { " - " });
                }
                s.push_str(&p);
            }
            if ctx >= 1 && s.contains([' ']) {
                format!("({s})")
            } else {
                s
            }
        }
        Expr::Product(factors) => {
            let mut flat = Vec::new();
            flatten(factors, true, &mut flat);
            let (mut num, mut den) = (1.0, 1.0);
            let mut weapon: Option<&Option<String>> = None;
            let mut times: Vec<String> = Vec::new();
            let mut over: Vec<String> = Vec::new();
            for (mul, f) in flat {
                match (fold(f), f) {
                    (Some(v), _) => {
                        if mul {
                            num *= v;
                        } else {
                            den *= v;
                        }
                    }
                    (None, Expr::Weapon(level)) if mul && weapon.is_none() => weapon = Some(level),
                    (None, f) => {
                        let shown = render(f, 1);
                        if mul {
                            times.push(shown);
                        } else {
                            over.push(shown);
                        }
                    }
                }
            }
            let mut parts: Vec<String> = Vec::new();
            match weapon {
                // Level-scaled weapon damage takes every number as one
                // percentage. At a fixed level what the product means is
                // not known (often a tooltip value times 100), so it stays
                // plain arithmetic.
                Some(level) if level.is_none() => {
                    let pct = num / den * 100.0;
                    den = 1.0;
                    if pct == 100.0 && !times.is_empty() {
                        parts.extend(times);
                        parts.push(format!("weapon damage{}", at_level(level)));
                    } else {
                        parts.extend(times);
                        parts.push(format!(
                            "{}% weapon damage{}",
                            format_number(pct),
                            at_level(level)
                        ));
                    }
                }
                other => {
                    if num != 1.0 || times.is_empty() {
                        parts.push(format_number(num));
                    }
                    parts.extend(times);
                    if let Some(level) = other {
                        parts.push(format!("weapon damage{}", at_level(level)));
                    }
                }
            }
            let mut s = parts.join(" × ");
            if den != 1.0 {
                over.insert(0, format_number(den));
            }
            for o in over {
                s.push_str(" / ");
                s.push_str(&o);
            }
            s
        }
    }
}

/// Spread nested products into one list of (multiplied?, factor).
fn flatten<'a>(factors: &'a [(bool, Expr)], outer: bool, out: &mut Vec<(bool, &'a Expr)>) {
    for (mul, f) in factors {
        let side = *mul == outer;
        match f {
            Expr::Product(inner) => flatten(inner, side, out),
            f => out.push((side, f)),
        }
    }
}

fn at_level(level: &Option<String>) -> String {
    level
        .as_ref()
        .map(|l| format!(" at level {l}"))
        .unwrap_or_default()
}

/// The value of an expression made only of numbers, if it is one.
fn fold(e: &Expr) -> Option<f64> {
    match e {
        Expr::Num(n) => Some(*n),
        Expr::Sum(terms) => terms
            .iter()
            .map(|(plus, t)| fold(t).map(|v| if *plus { v } else { -v }))
            .sum(),
        Expr::Product(factors) => factors.iter().try_fold(1.0, |acc, (mul, f)| {
            fold(f).map(|v| if *mul { acc * v } else { acc / v })
        }),
        _ => None,
    }
}

// ── SF value extraction from formula data block ──────────────────────────────

/// Extract SF values: SF_n is the text of script formula n.
///
/// Reads the current client's formula array first. If it isn't there,
/// falls back to the layout this parser was first written for.
fn extract_sf_values(data: &[u8]) -> HashMap<u32, String> {
    sf_values_from_formula_array(data).unwrap_or_else(|| sf_values_from_contiguous_block(data))
}

/// Where the current client keeps the script formula array: an
/// (offset, size) pair in the fixed struct, offsets relative to 0x10.
const FORMULA_ARRAY_PTR: usize = 0x0BB8;
const STRUCT_START: usize = 0x10;
const FORMULA_ENTRY_SIZE: usize = 0x20;

/// Read the current client's script formula array. Each 0x20-byte entry
/// holds the formula text's (offset, size) at +0x08 and its bytecode's
/// (offset, size) at +0x18. Returns `None` when the pair at
/// [`FORMULA_ARRAY_PTR`] doesn't describe a well-formed array of text
/// entries, so an older file falls through to the other reader.
fn sf_values_from_formula_array(data: &[u8]) -> Option<HashMap<u32, String>> {
    let off = read_u32(data, FORMULA_ARRAY_PTR)? as usize;
    let size = read_u32(data, FORMULA_ARRAY_PTR + 4)? as usize;
    if size == 0 || !size.is_multiple_of(FORMULA_ENTRY_SIZE) {
        return None;
    }
    let start = STRUCT_START.checked_add(off)?;
    data.get(start..start.checked_add(size)?)?;

    let mut values = HashMap::new();
    for i in 0..size / FORMULA_ENTRY_SIZE {
        let entry = start + i * FORMULA_ENTRY_SIZE;
        let text_off = read_u32(data, entry + 0x08)? as usize;
        let text_len = read_u32(data, entry + 0x0C)? as usize;
        if text_len == 0 {
            values.insert(i as u32, String::new());
            continue;
        }
        let text_start = STRUCT_START.checked_add(text_off)?;
        let bytes = data.get(text_start..text_start.checked_add(text_len)?)?;
        let (last, text) = bytes.split_last()?;
        if *last != 0 || !text.iter().all(|b| (32..127).contains(b)) {
            return None;
        }
        values.insert(i as u32, String::from_utf8_lossy(text).into_owned());
    }
    Some(values)
}

/// The original layout: struct size at 0x78, an absolute offset to the
/// first formula text at struct+0x258, then contiguous entries.
fn sf_values_from_contiguous_block(data: &[u8]) -> HashMap<u32, String> {
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
        let sf_refs = extract_sf_refs(&full_str);

        formulas.push(Formula {
            text: full_str,
            classification: classify(s),
            offset: *offset,
            values,
            sf_refs,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put_str(buf: &mut [u8], off: usize, s: &str) {
        buf[off..off + s.len()].copy_from_slice(s.as_bytes());
        buf[off + s.len()] = 0;
    }

    /// A power in the current client's layout: the fixed struct holds an
    /// (offset, size) pair at file 0xBB8 for the script formula array.
    /// Offsets are relative to the struct start at 0x10. Each array entry
    /// is 0x20 bytes, with the formula text's (offset, size) at +0x08 and
    /// its compiled bytecode's (offset, size) at +0x18. SF_n is entry n.
    /// The word at 0x78 is 0, as in some real files.
    fn current_layout_pow(sf_texts: &[&str], formula: &str) -> Vec<u8> {
        let mut buf = vec![0u8; 0x1000];
        put_u32(&mut buf, 0x00, POW_MAGIC);
        put_u32(&mut buf, 0x10, 0x0006_13B2);
        let array = 0xC00;
        put_u32(&mut buf, 0xBB8, (array - 0x10) as u32);
        put_u32(&mut buf, 0xBBC, (sf_texts.len() * 0x20) as u32);
        let mut text = 0xD00;
        for (i, s) in sf_texts.iter().enumerate() {
            let entry = array + i * 0x20;
            put_str(&mut buf, text, s);
            put_u32(&mut buf, entry + 0x08, (text - 0x10) as u32);
            put_u32(&mut buf, entry + 0x0C, (s.len() + 1) as u32);
            // Bytecode: a float push of 0, as the game writes after a literal.
            let code = text + ((s.len() + 1 + 3) & !3);
            put_u32(&mut buf, code, 6);
            put_u32(&mut buf, entry + 0x18, (code - 0x10) as u32);
            put_u32(&mut buf, entry + 0x1C, 12);
            text = code + 0x10;
        }
        put_str(&mut buf, 0xF00, formula);
        buf
    }

    #[test]
    fn current_layout_scaling_factors_resolve() {
        let data = current_layout_pow(&["SF_1 * Table(34, sLevel)", "0.42"], "SF_1 * 2.5");
        let pow = PowPreview::parse(&data).expect("parses");
        assert_eq!(
            pow.sf_values.get(&0).map(String::as_str),
            Some("SF_1 * Table(34, sLevel)")
        );
        assert_eq!(pow.sf_values.get(&1).map(String::as_str), Some("0.42"));
        let summary = pow.summary();
        assert!(summary.contains("SF_1  = 0.42"), "{summary}");
        assert!(!summary.contains("???"), "{summary}");
    }

    #[test]
    fn current_layout_empty_formula_array_resolves_nothing() {
        let data = current_layout_pow(&[], "SF_3 * 2");
        let pow = PowPreview::parse(&data).expect("parses");
        assert!(pow.sf_values.is_empty());
    }

    /// The layout this parser was first written for: struct size at 0x78,
    /// and an absolute offset to the first formula text at file 0x268,
    /// followed by [text, 4-byte aligned][12-byte bytecode] entries.
    #[test]
    fn older_layout_still_resolves() {
        let mut data = vec![0u8; 0x300];
        put_u32(&mut data, 0x00, POW_MAGIC);
        put_u32(&mut data, 0x78, 0x100);
        put_u32(&mut data, 0x268, 0x280);
        put_str(&mut data, 0x280, "0.5");
        put_u32(&mut data, 0x284, 6);
        put_str(&mut data, 0x290, "7");
        put_u32(&mut data, 0x294, 6);
        let pow = PowPreview::parse(&data).expect("parses");
        assert_eq!(pow.sf_values.get(&0).map(String::as_str), Some("0.5"));
        assert_eq!(pow.sf_values.get(&1).map(String::as_str), Some("7"));
    }

    /// The summary for one formula, with the given scaling factor values.
    fn summary_of(formula: &str, sfs: &[(u32, &str)]) -> String {
        let mut data = formula.as_bytes().to_vec();
        data.extend_from_slice(&[0; 16]);
        PowPreview {
            power_id: 0,
            file_size: data.len(),
            magic_ok: false,
            sf_values: sfs.iter().map(|(n, v)| (*n, v.to_string())).collect(),
            formulas: extract_formulas(&data),
        }
        .summary()
    }

    /// The plain line for one formula, or `None` when there is none.
    fn plain_of(formula: &str, sfs: &[(u32, &str)]) -> Option<String> {
        let sf: HashMap<u32, String> = sfs.iter().map(|(n, v)| (*n, v.to_string())).collect();
        plain_formula(formula, &sf)
    }

    #[test]
    fn plain_line_multiplies_every_factor() {
        assert_eq!(
            plain_of("SF_2 * Table(34,sLevel) * 1.5", &[(2, "2")]).as_deref(),
            Some("300% weapon damage")
        );
        assert_eq!(
            plain_of("0.9* Table(34,sLevel)", &[]).as_deref(),
            Some("90% weapon damage")
        );
    }

    #[test]
    fn plain_line_names_a_fixed_table_level() {
        assert_eq!(
            plain_of("SF_1 * 120 * Table(34, 3)", &[(1, "0.42")]).as_deref(),
            Some("50.4 × weapon damage at level 3")
        );
    }

    #[test]
    fn plain_line_keeps_sums_and_reads_slevel_as_level() {
        assert_eq!(
            plain_of(
                "(SF_1 + SF_2 * sLevel) * Table(34, sLevel)",
                &[(1, "0.5"), (2, "0.1")]
            )
            .as_deref(),
            Some("(0.5 + 0.1 × level) × weapon damage")
        );
    }

    #[test]
    fn plain_line_reads_the_cooldown_table() {
        assert_eq!(
            plain_of("Table(35, sLevel) * 0.8", &[]).as_deref(),
            Some("0.8 × base cooldown")
        );
    }

    #[test]
    fn plain_line_keeps_unknown_scaling_factors_and_functions() {
        assert_eq!(
            plain_of("min(0.1 * Table(34, sLevel), SF_16)", &[(16, "24")]).as_deref(),
            Some("min(10% weapon damage, 24)")
        );
        assert_eq!(
            plain_of("SF_1 * Table(34, sLevel)", &[]).as_deref(),
            Some("SF_1 × weapon damage")
        );
    }

    #[test]
    fn plain_line_folds_constants_and_skips_what_it_cannot_read() {
        assert_eq!(
            plain_of("SF_3 * 100", &[(3, "0.35")]).as_deref(),
            Some("35")
        );
        assert_eq!(plain_of("2 + 3 / 2", &[]).as_deref(), Some("3.5"));
        // Nothing to say beyond the formula itself.
        assert_eq!(plain_of("0.5", &[]), None);
        // Text it cannot parse gets no plain line rather than a wrong one.
        assert_eq!(plain_of("PowerTag.\"Power\" * 2", &[]), None);
    }

    #[test]
    fn plain_line_inlines_a_scaling_factor_that_is_a_formula() {
        assert_eq!(
            plain_of("SF_1 * 1.25", &[(1, "SF_5 * Table(34,sLevel)"), (5, "0.9")]).as_deref(),
            Some("112.5% weapon damage")
        );
    }

    #[test]
    fn plain_line_keeps_division_and_puts_constants_last() {
        assert_eq!(
            plain_of("SF_49 / SF_48", &[(48, "4")]).as_deref(),
            Some("SF_49 / 4")
        );
        assert_eq!(
            plain_of("SF_9 - SF_3", &[(3, "1.5")]).as_deref(),
            Some("SF_9 - 1.5")
        );
    }

    #[test]
    fn a_scaling_factor_that_names_itself_does_not_loop() {
        let plain = plain_of("SF_1 * 2", &[(1, "SF_1 + 1")]);
        assert!(plain.is_some());
    }

    #[test]
    fn summary_puts_the_plain_line_above_the_formula_and_factors_after() {
        let s = summary_of("SF_2 * Table(34,sLevel) * 1.5", &[(2, "2")]);
        let plain = s.find("= 300% weapon damage").expect(&s);
        let raw = s.find("SF_2 * Table(34,sLevel) * 1.5").expect(&s);
        let factors = s.find("Scaling Factors").expect(&s);
        assert!(plain < raw && raw < factors, "{s}");
        assert!(!s.contains("SF_2(2)"), "{s}");
    }
}
