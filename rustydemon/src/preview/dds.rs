//! DDS textures, as StarCraft II and Heroes of the Storm ship them.
//!
//! ```text
//!   0  "DDS "
//!  12  height, width (u32 each)
//!  28  mip count
//!  80  pixel format flags: 0x4 a four letter code names the format,
//!      0x40 plain pixels, 0x1 the plain pixels have alpha
//!  84  the four letter code: DXT1, DXT3, DXT5
//!  88  bits per plain pixel
//!  92  red, green, blue and alpha masks of a plain pixel
//! 128  the largest mip, then the smaller ones
//! ```
//!
//! Only the largest mip is read.  DXT1, DXT3 and DXT5 are stored as 4 x 4
//! blocks, so a size that is not a multiple of four is padded in the file
//! and cropped here.  Plain pixels are 24 or 32 bits and their channels
//! are found through the masks.

use std::sync::Arc;

use super::{ExportAction, PreviewOutput, PreviewPlugin, SiblingFetcher};

/// A decoded texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Four bytes a pixel: red, green, blue, alpha.
    pub rgba: Vec<u8>,
    /// How the file stored it, for the text beside the picture.
    pub format: &'static str,
}

const HEADER: usize = 128;
/// No texture in these games comes near this; a larger size is a bad file.
const MAX_SIDE: usize = 16384;

const FLAG_ALPHA: u32 = 0x1;
const FLAG_FOURCC: u32 = 0x4;
const FLAG_RGB: u32 = 0x40;

pub fn claims(filename: &str, data: &[u8]) -> bool {
    filename.to_ascii_lowercase().ends_with(".dds") && data.starts_with(b"DDS ")
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// One 4 x 4 block to sixteen pixels, each a u32 of blue, green, red,
/// alpha from the low byte up.
type BlockDecoder = fn(&[u8], &mut [u32]);

/// Decode block-compressed pixels, padded in the file to whole blocks.
fn blocks(
    pixels: &[u8],
    width: usize,
    height: usize,
    block_bytes: usize,
    decode: BlockDecoder,
) -> Result<Vec<u8>, String> {
    let (across, down) = (width.div_ceil(4), height.div_ceil(4));
    let need = across * down * block_bytes;
    if pixels.len() < need {
        return Err(format!(
            "{width}x{height} needs {need} bytes of pixels, the file has {}",
            pixels.len()
        ));
    }
    let mut rgba = vec![0u8; width * height * 4];
    let mut block = [0u32; 16];
    for (n, bytes) in pixels[..need].chunks_exact(block_bytes).enumerate() {
        decode(bytes, &mut block);
        let (bx, by) = (n % across * 4, n / across * 4);
        for (k, pixel) in block.iter().enumerate() {
            let (x, y) = (bx + k % 4, by + k / 4);
            if x < width && y < height {
                let [b, g, r, a] = pixel.to_le_bytes();
                let at = (y * width + x) * 4;
                rgba[at..at + 4].copy_from_slice(&[r, g, b, a]);
            }
        }
    }
    Ok(rgba)
}

/// One channel of a plain pixel as 8 bits.  `None` when the mask is
/// empty.
fn channel(pixel: u32, mask: u32) -> Option<u8> {
    if mask == 0 {
        return None;
    }
    let value = (pixel & mask) >> mask.trailing_zeros();
    let max = mask >> mask.trailing_zeros();
    Some((u64::from(value) * 255 / u64::from(max)) as u8)
}

fn plain(
    pixels: &[u8],
    width: usize,
    height: usize,
    bytes: usize,
    masks: [u32; 4],
    alpha: bool,
) -> Result<Vec<u8>, String> {
    let need = width * height * bytes;
    if pixels.len() < need {
        return Err(format!(
            "{width}x{height} needs {need} bytes of pixels, the file has {}",
            pixels.len()
        ));
    }
    let mut rgba = Vec::with_capacity(width * height * 4);
    for p in pixels[..need].chunks_exact(bytes) {
        let mut raw = [0u8; 4];
        raw[..bytes].copy_from_slice(p);
        let pixel = u32::from_le_bytes(raw);
        rgba.extend_from_slice(&[
            channel(pixel, masks[0]).unwrap_or(0),
            channel(pixel, masks[1]).unwrap_or(0),
            channel(pixel, masks[2]).unwrap_or(0),
            channel(pixel, masks[3]).filter(|_| alpha).unwrap_or(255),
        ]);
    }
    Ok(rgba)
}

pub fn decode(data: &[u8]) -> Result<Image, String> {
    if data.len() < HEADER || !data.starts_with(b"DDS ") {
        return Err("not a DDS file".into());
    }
    let (height, width) = (u32_at(data, 12) as usize, u32_at(data, 16) as usize);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("{width}x{height} is not a size a texture has"));
    }
    let flags = u32_at(data, 80);
    let pixels = &data[HEADER..];

    let (rgba, format) = if flags & FLAG_FOURCC != 0 {
        let code = &data[84..88];
        let (block_bytes, decoder, format): (usize, BlockDecoder, &'static str) = match code {
            b"DXT1" => (8, texture2ddecoder::decode_bc1_block, "DXT1"),
            b"DXT2" | b"DXT3" => (16, texture2ddecoder::decode_bc2_block, "DXT3"),
            b"DXT4" | b"DXT5" => (16, texture2ddecoder::decode_bc3_block, "DXT5"),
            _ => {
                return Err(format!(
                    "format {} is not read yet",
                    String::from_utf8_lossy(code)
                ))
            }
        };
        (blocks(pixels, width, height, block_bytes, decoder)?, format)
    } else if flags & FLAG_RGB != 0 {
        let masks = [92, 96, 100, 104].map(|at| u32_at(data, at));
        let alpha = flags & FLAG_ALPHA != 0;
        match u32_at(data, 88) {
            32 => (plain(pixels, width, height, 4, masks, alpha)?, "32 bit"),
            24 => (plain(pixels, width, height, 3, masks, alpha)?, "24 bit"),
            bits => return Err(format!("{bits} bit pixels are not read yet")),
        }
    } else {
        return Err(format!("pixel format flags {flags:#x} are not read yet"));
    };

    Ok(Image {
        width: width as u32,
        height: height as u32,
        rgba,
        format,
    })
}

pub struct DdsPreview;

impl PreviewPlugin for DdsPreview {
    fn name(&self) -> &str {
        ".dds texture"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        claims(filename, data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        _siblings: &SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        match decode(data) {
            Ok(img) => {
                let size = [img.width as usize, img.height as usize];
                out.texture = Some(ctx.load_texture(
                    "dds_texture",
                    egui::ColorImage::from_rgba_unmultiplied(size, &img.rgba),
                    egui::TextureOptions::default(),
                ));
                let mips = u32_at(data, 28).max(1);
                out.text = Some(format!(
                    "DDS texture {}×{}, {}, {mips} mip{}",
                    img.width,
                    img.height,
                    img.format,
                    if mips == 1 { "" } else { "s" }
                ));
                out.texture_pixels = Some((img.rgba, img.width, img.height));
                out.extra_exports.push(ExportAction {
                    label: "Export As PNG",
                    default_extension: "png",
                    filter_name: "PNG image",
                    build: Arc::new(|data, _path| {
                        let img = decode(data)?;
                        super::encode_png(&img.rgba, img.width, img.height)
                    }),
                });
            }
            Err(e) => {
                out.text = Some(format!("DDS texture\n\nCould not decode it: {e}"));
            }
        }
        out
    }
}
