//! D4 `.pow` skill/power preview plugin.

use super::{PreviewOutput, PreviewPlugin};

pub struct PowPreview;

impl PreviewPlugin for PowPreview {
    fn name(&self) -> &str {
        ".pow (D4 power)"
    }

    fn can_preview(&self, filename: &str, _data: &[u8]) -> bool {
        filename.to_ascii_lowercase().ends_with(".pow")
    }

    fn build(&self, _filename: &str, data: &[u8], _ctx: &egui::Context) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        if let Some(pow) = crate::pow_preview::PowPreview::parse(data) {
            out.text = Some(pow.summary());
        } else {
            out.text = Some(".pow file could not be parsed.".into());
        }
        out
    }
}
