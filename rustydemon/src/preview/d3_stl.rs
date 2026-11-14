//! Diablo III StringList SNOs (`<locale>/StringList/*.stl`): the game's
//! text, keyed by label.
//!
//! ```text
//! 0x00  u32  0xDEADBEEF     SNO header
//! 0x04  u32  0x2D           shared with Condition SNOs, so not a type tag
//! 0x10  u32  SNO ID
//! 0x28  u32  entry array offset, from byte 16
//! 0x2C  u32  entry array size in bytes
//! entry, 40 bytes:
//!   [8]  zero
//!   u32  label offset (from byte 16), u32 label size (with its NUL)
//!   [8]  zero
//!   u32  text offset, u32 text size
//!   u32  hash (of what is not known)
//!   [4]  zero
//! ```
//!
//! The strings follow the array, each NUL-terminated.

use std::sync::Arc;

const SNO_MAGIC: u32 = 0xDEAD_BEEF;
const STRINGLIST_WORD: u32 = 0x2D;
const DATA_BASE: usize = 16;
const ENTRY: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub text: String,
    pub hash: u32,
}

fn u32_at(data: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(o..o + 4)?.try_into().ok()?))
}

/// Whether the preview takes this file. The word after the magic is shared
/// with Condition SNOs, so the name must end in `.stl` as well.
pub fn claims(filename: &str, data: &[u8]) -> bool {
    filename.to_ascii_lowercase().ends_with(".stl")
        && u32_at(data, 0) == Some(SNO_MAGIC)
        && u32_at(data, 4) == Some(STRINGLIST_WORD)
}

pub fn parse(data: &[u8]) -> Result<Vec<Entry>, String> {
    let field = |o| u32_at(data, o).ok_or_else(|| "header is truncated".to_owned());
    let start = DATA_BASE + field(0x28)? as usize;
    let size = field(0x2C)? as usize;
    if !size.is_multiple_of(ENTRY) {
        return Err(format!(
            "entry array of {size} bytes is not whole {ENTRY}-byte entries"
        ));
    }
    let array = data
        .get(start..start + size)
        .ok_or("entry array runs past the end")?;
    let string = |o: usize, n: usize| -> Result<String, String> {
        let off = u32_at(array, o).expect("inside the entry") as usize;
        let len = u32_at(array, o + 4).expect("inside the entry") as usize;
        let at = DATA_BASE + off;
        let bytes = data
            .get(at..at + len)
            .ok_or_else(|| format!("string {n} runs past the end"))?;
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        Ok(String::from_utf8_lossy(bytes).into_owned())
    };
    (0..size / ENTRY)
        .map(|n| {
            let e = n * ENTRY;
            Ok(Entry {
                label: string(e + 8, n)?,
                text: string(e + 24, n)?,
                hash: u32_at(array, e + 32).expect("inside the entry"),
            })
        })
        .collect()
}

/// One line per entry, `label = text`, with line breaks inside a text
/// shown as `\n`.
pub fn render(list: &[Entry]) -> String {
    let mut out = format!("{} strings\n\n", list.len());
    for e in list {
        out.push_str(&e.label);
        out.push_str(" = ");
        out.push_str(&e.text.replace('\r', "").replace('\n', "\\n"));
        out.push('\n');
    }
    out
}

pub struct D3StlPreview;

impl super::PreviewPlugin for D3StlPreview {
    fn name(&self) -> &str {
        "Diablo III string list"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        claims(filename, data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> super::PreviewOutput {
        let mut out = super::PreviewOutput::new();
        match parse(data) {
            Ok(list) => {
                out.text = Some(render(&list));
                out.extra_exports.push(super::ExportAction {
                    label: "Export As TXT",
                    default_extension: "txt",
                    filter_name: "Text",
                    build: Arc::new(|data, _path| Ok(render(&parse(data)?).into_bytes())),
                });
            }
            Err(e) => {
                out.text = Some(format!("Diablo III string list\n\nCould not read it: {e}"));
            }
        }
        out
    }
}
