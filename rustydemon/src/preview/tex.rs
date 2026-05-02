//! D4 `.tex` raw block-compressed texture preview plugin.

use std::sync::Arc;

use rustydemon_lib::root::d4_texture::{BlockFormat, TextureDescriptor};

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
        // Preferred path: look up the texture's SNO ID in
        // Texture-Base-Global.dat for exact dims and format. Works for any
        // tier (payload/paylow/paymed) and fixes NPOT / small / wrong-format
        // cases the brute-force decoder can't handle.
        if let Some(desc) = (fetch.texture_info)(filename) {
            if let Some(out) = build_from_descriptor(filename, data, ctx, &desc) {
                return out;
            }
        }

        // Brute-force fallback for non-D4 archives, encrypted SNOs, or
        // unsupported BC variants (BC2, BC6H).
        if let Some((rgba, w, h, fmt)) = crate::tex_preview::decode_tex(data, filename) {
            return build_output(
                ctx,
                rgba,
                w,
                h,
                fmt.to_string(),
                filename.to_owned(),
                None,
                None,
                None,
            );
        }

        // Last-resort fallback for paylow/paymed: preview the payload twin
        // so the user at least sees the texture content.
        if let Some((kind, twin_path)) = payload_twin(filename) {
            if let Some(twin_bytes) = (fetch.by_name)(&twin_path) {
                if let Some(out) =
                    build_from_descriptor_or_brute(&twin_path, &twin_bytes, ctx, fetch, Some(kind))
                {
                    return out;
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

/// Decode a .tex using a known [`TextureDescriptor`].
///
/// - `payload/` files contain mip 0 at the descriptor's full dimensions.
/// - `paylow/` and `paymed/` files contain a smaller mip stream packed
///   largest-first; the largest stored mip is at half the descriptor dims.
fn build_from_descriptor(
    filename: &str,
    data: &[u8],
    ctx: &egui::Context,
    desc: &TextureDescriptor,
) -> Option<PreviewOutput> {
    let bc = desc.block_compression()?;
    let lower = filename.to_ascii_lowercase();
    let is_low = lower.contains("/paylow/") || lower.contains("/paymed/");

    // paylow/paymed start at mip 1 (half dims); payload starts at mip 0.
    let (w, h, label_suffix) = if is_low {
        (
            (desc.width as u32 / 2).max(1),
            (desc.height as u32 / 2).max(1),
            "  (paylow mip)",
        )
    } else {
        (desc.width as u32, desc.height as u32, "")
    };

    let (rgba, w, h, fmt_label) = crate::tex_preview::decode_tex_known(data, w, h, bc)?;

    let mut text = format!(
        "D4 .tex texture\n{w}×{h}  {fmt_label}{label_suffix}\n\n\
         Decoded from descriptor (Texture-Base-Global SNO {}).",
        desc.sno_id
    );
    if is_low {
        text.push_str(
            "\n\nThis is the largest mip stored in the paylow tier — \
             the higher-resolution mip 0 lives in the matching payload/ file.",
        );
    }
    Some(build_output(
        ctx,
        rgba,
        w,
        h,
        fmt_label.to_string(),
        filename.to_owned(),
        None,
        Some(text),
        Some(KnownDecode { bc, w, h }),
    ))
}

/// Captured dims/format used by the PNG export closure when descriptor-driven
/// decoding is in play — without this, PNG export falls back to brute-force
/// and fails for the same NPOT/paylow files the descriptor path just rescued.
#[derive(Clone, Copy)]
struct KnownDecode {
    bc: BlockFormat,
    w: u32,
    h: u32,
}

/// Helper: decode the payload twin via descriptor first, then brute-force.
/// Used as the last-resort fallback when the original paylow file has no
/// known descriptor.
fn build_from_descriptor_or_brute(
    twin_path: &str,
    twin_bytes: &[u8],
    ctx: &egui::Context,
    fetch: &super::SiblingFetcher<'_>,
    paylow_kind: Option<&'static str>,
) -> Option<PreviewOutput> {
    let twin_desc = (fetch.texture_info)(twin_path);
    let (rgba, w, h, fmt_label) = if let Some(desc) = twin_desc {
        let bc = desc.block_compression()?;
        crate::tex_preview::decode_tex_known(twin_bytes, desc.width as u32, desc.height as u32, bc)?
    } else {
        let (rgba, w, h, fmt) = crate::tex_preview::decode_tex(twin_bytes, twin_path)?;
        (rgba, w, h, fmt)
    };
    let note = paylow_kind.map(|kind| {
        format!(
            "{kind}/ couldn't be decoded directly — preview is from the matching \
             payload/ variant. Export Raw still writes the original {kind} bytes."
        )
    });
    let known = twin_desc.and_then(|d| {
        d.block_compression().map(|bc| KnownDecode {
            bc,
            w: d.width as u32,
            h: d.height as u32,
        })
    });
    Some(build_output(
        ctx,
        rgba,
        w,
        h,
        fmt_label.to_string(),
        twin_path.to_owned(),
        Some(twin_bytes.to_vec()),
        note,
        known,
    ))
}

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
    fmt_label: String,
    decode_filename: String,
    decoded_bytes_for_export: Option<Vec<u8>>,
    note: Option<String>,
    known_decode: Option<KnownDecode>,
) -> PreviewOutput {
    let mut out = PreviewOutput::new();
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    out.texture =
        Some(ctx.load_texture("tex_preview", color_image, egui::TextureOptions::default()));
    out.texture_pixels = Some((rgba, w, h));

    let text = match note {
        Some(n) => n,
        None => format!(
            "D4 .tex texture\n{w}×{h}  {fmt_label}\n\nDecoded from raw block-compressed data."
        ),
    };
    out.text = Some(text);

    out.extra_exports.push(ExportAction {
        label: "Export As PNG",
        default_extension: "png",
        filter_name: "PNG image",
        build: Arc::new(move |data, _path| {
            let bytes: &[u8] = decoded_bytes_for_export.as_deref().unwrap_or(data);
            // Prefer the descriptor-driven path when we have it; only NPOT
            // / paylow textures need it but it's strictly better for any
            // file we have a descriptor for.
            let (rgba, w, h, _fmt) = if let Some(k) = known_decode {
                crate::tex_preview::decode_tex_known(bytes, k.w, k.h, k.bc)
                    .ok_or_else(|| "tex decode failed (known descriptor)".to_string())?
            } else {
                crate::tex_preview::decode_tex(bytes, &decode_filename)
                    .ok_or_else(|| "tex decode failed".to_string())?
            };
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
