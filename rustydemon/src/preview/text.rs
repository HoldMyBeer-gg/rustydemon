//! Generic text-file preview plugin.  Runs last in the registry so
//! structured formats (.pow, .vid, .blp, .tex) claim their files first.

use super::{PreviewOutput, PreviewPlugin};

pub struct TextPreview;

impl PreviewPlugin for TextPreview {
    fn name(&self) -> &str {
        "text"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        crate::text_preview::decode(Some(filename), data).is_some()
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        out.text = crate::text_preview::decode(Some(filename), data);
        out
    }
}
