//! D4 `.vid` ("Movie") preview plugin.
//!
//! Shows the MOVI header metadata and offers a one-click export that
//! strips the 128-byte header so the raw BK2 stream can be opened in
//! RAD's free Bink Player (or a BK2-capable ffmpeg build).

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin};
use crate::vid_preview::{VidPreview as VidHeader, MOVI_HEADER_BYTES};

pub struct VidPreview;

impl PreviewPlugin for VidPreview {
    fn name(&self) -> &str {
        ".vid (D4 Movie / BK2)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        // Accept by extension OR by magic — some Movie files are
        // referenced under FDIDs without a canonical extension.
        let ext_match = filename.to_ascii_lowercase().ends_with(".vid");
        ext_match || VidHeader::parse(data).is_some()
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let Some(vid) = VidHeader::parse(data) else {
            out.text = Some(
                ".vid file did not start with the expected MOVI magic — cannot decode header."
                    .into(),
            );
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
