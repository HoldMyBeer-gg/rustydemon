//! D4 `.tex` raw block-compressed texture preview plugin.

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin};

pub struct TexPreview;

impl PreviewPlugin for TexPreview {
    fn name(&self) -> &str {
        ".tex (D4 BC texture)"
    }

    fn can_preview(&self, filename: &str, _data: &[u8]) -> bool {
        filename.to_ascii_lowercase().ends_with(".tex")
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let Some((rgba, w, h, fmt)) = crate::tex_preview::decode_tex(data, filename) else {
            out.text = Some(
                ".tex header could not be decoded — unknown dimensions or unsupported BC format."
                    .into(),
            );
            return out;
        };

        let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        out.texture =
            Some(ctx.load_texture("tex_preview", color_image, egui::TextureOptions::default()));
        out.texture_pixels = Some((rgba, w, h));
        out.text = Some(format!(
            "D4 .tex texture\n{w}×{h}  {fmt}\n\nDecoded from raw block-compressed data."
        ));

        let filename = filename.to_owned();
        out.extra_exports.push(ExportAction {
            label: "Export As PNG",
            default_extension: "png",
            filter_name: "PNG image",
            build: Arc::new(move |data, _path| {
                let (rgba, w, h, _fmt) = crate::tex_preview::decode_tex(data, &filename)
                    .ok_or_else(|| "tex decode failed".to_string())?;
                crate::preview::encode_png(&rgba, w, h)
            }),
        });
        out
    }
}
