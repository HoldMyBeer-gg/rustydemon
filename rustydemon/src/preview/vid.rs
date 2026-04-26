//! D4 `.vid` ("Movie") preview plugin.
//!
//! Shows the MOVI header metadata and offers a one-click export that
//! strips the 128-byte header so the raw BK2 stream can be opened in
//! RAD's free Bink Player (or a BK2-capable ffmpeg build).

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin};
use crate::vid_preview::{
    looks_like_child_stub, VidPreview as VidHeader, CHILD_STUB_BYTES, MOVI_HEADER_BYTES,
};

pub struct VidPreview;

impl PreviewPlugin for VidPreview {
    fn name(&self) -> &str {
        ".vid (D4 Movie / BK2)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        // Accept by extension OR by magic — some Movie files are
        // referenced under FDIDs without a canonical extension.
        let ext_match = filename.to_ascii_lowercase().ends_with(".vid");
        ext_match || VidHeader::parse(data).is_some() || looks_like_child_stub(data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        if looks_like_child_stub(data) {
            out.text = Some(format!(
                "D4 Movie redirection stub ({CHILD_STUB_BYTES} bytes, magic 0xDEADBEEF)\n\n\
                 This is not the actual movie. D4 stores `.vid` SNOs as small\n\
                 redirection records in `base/child/Movie/` and `base/meta/Movie/`\n\
                 (the two are byte-identical), and there are NO `base/payload/Movie/`\n\
                 entries — the real BK2 stream lives in a separate layer that the\n\
                 engine resolves at runtime via these stubs.\n\n\
                 Fetching the real movie from a stub isn't implemented (we don't\n\
                 yet know how the stub fields map to the streaming archive). See\n\
                 `research/d4/movie-stubs.md` for the layout we've reverse-engineered\n\
                 so far.\n\n\
                 (Separately, if you see 'index entry not found for ekey static\n\
                 container' errors on other .vid SNOs, those are stubs whose own\n\
                 data file isn't on disk — usually a partial install.)"
            ));
            return out;
        }

        let Some(vid) = VidHeader::parse(data) else {
            out.text = Some(format!(
                ".vid did not start with MOVI ({MOVI_HEADER_BYTES}-byte header) or\n\
                 the DEADBEEF child-reference stub. Cannot identify this format.\n\
                 First 4 bytes: {:02X} {:02X} {:02X} {:02X}",
                data.first().copied().unwrap_or(0),
                data.get(1).copied().unwrap_or(0),
                data.get(2).copied().unwrap_or(0),
                data.get(3).copied().unwrap_or(0),
            ));
            return out;
        };

        out.text = Some(vid.summary());

        out.extra_exports.push(ExportAction {
            label: "Export As BK2",
            default_extension: "bk2",
            filter_name: "Bink Video 2",
            build: Arc::new(|data, _path| {
                if data.len() < MOVI_HEADER_BYTES {
                    return Err("file shorter than MOVI header".into());
                }
                Ok(data[MOVI_HEADER_BYTES..].to_vec())
            }),
        });
        out
    }
}
