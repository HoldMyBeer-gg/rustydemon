//! Text-file detection and extraction for the preview panel.
//!
//! A file is considered text-previewable if either:
//!
//! 1. Its extension is on the whitelist of "definitely text" formats, OR
//! 2. The first few KiB are valid UTF-8 with a high proportion of
//!    printable characters (heuristic fallback for unknown extensions).
//!
//! BOMs and trailing NULs are stripped so the rendered text looks clean
//! in the egui multiline view.

/// Maximum number of bytes we'll ever render in the text preview — matches
/// the existing hex preview cap of 256 bytes × many rows so the panel
/// doesn't try to show multi-MB files all at once.
const MAX_PREVIEW_BYTES: usize = 64 * 1024; // 64 KiB
/// Bytes sampled from the head of a file for the printable-ratio heuristic.
const HEURISTIC_SAMPLE_BYTES: usize = 4096;
/// Minimum printable ratio (in [0, 1]) for heuristic text detection.
const PRINTABLE_THRESHOLD: f32 = 0.90;

/// Extensions that are always rendered as text (case-insensitive).
///
/// Keep this list conservative — anything listed here will be force-decoded
/// as UTF-8 (lossy), so exotic binary-with-text-extension formats won't
/// render cleanly but also won't panic.
const TEXT_EXTENSIONS: &[&str] = &[
    // Plain text / docs
    "txt",
    "md",
    "rst",
    "log",
    "readme",
    // Configuration
    "ini",
    "cfg",
    "conf",
    "config",
    "toml",
    "yaml",
    "yml",
    "properties",
    // Markup / structured data
    "xml",
    "json",
    "json5",
    "csv",
    "tsv",
    "html",
    "htm",
    "svg",
    // Scripts
    "lua",
    "py",
    "sh",
    "bash",
    "zsh",
    "fish",
    "bat",
    "cmd",
    "ps1",
    "js",
    "mjs",
    "ts",
    "tsx",
    "jsx",
    // Localisation
    "po",
    "pot",
    "strings",
    "stringtable",
    "stl",
    // Source code people might drop in
    "rs",
    "c",
    "cc",
    "cpp",
    "cxx",
    "h",
    "hpp",
    "hxx",
    "java",
    "cs",
    "go",
    "kt",
    "swift",
    "m",
    "mm",
    "rb",
    "pl",
    "php",
    "scala",
    "dart",
    // Shader sources
    "glsl",
    "hlsl",
    "fx",
    "vert",
    "frag",
    "shader",
    // D4 / Blizzard text-ish formats
    "build",
    "info",
    "buildinfo",
    "manifest",
    "product",
];

/// Classify a file and, if it's text, return a rendered preview string.
///
/// Returns `None` when the file doesn't look like text (caller should fall
/// back to the hex dump).
pub fn decode(filename: Option<&str>, data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }

    let ext_match = filename
        .and_then(|n| n.rsplit('.').next())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            TEXT_EXTENSIONS.iter().any(|e| *e == lower)
        })
        .unwrap_or(false);

    if !ext_match && !looks_like_text(data) {
        return None;
    }

    let truncated = &data[..data.len().min(MAX_PREVIEW_BYTES)];
    let mut text = String::from_utf8_lossy(truncated).into_owned();

    // Strip UTF-8 / UTF-16LE BOM if present.
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }

    // Normalise line endings so Windows-authored files don't render with
    // literal \r characters.
    text = text.replace("\r\n", "\n");

    // Trim trailing NULs (common in fixed-size buffer dumps).
    while text.ends_with('\0') {
        text.pop();
    }

    if data.len() > MAX_PREVIEW_BYTES {
        text.push_str(&format!(
            "\n\n… (truncated — {} of {} bytes shown)",
            MAX_PREVIEW_BYTES,
            data.len()
        ));
    }

    Some(text)
}

/// Heuristic: is this blob mostly printable UTF-8?
fn looks_like_text(data: &[u8]) -> bool {
    let sample = &data[..data.len().min(HEURISTIC_SAMPLE_BYTES)];

    // Must be valid UTF-8 (allow incomplete trailing codepoint at the
    // sample boundary).
    if std::str::from_utf8(sample).is_err() {
        // Try trimming up to 3 bytes off the end in case we chopped a
        // multi-byte codepoint.
        let mut ok = false;
        for trim in 1..=3 {
            if trim > sample.len() {
                break;
            }
            if std::str::from_utf8(&sample[..sample.len() - trim]).is_ok() {
                ok = true;
                break;
            }
        }
        if !ok {
            return false;
        }
    }

    let printable = sample
        .iter()
        .filter(|&&b| {
            b == b'\n' || b == b'\r' || b == b'\t' || (0x20..=0x7E).contains(&b) || b >= 0x80
        })
        .count();

    (printable as f32 / sample.len() as f32) >= PRINTABLE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_plain_text_by_extension() {
        let data = b"hello world\nsecond line\n";
        assert!(decode(Some("foo.txt"), data).is_some());
    }

    #[test]
    fn detects_json_by_content() {
        let data = br#"{"key": "value", "num": 42}"#;
        assert!(decode(Some("unknown.bin"), data).is_some());
    }

    #[test]
    fn rejects_binary_data() {
        let data: Vec<u8> = (0u8..=255).cycle().take(2048).collect();
        assert!(decode(Some("unknown.bin"), &data).is_none());
    }

    #[test]
    fn strips_bom() {
        let data = b"\xEF\xBB\xBFhello";
        let preview = decode(Some("a.txt"), data).unwrap();
        assert_eq!(preview, "hello");
    }

    #[test]
    fn normalises_crlf() {
        let data = b"a\r\nb\r\nc";
        let preview = decode(Some("a.txt"), data).unwrap();
        assert_eq!(preview, "a\nb\nc");
    }

    #[test]
    fn truncates_oversized_text() {
        let data = vec![b'x'; MAX_PREVIEW_BYTES + 1024];
        let preview = decode(Some("a.txt"), &data).unwrap();
        assert!(preview.contains("truncated"));
        assert!(preview.len() < MAX_PREVIEW_BYTES + 200);
    }
}
