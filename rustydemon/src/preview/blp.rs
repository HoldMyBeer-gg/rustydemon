//! BLP texture preview plugin (WoW / Warcraft).

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin};

pub struct BlpPreview;

fn is_blp(data: &[u8]) -> bool {
    data.len() >= 4 && (&data[..4] == b"BLP2" || &data[..4] == b"BLP1" || &data[..4] == b"BLP0")
}

impl PreviewPlugin for BlpPreview {
    fn name(&self) -> &str {
        ".blp texture"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        // Magic check is authoritative; the extension is a cheap early-out.
        if !filename.to_ascii_lowercase().ends_with(".blp") {
            return false;
        }
        is_blp(data)
    }

    fn build(&self, _filename: &str, data: &[u8], ctx: &egui::Context) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let Ok(blp) = rustydemon_blp2::BlpFile::from_bytes(data.to_vec()) else {
            out.text = Some("BLP header parsed, but decoding failed.".into());
            return out;
        };
        let Ok((pixels, w, h)) = blp.get_pixels(0) else {
            out.text = Some("BLP header parsed, but mipmap 0 could not be decoded.".into());
            return out;
        };

        let color_image =
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
        out.texture =
            Some(ctx.load_texture("blp_preview", color_image, egui::TextureOptions::default()));
        out.texture_pixels = Some((pixels, w, h));
        out.text = Some(format!("BLP texture {w}×{h}"));
        out.extra_exports.push(ExportAction {
            label: "Export As PNG",
            default_extension: "png",
            filter_name: "PNG image",
            build: Arc::new(|data| {
                let blp = rustydemon_blp2::BlpFile::from_bytes(data.to_vec())
                    .map_err(|e| format!("{e:?}"))?;
                let (pixels, w, h) = blp.get_pixels(0).map_err(|e| format!("{e:?}"))?;
                crate::preview::encode_png(&pixels, w, h)
            }),
        });
        out
    }
}
