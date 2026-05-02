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
        fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        // Try the file as-is first. paylow/paymed are mip-stream variants we
        // can't decode yet, so on failure we fall back to previewing the
        // matching payload/ twin. Export Raw still writes the original bytes.
        if let Some((rgba, w, h, fmt)) = crate::tex_preview::decode_tex(data, filename) {
            return build_output(ctx, rgba, w, h, fmt, filename.to_owned(), None, None);
        }

        if let Some((kind, twin_path)) = payload_twin(filename) {
            if let Some(twin_bytes) = (fetch.by_name)(&twin_path) {
                if let Some((rgba, w, h, fmt)) =
                    crate::tex_preview::decode_tex(&twin_bytes, &twin_path)
                {
                    let note = format!(
                        "{kind}/ mipmap stream isn't decoded yet — preview is from \
                         the matching payload/ variant. Export Raw still writes \
                         the original {kind} bytes."
                    );
                    return build_output(
                        ctx,
                        rgba,
                        w,
                        h,
                        fmt,
                        twin_path,
                        Some(twin_bytes),
                        Some(note),
                    );
                }
            }
        }

        let mut out = PreviewOutput::new();
        out.text = Some(
            ".tex header could not be decoded — unknown dimensions or unsupported BC format."
                .into(),
        );
        out
    }
}

/// If `filename` lives under `base/paylow/` or `base/paymed/`, return
/// `(tier_label, twin_path_under_base/payload/)` so the plugin can fetch
/// the full-resolution variant for preview.
fn payload_twin(filename: &str) -> Option<(&'static str, String)> {
    let lower = filename.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("base/paylow/") {
        Some(("paylow", format!("base/payload/{rest}")))
    } else if let Some(rest) = lower.strip_prefix("base/paymed/") {
        Some(("paymed", format!("base/payload/{rest}")))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn build_output(
    ctx: &egui::Context,
    rgba: Vec<u8>,
    w: u32,
    h: u32,
    fmt: &'static str,
    decode_filename: String,
    decoded_bytes_for_export: Option<Vec<u8>>,
    note: Option<String>,
) -> PreviewOutput {
    let mut out = PreviewOutput::new();
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    out.texture =
        Some(ctx.load_texture("tex_preview", color_image, egui::TextureOptions::default()));
    out.texture_pixels = Some((rgba, w, h));

    let mut text =
        format!("D4 .tex texture\n{w}×{h}  {fmt}\n\nDecoded from raw block-compressed data.");
    if let Some(n) = note {
        text.push_str("\n\n");
        text.push_str(&n);
    }
    out.text = Some(text);

    out.extra_exports.push(ExportAction {
        label: "Export As PNG",
        default_extension: "png",
        filter_name: "PNG image",
        build: Arc::new(move |data, _path| {
            // PNG export mirrors what the user *sees*. When we previewed a
            // payload twin, encode that; otherwise fall back to the file
            // bytes the framework hands us.
            let bytes: &[u8] = decoded_bytes_for_export.as_deref().unwrap_or(data);
            let (rgba, w, h, _fmt) = crate::tex_preview::decode_tex(bytes, &decode_filename)
                .ok_or_else(|| "tex decode failed".to_string())?;
            crate::preview::encode_png(&rgba, w, h)
        }),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::payload_twin;

    #[test]
    fn paylow_maps_to_payload_twin() {
        let (tier, twin) =
            payload_twin("base/paylow/Texture/warlock_sigilOfSummons_Color.tex").unwrap();
        assert_eq!(tier, "paylow");
        assert_eq!(
            twin,
            "base/payload/texture/warlock_sigilofsummons_color.tex"
        );
    }

    #[test]
    fn paymed_maps_to_payload_twin() {
        let (tier, twin) = payload_twin("base/paymed/Texture/some_file.tex").unwrap();
        assert_eq!(tier, "paymed");
        assert_eq!(twin, "base/payload/texture/some_file.tex");
    }

    #[test]
    fn payload_returns_none() {
        assert!(payload_twin("base/payload/Texture/foo.tex").is_none());
    }

    #[test]
    fn unrelated_path_returns_none() {
        assert!(payload_twin("World/Maps/Azeroth/foo.adt").is_none());
    }
}
