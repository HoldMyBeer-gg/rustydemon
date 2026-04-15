//! D2R `.model` recognizer plugin.
//!
//! D2R model files don't use an ASCII magic — they start with a fixed
//! 128-bit type GUID that acts as the format identifier.  Every D2R
//! `.model` observed so far begins with the same 32-byte prefix:
//!
//! ```text
//!   0x00  E5 9B 49 5E 6F 63 1F 14 1E 13 EB A9 90 BE ED C4   type GUID
//!   0x10  C8 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00   header_size=456 + padding
//!   0x20  07 00 00 00                                       version=7
//! ```
//!
//! This plugin doesn't decode geometry — that would need more samples
//! and a ground-truth mesh to validate against.  It just recognizes
//! the format and shows the fields we're confident about, so the
//! preview panel says something more useful than "hex dump" when the
//! user clicks a `.model` file.

use super::{PreviewOutput, PreviewPlugin};

pub struct ModelD2rPreview;

/// The fixed 16-byte type GUID every D2R `.model` starts with.  Not a
/// human-readable string — Blizzard uses a binary GUID here the same
/// way it uses `<DE(` for `.texture` files.
const MAGIC: [u8; 16] = [
    0xE5, 0x9B, 0x49, 0x5E, 0x6F, 0x63, 0x1F, 0x14, 0x1E, 0x13, 0xEB, 0xA9, 0x90, 0xBE, 0xED, 0xC4,
];

impl PreviewPlugin for ModelD2rPreview {
    fn name(&self) -> &str {
        ".model (D2R)"
    }

    fn can_preview(&self, _filename: &str, data: &[u8]) -> bool {
        data.len() >= MAGIC.len() && data[..MAGIC.len()] == MAGIC
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        // The header walk-through we're confident about.  Anything
        // past 0x34 is still conjecture and deliberately left out.
        let read_u32 = |off: usize| -> Option<u32> {
            data.get(off..off + 4)
                .and_then(|s| s.try_into().ok())
                .map(u32::from_le_bytes)
        };

        let header_size = read_u32(0x10);
        let version = read_u32(0x20);
        // These two u32s at 0x30/0x34 are small numbers that look like
        // sub-object counts on every sample seen so far, but we're
        // not sure what they count yet — label them honestly.
        let count_a = read_u32(0x30);
        let count_b = read_u32(0x34);

        let mut text = String::new();
        text.push_str("D2R .model  (binary-GUID format, not fully decoded)\n");
        text.push('\n');
        text.push_str("Header fields (verified):\n");
        if let Some(v) = version {
            text.push_str(&format!("  version      = {v}\n"));
        }
        if let Some(h) = header_size {
            text.push_str(&format!(
                "  header_size  = {h} bytes (0x{h:04X}) — payload starts at this offset\n"
            ));
        }
        if let (Some(a), Some(b)) = (count_a, count_b) {
            text.push_str(&format!(
                "  count_a      = {a}   (at 0x30 — possibly sub-objects)\n"
            ));
            text.push_str(&format!(
                "  count_b      = {b}   (at 0x34 — possibly materials)\n"
            ));
        }
        text.push_str(&format!("\nFile size: {} bytes\n", data.len()));
        text.push('\n');
        text.push_str("Geometry decode isn't implemented — collect more samples\n");
        text.push_str("(ideally with a known-shape reference mesh) before attempting it.\n");

        out.text = Some(text);
        out
    }
}
