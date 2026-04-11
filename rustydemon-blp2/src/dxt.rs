// Ported from SereniaBLPLib / LibSquish (libsquish.googlecode.com)

/// Selects which DXT variant to decompress.
///
/// Chosen by [`BlpFile::get_pixels`] based on the `alpha_size` and
/// `preferred_format` fields in the BLP header:
///
/// | `alpha_size` | `preferred_format` | Variant |
/// |---|---|---|
/// | `0` or `1` | any | [`Dxt1`](DxtFlags::Dxt1) |
/// | `> 1` | [`Dxt5`](crate::PixelFormat::Dxt5) | [`Dxt5`](DxtFlags::Dxt5) |
/// | `> 1` | anything else | [`Dxt3`](DxtFlags::Dxt3) |
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DxtFlags {
    /// DXT1 (BC1) — 8 bytes per 4×4 block, 0- or 1-bit alpha.
    Dxt1,
    /// DXT3 (BC2) — 16 bytes per 4×4 block, explicit 4-bit alpha.
    Dxt3,
    /// DXT5 (BC3) — 16 bytes per 4×4 block, interpolated 8-bit alpha.
    Dxt5,
}

/// Decompresses a DXT-compressed image into raw RGBA pixel data.
///
/// Returns `None` if `width * height * 4` overflows `usize`, which the caller
/// should surface as [`BlpError::ImageTooLarge`](crate::BlpError::ImageTooLarge).
///
/// Partial blocks (where the data ends before a full 8- or 16-byte block has
/// been read) are silently skipped — the corresponding pixels remain zeroed.
/// This matches the behaviour of the original SereniaBLPLib, where truncated
/// mipmap data produces a black region rather than a crash.
pub fn decompress_image(width: u32, height: u32, data: &[u8], flags: DxtFlags) -> Option<Vec<u8>> {
    let n_pixels = (width as usize).checked_mul(height as usize)?;
    let n_bytes = n_pixels.checked_mul(4)?;

    let mut rgba = vec![0u8; n_bytes];
    let bytes_per_block: usize = if flags == DxtFlags::Dxt1 { 8 } else { 16 };
    let mut source_pos = 0usize;
    let mut target_rgba = [0u8; 64]; // 4 bytes × 16 pixels per 4×4 block

    for y in (0..height).step_by(4) {
        for x in (0..width).step_by(4) {
            // Skip incomplete blocks rather than indexing out of bounds.
            if source_pos.saturating_add(bytes_per_block) > data.len() {
                continue;
            }

            decompress_block(&mut target_rgba, data, source_pos, flags);

            // Copy only the pixels that fall inside the image boundary.
            // For images whose dimensions are not multiples of 4, the block
            // may extend beyond the right or bottom edge.
            let mut target_pos = 0usize;
            for py in 0..4u32 {
                for px in 0..4u32 {
                    let sx = x + px;
                    let sy = y + py;
                    if sx < width && sy < height {
                        let dst = 4 * (width * sy + sx) as usize;
                        rgba[dst..dst + 4]
                            .copy_from_slice(&target_rgba[target_pos..target_pos + 4]);
                    }
                    target_pos += 4;
                }
            }

            source_pos += bytes_per_block;
        }
    }

    Some(rgba)
}

// ── Block decompression ───────────────────────────────────────────────────────

/// Decompresses one 4×4 block into 64 bytes of RGBA data.
///
/// For DXT3 and DXT5 the first 8 bytes of the block contain alpha information;
/// the colour data begins at byte 8. For DXT1 the colour data starts at byte 0.
fn decompress_block(rgba: &mut [u8; 64], block: &[u8], block_idx: usize, flags: DxtFlags) {
    let color_idx = match flags {
        DxtFlags::Dxt3 | DxtFlags::Dxt5 => block_idx + 8,
        DxtFlags::Dxt1 => block_idx,
    };

    decompress_color(rgba, block, color_idx, flags == DxtFlags::Dxt1);

    match flags {
        DxtFlags::Dxt3 => decompress_alpha_dxt3(rgba, block, block_idx),
        DxtFlags::Dxt5 => decompress_alpha_dxt5(rgba, block, block_idx),
        DxtFlags::Dxt1 => {}
    }
}

// ── Color decompression ───────────────────────────────────────────────────────

/// Unpacks one RGB565-encoded endpoint into a `colour` slice at `colour_offset`.
///
/// Returns the raw 16-bit packed value so the caller can compare endpoints to
/// select the correct interpolation codebook.
fn unpack565(
    block: &[u8],
    block_idx: usize,
    packed_offset: usize,
    colour: &mut [u8],
    colour_offset: usize,
) -> u16 {
    let lo = block[block_idx + packed_offset] as u16;
    let hi = block[block_idx + packed_offset + 1] as u16;
    let value = lo | (hi << 8);

    // Expand 5-6-5 components to 8-bit by replicating the high bits into the low bits.
    let red = ((value >> 11) & 0x1F) as u8;
    let green = ((value >> 5) & 0x3F) as u8;
    let blue = (value & 0x1F) as u8;

    colour[colour_offset] = (red << 3) | (red >> 2);
    colour[colour_offset + 1] = (green << 2) | (green >> 4);
    colour[colour_offset + 2] = (blue << 3) | (blue >> 2);
    colour[colour_offset + 3] = 255;

    value
}

/// Decompresses the 8-byte colour portion of any DXT block.
///
/// Builds a 4-colour codebook from two RGB565 endpoints, then maps 2-bit
/// per-pixel indices to RGBA values.
///
/// When `is_dxt1 && endpoint0 <= endpoint1`, the fourth codebook entry is
/// transparent black (the "punch-through alpha" mode of DXT1).
fn decompress_color(rgba: &mut [u8; 64], block: &[u8], block_idx: usize, is_dxt1: bool) {
    let mut codes = [0u8; 16]; // 4 RGBA entries × 4 bytes
    let a = unpack565(block, block_idx, 0, &mut codes, 0);
    let b = unpack565(block, block_idx, 2, &mut codes, 4);

    for i in 0..3 {
        let c = codes[i] as i32;
        let d = codes[4 + i] as i32;

        if is_dxt1 && a <= b {
            // Punch-through alpha: midpoint colour + transparent black.
            codes[8 + i] = ((c + d) / 2) as u8;
            codes[12 + i] = 0;
        } else {
            // Standard: two interpolated colours.
            codes[8 + i] = ((2 * c + d) / 3) as u8;
            codes[12 + i] = ((c + 2 * d) / 3) as u8;
        }
    }

    codes[8 + 3] = 255;
    codes[12 + 3] = if is_dxt1 && a <= b { 0 } else { 255 };

    // Unpack the 16 2-bit indices (4 bytes, 4 indices per byte, LSB first).
    let mut indices = [0u8; 16];
    for i in 0..4 {
        let packed = block[block_idx + 4 + i];
        indices[i * 4] = packed & 0x3;
        indices[i * 4 + 1] = (packed >> 2) & 0x3;
        indices[i * 4 + 2] = (packed >> 4) & 0x3;
        indices[i * 4 + 3] = (packed >> 6) & 0x3;
    }

    for i in 0..16 {
        let offset = 4 * indices[i] as usize;
        rgba[4 * i..4 * i + 4].copy_from_slice(&codes[offset..offset + 4]);
    }
}

// ── Alpha decompression ───────────────────────────────────────────────────────

/// Decompresses the 8-byte DXT3 alpha block.
///
/// Each byte contains two 4-bit alpha values. They are expanded to 8-bit by
/// duplicating the nibble into both the high and low halves of the output byte:
/// `lo | (lo << 4)` and `hi | (hi >> 4)`.
fn decompress_alpha_dxt3(rgba: &mut [u8; 64], block: &[u8], block_idx: usize) {
    for i in 0..8 {
        let quant = block[block_idx + i];
        let lo = quant & 0x0F;
        let hi = quant & 0xF0;
        rgba[8 * i + 3] = lo | (lo << 4);
        rgba[8 * i + 7] = hi | (hi >> 4);
    }
}

/// Decompresses the 8-byte DXT5 alpha block.
///
/// Two 8-bit endpoint values are followed by 6 bytes of packed 3-bit indices
/// (8 indices per 3-byte group, 16 indices total). The endpoints define an
/// interpolated codebook of up to 8 values:
///
/// * If `alpha0 <= alpha1`: 5-value codebook plus explicit `0` and `255`.
/// * If `alpha0 > alpha1`: 7-value interpolated codebook.
fn decompress_alpha_dxt5(rgba: &mut [u8; 64], block: &[u8], block_idx: usize) {
    let alpha0 = block[block_idx];
    let alpha1 = block[block_idx + 1];

    let mut codes = [0u8; 8];
    codes[0] = alpha0;
    codes[1] = alpha1;

    if alpha0 <= alpha1 {
        for i in 1..5usize {
            codes[1 + i] = (((5 - i) as u32 * alpha0 as u32 + i as u32 * alpha1 as u32) / 5) as u8;
        }
        codes[6] = 0;
        codes[7] = 255;
    } else {
        for i in 1..7usize {
            codes[i + 1] = (((7 - i) as u32 * alpha0 as u32 + i as u32 * alpha1 as u32) / 7) as u8;
        }
    }

    // Decode 16 three-bit indices packed into 6 bytes (2 groups of 3 bytes).
    let mut indices = [0u8; 16];
    let mut src_pos = 2usize;
    let mut idx_pos = 0usize;
    for _ in 0..2 {
        let mut value = 0u32;
        for j in 0..3usize {
            value |= (block[block_idx + src_pos] as u32) << (8 * j);
            src_pos += 1;
        }
        for j in 0..8usize {
            indices[idx_pos] = ((value >> (3 * j)) & 0x07) as u8;
            idx_pos += 1;
        }
    }

    for i in 0..16 {
        rgba[4 * i + 3] = codes[indices[i] as usize];
    }
}
