//! Parser for D4 `.vid` ("Movie") files.
//!
//! The format is a 128-byte custom header followed by a raw Bink Video 2
//! (BK2) stream.  We can't decode BK2 directly — it's RAD Game Tools'
//! proprietary codec and there is no FOSS decoder — but we can:
//!
//! 1. Parse the header to show metadata (dimensions, version, audio GUIDs).
//! 2. Offer a one-click export that strips the 128-byte header and writes
//!    the BK2 stream to disk, where it can be played by RAD's free Bink
//!    Player or converted with ffmpeg (which has BK2 decoding support on
//!    some builds).
//!
//! Reference: `OWLib/DataTool/ToolLogic/Extract/ExtractMovies.cs` in the
//! vendored TACTLib copy.

/// Magic: `'MOVI'` as a little-endian `u32`.
pub const MOVI_MAGIC: u32 = 0x4956_4F4D;
/// Size of the fixed header preceding the BK2 payload.
pub const MOVI_HEADER_BYTES: usize = 128;

/// Magic for a "child reference" stub: `0xDEADBEEF` little-endian.  D4 stores
/// many `.vid` SNOs as 52-byte redirection records that point at the real BK2
/// payload in a parent / sibling container — the stub itself is not playable.
pub const CHILD_STUB_MAGIC: u32 = 0xDEAD_BEEF;
/// Observed size of a D4 child-reference stub.
pub const CHILD_STUB_BYTES: usize = 52;

/// Detect the 52-byte child-reference stub.
pub fn looks_like_child_stub(data: &[u8]) -> bool {
    data.len() == CHILD_STUB_BYTES
        && data.len() >= 4
        && u32::from_le_bytes([data[0], data[1], data[2], data[3]]) == CHILD_STUB_MAGIC
}

/// Parsed `.vid` metadata.
#[derive(Debug, Clone)]
pub struct VidPreview {
    pub version: u32,
    pub flags: u16,
    pub unknown1: u16,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub master_audio_guid: u64,
    pub extra_audio_guid: u64,
    /// Total file size, for computing the BK2 payload size.
    pub file_size: usize,
}

impl VidPreview {
    /// Parse the header if the data looks like a D4 Movie file.
    ///
    /// Returns `None` if the magic doesn't match or the file is shorter
    /// than the 128-byte header.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < MOVI_HEADER_BYTES {
            return None;
        }
        let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
        if magic != MOVI_MAGIC {
            return None;
        }

        Some(Self {
            version: u32::from_le_bytes(data[4..8].try_into().ok()?),
            unknown1: u16::from_le_bytes(data[8..10].try_into().ok()?),
            flags: u16::from_le_bytes(data[10..12].try_into().ok()?),
            width: u32::from_le_bytes(data[12..16].try_into().ok()?),
            height: u32::from_le_bytes(data[16..20].try_into().ok()?),
            depth: u32::from_le_bytes(data[20..24].try_into().ok()?),
            master_audio_guid: u64::from_le_bytes(data[24..32].try_into().ok()?),
            extra_audio_guid: u64::from_le_bytes(data[32..40].try_into().ok()?),
            file_size: data.len(),
        })
    }

    /// Size of the BK2 payload (everything after the header).
    pub fn bk2_size(&self) -> usize {
        self.file_size.saturating_sub(MOVI_HEADER_BYTES)
    }

    /// Build a multi-line human-readable summary for the preview panel.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str("D4 Movie (BK2 container)\n");
        out.push_str(&format!("Version:   {}\n", self.version));
        out.push_str(&format!("Flags:     0x{:04X}\n", self.flags));
        out.push_str(&format!(
            "Dimensions: {} × {} × {}\n",
            self.width, self.height, self.depth
        ));
        out.push_str(&format!("BK2 payload: {} bytes\n", self.bk2_size()));
        out.push('\n');
        out.push_str(&format!(
            "Master audio GUID: {:016X}\n",
            self.master_audio_guid
        ));
        out.push_str(&format!(
            "Extra audio GUID:  {:016X}\n",
            self.extra_audio_guid
        ));
        out.push('\n');
        out.push_str("The payload is a Bink Video 2 (BK2) stream.  Export it\n");
        out.push_str("with 'Export As BK2' and open it in RAD's free Bink Player,\n");
        out.push_str("or convert to MP4 with a BK2-capable ffmpeg build.\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_header() {
        let mut buf = vec![0u8; MOVI_HEADER_BYTES + 10];
        buf[0..4].copy_from_slice(&MOVI_MAGIC.to_le_bytes());
        buf[4..8].copy_from_slice(&3u32.to_le_bytes());
        buf[10..12].copy_from_slice(&0x42u16.to_le_bytes());
        buf[12..16].copy_from_slice(&1920u32.to_le_bytes());
        buf[16..20].copy_from_slice(&1080u32.to_le_bytes());
        buf[20..24].copy_from_slice(&24u32.to_le_bytes());

        let vid = VidPreview::parse(&buf).expect("should parse");
        assert_eq!(vid.version, 3);
        assert_eq!(vid.flags, 0x42);
        assert_eq!(vid.width, 1920);
        assert_eq!(vid.height, 1080);
        assert_eq!(vid.depth, 24);
        assert_eq!(vid.bk2_size(), 10);
    }

    #[test]
    fn rejects_bad_magic() {
        let buf = vec![0u8; MOVI_HEADER_BYTES];
        assert!(VidPreview::parse(&buf).is_none());
    }

    #[test]
    fn rejects_short_data() {
        let buf = vec![0x4Du8; 10];
        assert!(VidPreview::parse(&buf).is_none());
    }
}
