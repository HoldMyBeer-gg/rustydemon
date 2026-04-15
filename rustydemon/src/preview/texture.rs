//! D2R `.texture` preview plugin (`<DE(` container).
//!
//! D2R wraps its BC-compressed textures in a small Blizzard-specific
//! container that starts with the 4-byte magic `<DE(`.  The format was
//! worked out by dumping known-content samples (albedo, normal, mask,
//! flow, thickness, ORM, LUT) and reconciling byte patterns against BC
//! block layouts.  See the header walk-through below for the full map.
//!
//! Unlike the D4 `.tex` plugin, which has to guess dimensions from file
//! size, this plugin reads width/height/mip-count/mip0-offset directly
//! from the header, so decode is essentially deterministic.

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin};

pub struct TextureDePreview;

/// Parsed `<DE(` header — only the fields we actually use for decoding.
#[derive(Debug, Clone, Copy)]
struct DeHeader {
    format_code: u8,
    width: u32,
    height: u32,
    /// Absolute file offset of the first (largest) mip's BC payload.
    mip0_offset: usize,
    /// Size in bytes of the first mip.  Used to pick between BC1/BC4
    /// (8 bytes/block) and BC2/3/5/7 (16 bytes/block).
    mip0_size: usize,
}

const MAGIC: &[u8; 4] = b"<DE(";

/// Parse the header and locate mip 0.  Returns `None` on any structural
/// problem — the UI then falls back to the generic text/hex dump.
fn parse_header(data: &[u8]) -> Option<DeHeader> {
    if data.len() < 0x2C || &data[..4] != MAGIC {
        return None;
    }

    let format_code = data[4];
    let width = u32::from_le_bytes(data[0x08..0x0C].try_into().ok()?);
    let height = u32::from_le_bytes(data[0x0C..0x10].try_into().ok()?);
    let mip_count = u32::from_le_bytes(data[0x1C..0x20].try_into().ok()?);

    if mip_count == 0 || mip_count > 16 || width == 0 || height == 0 {
        return None;
    }
    if width > 16384 || height > 16384 {
        return None;
    }

    // Mip table lives at 0x24, one (size, self_rel_offset) pair per mip.
    let table_start = 0x24usize;
    let table_end = table_start.checked_add((mip_count as usize).checked_mul(8)?)?;
    if data.len() < table_end {
        return None;
    }

    // Read the first entry.
    let mip0_size =
        u32::from_le_bytes(data[table_start..table_start + 4].try_into().ok()?) as usize;
    let offset_field_pos = table_start + 4;
    let self_rel = u32::from_le_bytes(
        data[offset_field_pos..offset_field_pos + 4]
            .try_into()
            .ok()?,
    ) as usize;
    // Offset is measured from the address of the offset field itself.
    let mip0_offset = offset_field_pos.checked_add(self_rel)?;

    if mip0_offset.checked_add(mip0_size)? > data.len() {
        return None;
    }
    if mip0_size == 0 {
        return None;
    }

    Some(DeHeader {
        format_code,
        width,
        height,
        mip0_offset,
        mip0_size,
    })
}

/// Signature shared by every `texture2ddecoder::decode_bcN` function.
type BcDecodeFn = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;

/// One BC format candidate to try, in priority order.
#[derive(Clone, Copy)]
struct BcCandidate {
    name: &'static str,
    decode: BcDecodeFn,
}

const BC1: BcCandidate = BcCandidate {
    name: "BC1",
    decode: texture2ddecoder::decode_bc1,
};
const BC3: BcCandidate = BcCandidate {
    name: "BC3",
    decode: texture2ddecoder::decode_bc3,
};
const BC4: BcCandidate = BcCandidate {
    name: "BC4",
    decode: texture2ddecoder::decode_bc4,
};
const BC5: BcCandidate = BcCandidate {
    name: "BC5",
    decode: texture2ddecoder::decode_bc5,
};
const BC7: BcCandidate = BcCandidate {
    name: "BC7",
    decode: texture2ddecoder::decode_bc7,
};

/// Pick decode candidates for a given format code + filename hint.
///
/// The format code is the 5th byte of the header (the one I originally
/// mis-read as a closing `>`).  The table below came from eyeballing
/// known-content samples; unknown codes fall through to a block-size-
/// matched brute force that almost always resolves on the first try.
fn candidates_for(header: &DeHeader, filename: &str) -> Vec<BcCandidate> {
    let lower = filename.to_ascii_lowercase();

    // Block-size class is authoritative: we know it exactly from the
    // header, so we only try formats whose block size matches mip0's
    // bytes/block ratio (w*h/16 × block_bytes == mip0_size).
    let blocks = ((header.width.max(4) / 4) as usize) * ((header.height.max(4) / 4) as usize);
    let bytes_per_block = header
        .mip0_size
        .checked_div(blocks.max(1))
        .unwrap_or(header.mip0_size);

    let is_16b = bytes_per_block == 16;

    match (header.format_code, is_16b) {
        // Verified from block inspection on aluminum_alb (all-white BC3).
        (0x3E, true) => vec![BC3, BC7],

        // Observed on normal maps, masks, ORM, LUTs, gradients.  Default
        // to BC7 (D4/D2R-era general-purpose format), with a BC5 hint
        // for normal maps since two-channel encoding is common there.
        (0x3D, true) => {
            if lower.contains("_nrm") || lower.contains("_normal") {
                vec![BC5, BC7, BC3]
            } else {
                vec![BC7, BC3, BC5]
            }
        }

        // Flow maps (default_flow).  Two-channel vector field.
        (0x39, false) => vec![BC1, BC4],

        // Single-channel mask / scalar (default_hrt, default_thickness).
        (0x3A, false) | (0x3F, false) => vec![BC4, BC1],

        // Unknown code, 16-byte blocks: try the full "big BC" set.
        (_, true) => vec![BC7, BC3, BC5],

        // Unknown code, 8-byte blocks.
        (_, false) => vec![BC1, BC4],
    }
}

/// Decode mip 0 with the first candidate that produces a plausible image.
/// Returns `(rgba, width, height, format_name)` on success.
fn decode_mip0(data: &[u8], filename: &str) -> Option<(Vec<u8>, u32, u32, &'static str)> {
    let header = parse_header(data)?;
    let mip = &data[header.mip0_offset..header.mip0_offset + header.mip0_size];

    // Clamp to multiple of 4 for BC decoding.  BC blocks are always 4×4,
    // so sub-4 textures are padded — the decoder still writes `w*h`
    // output pixels so we pass the true dimensions through.
    let w = header.width as usize;
    let h = header.height as usize;

    for cand in candidates_for(&header, filename) {
        let mut rgba_u32 = vec![0u32; w * h];
        let ok = (cand.decode)(mip, w, h, &mut rgba_u32).is_ok();
        if !ok {
            continue;
        }
        if !crate::tex_preview::looks_valid_u32(&rgba_u32, w) {
            continue;
        }
        let rgba = crate::tex_preview::u32_to_rgba_bytes(&rgba_u32);
        return Some((rgba, header.width, header.height, cand.name));
    }

    None
}

impl PreviewPlugin for TextureDePreview {
    fn name(&self) -> &str {
        ".texture (D2R DE container)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        // Extension is a cheap early-out; magic is authoritative.  We
        // accept any extension as long as the magic matches — a few D2R
        // assets with non-.texture extensions use the same wrapper.
        if data.len() < 4 || &data[..4] != MAGIC {
            return false;
        }
        let _ = filename;
        true
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let header = match parse_header(data) {
            Some(h) => h,
            None => {
                out.text = Some("<DE( header could not be parsed.".into());
                return out;
            }
        };

        let Some((rgba, w, h, fmt)) = decode_mip0(data, filename) else {
            out.text = Some(format!(
                "<DE( texture {}×{} (format code 0x{:02X}) — no BC decoder produced a valid image.",
                header.width, header.height, header.format_code
            ));
            return out;
        };

        let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        out.texture = Some(ctx.load_texture(
            "texture_de_preview",
            color_image,
            egui::TextureOptions::default(),
        ));
        out.texture_pixels = Some((rgba, w, h));
        out.text = Some(format!(
            "D2R .texture  ({}×{}, {fmt}, code 0x{:02X})",
            w, h, header.format_code
        ));

        let filename = filename.to_owned();
        out.extra_exports.push(ExportAction {
            label: "Export As PNG",
            default_extension: "png",
            filter_name: "PNG image",
            build: Arc::new(move |data| {
                let (rgba, w, h, _fmt) = decode_mip0(data, &filename)
                    .ok_or_else(|| "texture decode failed".to_string())?;
                crate::preview::encode_png(&rgba, w, h)
            }),
        });

        out
    }
}
