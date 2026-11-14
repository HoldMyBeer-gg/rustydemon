//! Diablo III texture SNOs (`Base/Textures/*.tex`).
//!
//! ```text
//! 0x00  u32  0xDEADBEEF          SNO header
//! 0x04  u32  0x2F                every texture carries this, and so does
//!                                EffectGroup, so it is not a type tag
//! 0x10  u32  SNO ID
//! 0x20  u32  format
//! 0x24  u32  width
//! 0x28  u32  height
//! 0x2C  u32  faces (6 on cube maps)
//! 0x38  (u32 offset, u32 size) per mip, largest first; offsets count
//!       from byte 16
//! ```
//!
//! Block-compressed mips are stored field by field instead of block by
//! block: for BC1 every block's colour endpoints, then every block's
//! indices; for BC2 alpha, colour endpoints, then colour indices; for BC3
//! alpha endpoints, alpha indices, colour endpoints, then colour indices.
//! [`unplane`] weaves them back into ordinary blocks.
//!
//! | format | pixels |
//! |---|---|
//! | 0 | 32-bit BGRA |
//! | 9, 10 | BC1 |
//! | 11 | BC2 |
//! | 12, 43 | BC3 (43 is used by normal maps) |
//!
//! Formats 4, 7 and 23 (seven files on a full install) are not decoded.

const SNO_MAGIC: u32 = 0xDEAD_BEEF;
const TEXTURE_KIND: u32 = 0x2F;
const MIP_TABLE: usize = 0x38;
const DATA_BASE: usize = 16;

#[derive(Debug)]
pub struct Image {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: &'static str,
}

fn u32_at(data: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(o..o + 4)?.try_into().ok()?))
}

/// Whether the preview takes this file. The word after the magic is shared
/// with other SNO types (EffectGroup carries 0x2F too), so the name must
/// end in `.tex` as well.
pub fn claims(filename: &str, data: &[u8]) -> bool {
    filename.to_ascii_lowercase().ends_with(".tex") && looks_like_d3_texture(data)
}

pub fn looks_like_d3_texture(data: &[u8]) -> bool {
    u32_at(data, 0) == Some(SNO_MAGIC)
        && u32_at(data, 4) == Some(TEXTURE_KIND)
        && data.len() >= MIP_TABLE + 8
}

/// Weave planar block fields back into whole blocks. `planes` holds the
/// byte width of each field, in block order.
pub fn unplane(data: &[u8], planes: &[usize]) -> Vec<u8> {
    let block: usize = planes.iter().sum();
    let count = data.len() / block;
    let mut out = vec![0u8; count * block];
    let mut base = 0;
    let mut col = 0;
    for &width in planes {
        for i in 0..count {
            let src = base + i * width;
            out[i * block + col..i * block + col + width].copy_from_slice(&data[src..src + width]);
        }
        base += count * width;
        col += width;
    }
    out
}

type BlockDecoder = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;

/// Decode the largest mip to RGBA.
pub fn decode(data: &[u8]) -> Result<Image, String> {
    if !looks_like_d3_texture(data) {
        return Err("not a Diablo III texture".into());
    }
    let field = |o| u32_at(data, o).ok_or("header is truncated");
    let format = field(0x20)?;
    let (width, height) = (field(0x24)?, field(0x28)?);
    let (offset, size) = (field(MIP_TABLE)? as usize, field(MIP_TABLE + 4)? as usize);
    let start = DATA_BASE + offset;
    let pixels = data
        .get(start..start + size)
        .ok_or_else(|| format!("mip of {size} bytes at {start} runs past the end"))?;
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(format!("implausible size {width}x{height}"));
    }

    let (name, rgba) = match format {
        0 => {
            if pixels.len() < w * h * 4 {
                return Err("mip is smaller than its size says".into());
            }
            let rgba = pixels[..w * h * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[b, g, r, a]| [r, g, b, a])
                .collect();
            ("BGRA8", rgba)
        }
        9 | 10 => (
            "BC1",
            decode_blocks(pixels, w, h, &[4, 4], texture2ddecoder::decode_bc1)?,
        ),
        11 => (
            "BC2",
            decode_blocks(pixels, w, h, &[8, 4, 4], texture2ddecoder::decode_bc2)?,
        ),
        12 | 43 => (
            "BC3",
            decode_blocks(pixels, w, h, &[2, 6, 4, 4], texture2ddecoder::decode_bc3)?,
        ),
        other => return Err(format!("texture format {other} is not decoded yet")),
    };
    Ok(Image {
        rgba,
        width,
        height,
        format: name,
    })
}

/// Decode a planar block-compressed mip, padding to whole blocks and
/// cropping back to `w` x `h`.
fn decode_blocks(
    pixels: &[u8],
    w: usize,
    h: usize,
    planes: &[usize],
    decoder: BlockDecoder,
) -> Result<Vec<u8>, String> {
    let (bw, bh) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
    let need = (bw / 4) * (bh / 4) * planes.iter().sum::<usize>();
    if pixels.len() < need {
        return Err(format!(
            "mip holds {} bytes, {w}x{h} needs {need}",
            pixels.len()
        ));
    }
    let blocks = unplane(&pixels[..need], planes);
    let mut out = vec![0u32; bw * bh];
    decoder(&blocks, bw, bh, &mut out)?;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in out.chunks_exact(bw).take(h) {
        for &p in &row[..w] {
            let [b, g, r, a] = p.to_le_bytes();
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Ok(rgba)
}

pub struct D3TexPreview;

impl super::PreviewPlugin for D3TexPreview {
    fn name(&self) -> &str {
        "Diablo III texture"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        claims(filename, data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> super::PreviewOutput {
        let mut out = super::PreviewOutput::new();
        match decode(data) {
            Ok(img) => {
                let size = [img.width as usize, img.height as usize];
                out.texture = Some(ctx.load_texture(
                    "d3_texture",
                    egui::ColorImage::from_rgba_unmultiplied(size, &img.rgba),
                    egui::TextureOptions::default(),
                ));
                out.text = Some(format!(
                    "Diablo III texture {}×{}, {}",
                    img.width, img.height, img.format
                ));
                out.texture_pixels = Some((img.rgba, img.width, img.height));
                out.extra_exports.push(super::ExportAction {
                    label: "Export As PNG",
                    default_extension: "png",
                    filter_name: "PNG image",
                    build: std::sync::Arc::new(|data, _path| {
                        let img = decode(data)?;
                        super::encode_png(&img.rgba, img.width, img.height)
                    }),
                });
            }
            Err(e) => {
                out.text = Some(format!("Diablo III texture\n\nCould not decode it: {e}"));
            }
        }
        out
    }
}
