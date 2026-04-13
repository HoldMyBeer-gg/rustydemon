//! PCX image preview plugin.
//!
//! Decodes the ZSoft PCX image format used by StarCraft 1 for UI, fonts,
//! and menu art. Supports 8 bits-per-pixel palettized images (the common
//! SC1 case) with either a trailing 256-color palette or a header EGA
//! palette fallback.

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin};

pub struct PcxPreview;

impl PreviewPlugin for PcxPreview {
    fn name(&self) -> &str {
        ".pcx image"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        if !filename.to_ascii_lowercase().ends_with(".pcx") {
            return false;
        }
        data.len() >= 128 && data[0] == 0x0A
    }

    fn build(&self, _filename: &str, data: &[u8], ctx: &egui::Context) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        match decode_pcx(data) {
            Ok((pixels, w, h)) => {
                let color_image =
                    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
                out.texture = Some(ctx.load_texture(
                    "pcx_preview",
                    color_image,
                    egui::TextureOptions::default(),
                ));
                out.texture_pixels = Some((pixels, w, h));
                let diag = pcx_diagnostics(data);
                out.text = Some(format!("PCX image {w}×{h}\n{diag}"));
                out.extra_exports.push(ExportAction {
                    label: "Export As PNG",
                    default_extension: "png",
                    filter_name: "PNG image",
                    build: Arc::new(|data| {
                        let (pixels, w, h) =
                            decode_pcx(data).map_err(|e| format!("pcx decode: {e}"))?;
                        crate::preview::encode_png(&pixels, w, h)
                    }),
                });
            }
            Err(e) => {
                out.text = Some(format!("PCX header parsed, but decoding failed: {e}"));
            }
        }

        out
    }
}

fn pcx_diagnostics(data: &[u8]) -> String {
    if data.len() < 128 {
        return "too small".into();
    }
    let version = data[1];
    let encoding = data[2];
    let bpp = data[3];
    let num_planes = data[65];
    let bpl = u16::from_le_bytes([data[66], data[67]]);
    let trailer_marker = data.len() >= 769 && data[data.len() - 769] == 0x0C;

    let mut pal_sample = String::new();
    if trailer_marker {
        let start = data.len() - 768;
        let mut max_v = 0u8;
        for i in 0..8 {
            let r = data[start + i * 3];
            let g = data[start + i * 3 + 1];
            let b = data[start + i * 3 + 2];
            max_v = max_v.max(r).max(g).max(b);
            pal_sample.push_str(&format!("{i}:({r},{g},{b}) "));
        }
        pal_sample.push_str(&format!("max={max_v}"));
    }

    format!(
        "ver={version} enc={encoding} bpp={bpp} planes={num_planes} bpl={bpl} trailer_0x0C={trailer_marker}\nfile={}B  pal[0..8]: {}",
        data.len(),
        pal_sample
    )
}

/// Minimal PCX decoder. Returns `(rgba_pixels, width, height)`.
fn decode_pcx(data: &[u8]) -> Result<(Vec<u8>, u32, u32), &'static str> {
    if data.len() < 128 || data[0] != 0x0A {
        return Err("not a PCX file");
    }

    let encoding = data[2];
    let bpp = data[3] as u32;
    let xmin = u16::from_le_bytes([data[4], data[5]]) as u32;
    let ymin = u16::from_le_bytes([data[6], data[7]]) as u32;
    let xmax = u16::from_le_bytes([data[8], data[9]]) as u32;
    let ymax = u16::from_le_bytes([data[10], data[11]]) as u32;
    let num_planes = data[65] as u32;
    let bytes_per_line = u16::from_le_bytes([data[66], data[67]]) as u32;

    if encoding != 1 {
        return Err("unsupported PCX encoding (expected RLE)");
    }
    if bpp != 8 {
        return Err("unsupported PCX bits-per-pixel (expected 8)");
    }
    if num_planes != 1 {
        return Err("unsupported PCX plane count (expected 1)");
    }

    let width = xmax.wrapping_sub(xmin) + 1;
    let height = ymax.wrapping_sub(ymin) + 1;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("invalid PCX dimensions");
    }

    let total_scanline_bytes = (bytes_per_line as usize) * (height as usize);
    let mut raw = Vec::with_capacity(total_scanline_bytes);
    let mut i = 128usize;
    while raw.len() < total_scanline_bytes {
        if i >= data.len() {
            return Err("PCX RLE stream truncated");
        }
        let b = data[i];
        i += 1;
        if b & 0xC0 == 0xC0 {
            let run = (b & 0x3F) as usize;
            if i >= data.len() {
                return Err("PCX RLE run missing value byte");
            }
            let v = data[i];
            i += 1;
            for _ in 0..run {
                raw.push(v);
                if raw.len() >= total_scanline_bytes {
                    break;
                }
            }
        } else {
            raw.push(b);
        }
    }

    // 256-color palette trailer: last 769 bytes of the file, first byte = 0x0C.
    let palette: [u8; 768] = if data.len() >= 769 && data[data.len() - 769] == 0x0C {
        let start = data.len() - 768;
        let mut p = [0u8; 768];
        p.copy_from_slice(&data[start..]);
        p
    } else {
        // Fallback: grayscale ramp.
        let mut p = [0u8; 768];
        for i in 0..256 {
            p[i * 3] = i as u8;
            p[i * 3 + 1] = i as u8;
            p[i * 3 + 2] = i as u8;
        }
        p
    };

    let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for row in 0..height as usize {
        let row_off = row * bytes_per_line as usize;
        for col in 0..width as usize {
            let idx = raw[row_off + col] as usize;
            let r = palette[idx * 3];
            let g = palette[idx * 3 + 1];
            let b = palette[idx * 3 + 2];
            rgba.extend_from_slice(&[r, g, b, 0xFF]);
        }
    }

    Ok((rgba, width, height))
}
