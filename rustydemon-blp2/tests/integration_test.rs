/// Integration tests for the `blp` crate.
///
/// The original C# SereniaBLPLib had no tests at all.  This suite covers:
///   1. Correctness  — each encoding/format produces correct RGBA output
///   2. Error paths  — invalid / truncated / corrupt input returns errors, not panics
///   3. Security     — malicious field values cannot cause panics, OOB, or overflow
use rustydemon_blp2::{BlpError, BlpFile, ColorEncoding};

// ─────────────────────────────────────────────────────────────────────────────
// Synthetic BLP builder helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Byte offsets inside a BLP2 file (used by tests that mutate specific fields).
mod offsets {
    pub const FORMAT_VERSION: usize = 4;
    pub const COLOR_ENCODING: usize = 8;
    /// First mip offset (u32 LE) in the 16-entry offset table.
    pub const MIP_OFFSET_0: usize = 20;
    /// First mip size (u32 LE) in the 16-entry size table.
    pub const MIP_SIZE_0: usize = 84;
}

/// BLP2 header (20 bytes): magic + version + 4 one-byte fields + width + height
fn blp2_header(color_enc: u8, alpha_size: u8, pf: u8, width: i32, height: i32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"BLP2");
    v.extend_from_slice(&1u32.to_le_bytes()); // format version
    v.push(color_enc);
    v.push(alpha_size);
    v.push(pf);
    v.push(0u8); // has_mipmaps
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&height.to_le_bytes());
    v
}

/// Appends the 16-entry offset table and 16-entry size table (128 bytes total).
fn append_mip_tables(v: &mut Vec<u8>, first_offset: u32, first_size: u32) {
    v.extend_from_slice(&first_offset.to_le_bytes());
    for _ in 1..16 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }
    v.extend_from_slice(&first_size.to_le_bytes());
    for _ in 1..16 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }
}

/// Returns a BLP2 palette file.
///
/// Layout (BLP2, Palette encoding):
///   0..20   header
///   20..84  mip_offsets
///   84..148 mip_sizes
///   148..1172 palette (256 × i32 BGRA)
///   1172..   mip_data
///
/// `palette_bgra` — 256 × 4 bytes (B, G, R, A per entry).
/// `mip_data`     — raw palette-index bytes + optional alpha bytes.
fn make_blp2_palette(
    width: i32,
    height: i32,
    alpha_size: u8,
    palette_bgra: &[[u8; 4]; 256],
    mip_data: Vec<u8>,
) -> Vec<u8> {
    const DATA_OFFSET: u32 = 1172;
    let mut v = blp2_header(1 /*Palette*/, alpha_size, 0, width, height);
    append_mip_tables(&mut v, DATA_OFFSET, mip_data.len() as u32);

    // Palette: 256 × i32 (BGRA stored as 4 LE bytes)
    for entry in palette_bgra {
        v.extend_from_slice(entry);
    }
    assert_eq!(v.len(), 1172);
    v.extend_from_slice(&mip_data);
    v
}

/// Builds a default 256-entry palette where entry `idx` is the given BGRA color.
fn palette_with_one(idx: u8, bgra: [u8; 4]) -> [[u8; 4]; 256] {
    let mut p = [[0u8; 4]; 256];
    p[idx as usize] = bgra;
    p
}

/// BLP2 DXT file.
///
/// Layout (BLP2, DXT encoding, no palette):
///   0..20   header
///   20..148 mip_tables
///   148..   mip_data
fn make_blp2_dxt(width: i32, height: i32, alpha_size: u8, pf: u8, mip_data: Vec<u8>) -> Vec<u8> {
    const DATA_OFFSET: u32 = 148;
    let mut v = blp2_header(2 /*Dxt*/, alpha_size, pf, width, height);
    append_mip_tables(&mut v, DATA_OFFSET, mip_data.len() as u32);
    v.extend_from_slice(&mip_data);
    v
}

/// BLP2 ARGB8888 file.
fn make_blp2_argb8888(width: i32, height: i32, mip_data: Vec<u8>) -> Vec<u8> {
    const DATA_OFFSET: u32 = 148;
    let mut v = blp2_header(3 /*Argb8888*/, 8, 2, width, height);
    append_mip_tables(&mut v, DATA_OFFSET, mip_data.len() as u32);
    v.extend_from_slice(&mip_data);
    v
}

/// A single DXT1 block that encodes a 4×4 solid-color image.
///
/// endpoint0 (RGB565) > endpoint1 (0x0000), indices all 0 → all pixels = endpoint0.
fn dxt1_solid_block(r: u8, g: u8, b: u8) -> [u8; 8] {
    // Encode the color as RGB565.
    let r5 = (r >> 3) as u16;
    let g6 = (g >> 2) as u16;
    let b5 = (b >> 3) as u16;
    let rgb565: u16 = (r5 << 11) | (g6 << 5) | b5;

    // endpoint0 must be > endpoint1 so the decoder uses the 4-color (opaque) codebook.
    // endpoint1 = 0 (black) satisfies this as long as rgb565 > 0.
    let ep0 = rgb565.max(1); // ensure ep0 > 0 = ep1
    let [lo, hi] = ep0.to_le_bytes();
    [lo, hi, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
}

// ─────────────────────────────────────────────────────────────────────────────
// Correctness tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn palette_1x1_red_no_alpha() {
    // palette[0] = R=255 G=0 B=0 A=255 stored as BGRA bytes
    let bgra_red: [u8; 4] = [0x00, 0x00, 0xFF, 0xFF];
    let data = make_blp2_palette(1, 1, 0, &palette_with_one(0, bgra_red), vec![0x00]);
    let blp = BlpFile::from_bytes(data).unwrap();

    assert_eq!(blp.mipmap_count(), 1);
    assert_eq!(blp.width, 1);
    assert_eq!(blp.height, 1);
    assert_eq!(blp.color_encoding, ColorEncoding::Palette);

    let (pixels, w, h) = blp.get_pixels(0).unwrap();
    assert_eq!((w, h), (1, 1));
    assert_eq!(&pixels, &[255, 0, 0, 255]); // RGBA red, fully opaque
}

#[test]
fn palette_2x2_green_alpha8() {
    // palette[1] = R=0 G=128 B=0 A=0 (alpha ignored; we use per-pixel alpha)
    let mut palette = [[0u8; 4]; 256];
    palette[1] = [0x00, 0x80, 0x00, 0x00]; // BGRA: B=0,G=128,R=0,A=0

    // 2×2 image: 4 pixels all using palette index 1.
    // alpha_size=8 → 4 alpha bytes follow the 4 index bytes.
    // alpha values: 0, 64, 128, 255
    let mip_data: Vec<u8> = vec![
        1, 1, 1, 1, // pixel indices
        0, 64, 128, 255, // alpha channel (8-bit per pixel)
    ];
    let data = make_blp2_palette(2, 2, 8, &palette, mip_data);
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, w, h) = blp.get_pixels(0).unwrap();

    assert_eq!((w, h), (2, 2));
    assert_eq!(pixels[0..4], [0, 128, 0, 0]); // pixel 0: RGBA
    assert_eq!(pixels[4..8], [0, 128, 0, 64]);
    assert_eq!(pixels[8..12], [0, 128, 0, 128]);
    assert_eq!(pixels[12..16], [0, 128, 0, 255]);
}

#[test]
fn palette_2x2_alpha1() {
    // 4 pixels, alpha_size=1 → 1 bit/pixel packed → ceil(4/8)=1 byte.
    // Bit 0 = pixel 0 alpha, bit 1 = pixel 1 alpha, etc.
    // byte = 0b0000_1010 → pixels 1,3 opaque; 0,2 transparent.
    let mut palette = [[0u8; 4]; 256];
    palette[0] = [0xFF, 0x00, 0x00, 0x00]; // blue
    let mip_data: Vec<u8> = vec![
        0,
        0,
        0,
        0,           // indices
        0b0000_1010, // alpha bits
    ];
    let data = make_blp2_palette(2, 2, 1, &palette, mip_data);
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, _, _) = blp.get_pixels(0).unwrap();

    assert_eq!(pixels[3], 0x00); // pixel 0 transparent
    assert_eq!(pixels[7], 0xFF); // pixel 1 opaque
    assert_eq!(pixels[11], 0x00); // pixel 2 transparent
    assert_eq!(pixels[15], 0xFF); // pixel 3 opaque
}

#[test]
fn palette_4x1_alpha4() {
    // 4 pixels, alpha_size=4 → 4 bits/pixel → ceil(4/2)=2 bytes.
    // Byte 0: lo nibble = pixel 0 alpha, hi nibble = pixel 1 alpha.
    // Byte 1: lo nibble = pixel 2 alpha, hi nibble = pixel 3 alpha.
    // The library stores it as: even pixel → (byte & 0x0F) << 4, odd pixel → byte & 0xF0.
    // So low nibble 0x5 → 0x50; high nibble 0xA0 → 0xA0.
    let mut palette = [[0u8; 4]; 256];
    palette[0] = [0x00, 0x00, 0xFF, 0x00]; // red
    let mip_data: Vec<u8> = vec![
        0, 0, 0, 0, // indices
        0xA5, 0xF0, // alpha bytes
    ];
    let data = make_blp2_palette(4, 1, 4, &palette, mip_data);
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, w, h) = blp.get_pixels(0).unwrap();

    assert_eq!((w, h), (4, 1));
    // pixel 0: even, low nibble of 0xA5 = 0x5 → (0x5 & 0x0F) << 4 = 0x50
    assert_eq!(pixels[3], 0x50);
    // pixel 1: odd, high nibble of 0xA5 = 0xA0 → 0xA5 & 0xF0 = 0xA0
    assert_eq!(pixels[7], 0xA0);
    // pixel 2: even, low nibble of 0xF0 = 0x0 → 0x00
    assert_eq!(pixels[11], 0x00);
    // pixel 3: odd, high nibble of 0xF0 = 0xF0 → 0xF0
    assert_eq!(pixels[15], 0xF0);
}

#[test]
fn dxt1_4x4_red() {
    // One DXT1 block encoding a solid red 4×4 image.
    let block = dxt1_solid_block(255, 0, 0);
    let data = make_blp2_dxt(4, 4, 0, 0 /*Dxt1*/, block.to_vec());
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, w, h) = blp.get_pixels(0).unwrap();

    assert_eq!((w, h), (4, 4));
    for chunk in pixels.chunks_exact(4) {
        // DXT1 round-trips through RGB565, so reconstruct the expected value.
        let r5: u8 = 255 >> 3; // 31
        let expected_r: u8 = (r5 << 3) | (r5 >> 2); // 255
        assert_eq!(chunk[0], expected_r, "red channel mismatch");
        assert_eq!(chunk[1], 0, "green should be 0");
        assert_eq!(chunk[2], 0, "blue should be 0");
        assert_eq!(chunk[3], 255, "alpha should be opaque");
    }
}

#[test]
fn dxt1_non_power_of_two_2x2() {
    // A 2×2 image with one DXT1 block (4×4 logical block, but only 2×2 pixels valid).
    let block = dxt1_solid_block(0, 0, 255); // blue
    let data = make_blp2_dxt(2, 2, 0, 0, block.to_vec());
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, w, h) = blp.get_pixels(0).unwrap();

    assert_eq!((w, h), (2, 2));
    assert_eq!(pixels.len(), 2 * 2 * 4);
    // Blue channel should be non-zero for all 4 pixels.
    for chunk in pixels.chunks_exact(4) {
        assert!(chunk[2] > 0, "blue channel should be non-zero");
    }
}

#[test]
fn argb8888_1x1_red_bgra_swap() {
    // On-disk BGRA (B=0, G=0, R=255, A=255) → decoded RGBA (255,0,0,255).
    let mip_data = vec![0x00u8, 0x00, 0xFF, 0xFF]; // BGRA red
    let data = make_blp2_argb8888(1, 1, mip_data);
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, w, h) = blp.get_pixels(0).unwrap();

    assert_eq!((w, h), (1, 1));
    assert_eq!(&pixels, &[255, 0, 0, 255]); // RGBA after swap
}

#[test]
fn argb8888_2x1_two_colors() {
    // Two pixels: BGRA red and BGRA blue on disk → RGBA after swap.
    let mip_data = vec![
        0x00, 0x00, 0xFF, 0xFF, // BGRA red
        0xFF, 0x00, 0x00, 0xFF, // BGRA blue
    ];
    let data = make_blp2_argb8888(2, 1, mip_data);
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, w, h) = blp.get_pixels(0).unwrap();

    assert_eq!((w, h), (2, 1));
    assert_eq!(&pixels[0..4], &[255, 0, 0, 255]); // RGBA red
    assert_eq!(&pixels[4..8], &[0, 0, 255, 255]); // RGBA blue
}

#[test]
fn mipmap_count_from_offset_table() {
    // Build a palette file with 3 mip levels.
    // Palette BGRA data offset: 1172 bytes.
    // Mip 0: 4 pixels (2×2), offset 1172, size 4.
    // Mip 1: 1 pixel (1×1), offset 1176, size 1.
    // Mip 2: 1 pixel (1×1), offset 1177, size 1.
    let palette = [[0u8; 4]; 256];
    const BASE: u32 = 1172;
    let mut v = blp2_header(1 /*Palette*/, 0, 0, 2, 2);

    // mip_offsets
    v.extend_from_slice(&BASE.to_le_bytes());
    v.extend_from_slice(&(BASE + 4).to_le_bytes());
    v.extend_from_slice(&(BASE + 5).to_le_bytes());
    for _ in 3..16 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }
    // mip_sizes
    v.extend_from_slice(&4u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    for _ in 3..16 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }

    for entry in &palette {
        v.extend_from_slice(entry);
    }
    v.extend_from_slice(&[0u8, 0, 0, 0, 0, 0]); // mip data for all 3 levels

    let blp = BlpFile::from_bytes(v).unwrap();
    assert_eq!(blp.mipmap_count(), 3);
}

#[test]
fn mipmap_level_clamped_to_last() {
    let bgra_red: [u8; 4] = [0x00, 0x00, 0xFF, 0xFF];
    let data = make_blp2_palette(1, 1, 0, &palette_with_one(0, bgra_red), vec![0x00]);
    let blp = BlpFile::from_bytes(data).unwrap();

    // Level 99 should clamp to 0 (only one mip).
    let (pixels, w, h) = blp.get_pixels(99).unwrap();
    assert_eq!((w, h), (1, 1));
    assert_eq!(&pixels, &[255, 0, 0, 255]);
}

#[test]
fn blp1_palette_1x1() {
    // BLP1 header layout:
    //   0..4   magic "BLP1"
    //   4..8   color_encoding (i32): 1 = Palette
    //   8..12  alpha_size (i32): 0
    //   12..16 width (i32): 1
    //   16..20 height (i32): 1
    //   20..24 preferred_format (i32): 0
    //   24..28 has_mipmaps (i32): 0
    //   28..92  mip_offsets (16 × u32)
    //   92..156 mip_sizes (16 × u32)
    //   156..1180 palette
    //   1180     mip_data
    const DATA_OFFSET: u32 = 1180;
    let mut v: Vec<u8> = Vec::new();
    v.extend_from_slice(b"BLP1");
    for val in [1i32, 0, 1, 1, 0, 0] {
        v.extend_from_slice(&val.to_le_bytes());
    }
    // mip_offsets
    v.extend_from_slice(&DATA_OFFSET.to_le_bytes());
    for _ in 1..16 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }
    // mip_sizes
    v.extend_from_slice(&1u32.to_le_bytes());
    for _ in 1..16 {
        v.extend_from_slice(&0u32.to_le_bytes());
    }
    // palette[0] = BGRA blue
    v.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF]);
    for _ in 1..256 {
        v.extend_from_slice(&[0u8; 4]);
    }
    // mip data
    v.push(0x00);

    let blp = BlpFile::from_bytes(v).unwrap();
    assert_eq!(blp.mipmap_count(), 1);
    let (pixels, w, h) = blp.get_pixels(0).unwrap();
    assert_eq!((w, h), (1, 1));
    // palette[0] BGRA [0xFF,0x00,0x00,0xFF] → R=0x00, G=0x00, B=0xFF, A=0xFF → RGBA [0,0,255,255]
    assert_eq!(&pixels, &[0, 0, 255, 255]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Error path tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn error_empty_input() {
    assert!(BlpFile::from_bytes(vec![]).is_err());
}

#[test]
fn error_three_bytes() {
    assert!(BlpFile::from_bytes(vec![b'B', b'L', b'P']).is_err());
}

#[test]
fn error_invalid_magic() {
    let data = b"JUNK\x00\x00\x00\x00".to_vec();
    assert!(matches!(
        BlpFile::from_bytes(data),
        Err(BlpError::InvalidMagic)
    ));
}

#[test]
fn error_blp2_bad_format_version_0() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0]);
    data[offsets::FORMAT_VERSION..offsets::FORMAT_VERSION + 4].copy_from_slice(&0u32.to_le_bytes());
    assert!(matches!(
        BlpFile::from_bytes(data),
        Err(BlpError::InvalidFormatVersion(0))
    ));
}

#[test]
fn error_blp2_bad_format_version_2() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0]);
    data[offsets::FORMAT_VERSION..offsets::FORMAT_VERSION + 4].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        BlpFile::from_bytes(data),
        Err(BlpError::InvalidFormatVersion(2))
    ));
}

#[test]
fn error_unknown_color_encoding() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0]);
    data[offsets::COLOR_ENCODING] = 9; // not a valid ColorEncoding value
    assert!(matches!(
        BlpFile::from_bytes(data),
        Err(BlpError::UnsupportedEncoding(9))
    ));
}

#[test]
fn error_truncated_at_format_version() {
    // Magic only (4 bytes); reading format version should fail.
    assert!(BlpFile::from_bytes(b"BLP2".to_vec()).is_err());
}

#[test]
fn error_truncated_mid_mip_table() {
    // Valid header, mip table cut short.
    let mut data = blp2_header(1, 0, 0, 1, 1);
    data.extend_from_slice(&[0u8; 10]); // far too short for the 128-byte table
    assert!(BlpFile::from_bytes(data).is_err());
}

#[test]
fn error_no_mipmaps_all_offsets_zero() {
    // All mip offsets = 0 → mipmap_count() = 0 → get_pixels returns NoMipmaps.
    let palette_bgra: [u8; 4] = [0u8; 4];
    let mut data = blp2_header(1, 0, 0, 1, 1);
    // All zeros for both tables and palette
    data.extend_from_slice(&[0u8; 128 + 1024]);

    let blp = BlpFile::from_bytes(data).unwrap();
    assert_eq!(blp.mipmap_count(), 0);
    assert!(matches!(blp.get_pixels(0), Err(BlpError::NoMipmaps)));
    let _ = palette_bgra;
}

// ─────────────────────────────────────────────────────────────────────────────
// Security / hardening tests
// ─────────────────────────────────────────────────────────────────────────────

/// Mip offset claims data starts past the end of the file.
#[test]
fn security_mip_offset_beyond_eof() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0x00]);
    let eof_plus = (data.len() as u32).wrapping_add(1024);
    data[offsets::MIP_OFFSET_0..offsets::MIP_OFFSET_0 + 4].copy_from_slice(&eof_plus.to_le_bytes());

    let blp = BlpFile::from_bytes(data).unwrap();
    assert!(matches!(blp.get_pixels(0), Err(BlpError::OutOfBounds)));
}

/// Mip size is u32::MAX — offset + size overflows before the slice check.
#[test]
fn security_mip_size_u32_max() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0x00]);
    data[offsets::MIP_SIZE_0..offsets::MIP_SIZE_0 + 4].copy_from_slice(&u32::MAX.to_le_bytes());

    let blp = BlpFile::from_bytes(data).unwrap();
    // checked_add catches the overflow; must not panic.
    assert!(matches!(blp.get_pixels(0), Err(BlpError::OutOfBounds)));
}

/// offset=u32::MAX-1, size=4: offset+size wraps around to a small number.
#[test]
fn security_mip_offset_plus_size_wraps() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0x00]);
    let off: u32 = u32::MAX - 1;
    let sz: u32 = 4;
    data[offsets::MIP_OFFSET_0..offsets::MIP_OFFSET_0 + 4].copy_from_slice(&off.to_le_bytes());
    data[offsets::MIP_SIZE_0..offsets::MIP_SIZE_0 + 4].copy_from_slice(&sz.to_le_bytes());

    let blp = BlpFile::from_bytes(data).unwrap();
    // checked_add returns None for usize overflow; must not panic.
    let result = blp.get_pixels(0);
    assert!(result.is_err());
}

/// Declared width×height is too large to fit in MAX_IMAGE_BYTES (256 MiB).
#[test]
fn security_image_too_large_dimensions() {
    // 32768×32768 × 4 = 4 GiB > 256 MiB ceiling.
    let data = make_blp2_palette(32768, 32768, 0, &[[0u8; 4]; 256], vec![0x00]);
    let blp = BlpFile::from_bytes(data).unwrap();
    assert!(matches!(blp.get_pixels(0), Err(BlpError::ImageTooLarge)));
}

/// Mip data is shorter than n_pixels (palette decode would index out of bounds).
#[test]
fn security_palette_mip_data_too_short_for_indices() {
    // 4×4 image = 16 pixels, but mip_data has only 4 bytes.
    let data = make_blp2_palette(4, 4, 0, &[[0u8; 4]; 256], vec![0u8; 4]);
    let blp = BlpFile::from_bytes(data).unwrap();
    assert!(matches!(blp.get_pixels(0), Err(BlpError::DataTooShort)));
}

/// Mip data has the right number of index bytes but is missing alpha data.
#[test]
fn security_palette_missing_alpha8_data() {
    // 2×2 = 4 pixels, alpha_size=8 → needs 4 index bytes + 4 alpha bytes = 8 total.
    // Provide only 4 bytes (just the indices).
    let data = make_blp2_palette(2, 2, 8, &[[0u8; 4]; 256], vec![0u8; 4]);
    let blp = BlpFile::from_bytes(data).unwrap();
    assert!(matches!(blp.get_pixels(0), Err(BlpError::DataTooShort)));
}

/// Same but for 1-bit alpha.
#[test]
fn security_palette_missing_alpha1_data() {
    // 8 pixels → 8 index bytes + 1 alpha byte = 9 total needed.
    // Provide only 8 bytes.
    let data = make_blp2_palette(8, 1, 1, &[[0u8; 4]; 256], vec![0u8; 8]);
    let blp = BlpFile::from_bytes(data).unwrap();
    assert!(matches!(blp.get_pixels(0), Err(BlpError::DataTooShort)));
}

/// Same but for 4-bit alpha.
#[test]
fn security_palette_missing_alpha4_data() {
    // 4 pixels → 4 index bytes + 2 alpha bytes = 6 total needed.
    // Provide only 4 bytes.
    let data = make_blp2_palette(4, 1, 4, &[[0u8; 4]; 256], vec![0u8; 4]);
    let blp = BlpFile::from_bytes(data).unwrap();
    assert!(matches!(blp.get_pixels(0), Err(BlpError::DataTooShort)));
}

/// DXT1 mip data is shorter than one full 8-byte block — must not panic.
#[test]
fn security_dxt_partial_block_no_panic() {
    // 4×4 DXT1 needs 8 bytes; provide only 4.
    let data = make_blp2_dxt(4, 4, 0, 0, vec![0u8; 4]);
    let blp = BlpFile::from_bytes(data).unwrap();
    // Should succeed (zeroed pixels) or fail gracefully — must not panic.
    let result = blp.get_pixels(0);
    if let Ok((pixels, w, h)) = result {
        // If it succeeds the pixel buffer must be correctly sized (zeroed).
        assert_eq!(pixels.len(), (w * h * 4) as usize);
    }
    // Panic = test failure, which is the point.
}

/// DXT5 mip data completely empty — must not panic.
#[test]
fn security_dxt5_empty_data_no_panic() {
    let data = make_blp2_dxt(4, 4, 8, 7 /*Dxt5*/, vec![]);
    let blp = BlpFile::from_bytes(data).unwrap();
    let _ = blp.get_pixels(0); // must not panic
}

/// DXT3 mip data completely empty — must not panic.
#[test]
fn security_dxt3_empty_data_no_panic() {
    let data = make_blp2_dxt(4, 4, 8, 1 /*Dxt3*/, vec![]);
    let blp = BlpFile::from_bytes(data).unwrap();
    let _ = blp.get_pixels(0);
}

/// JPEG encoding with a claimed header size larger than the remaining file.
/// The allocator guard (MAX_JPEG_HEADER) must reject it before any read.
#[test]
fn security_jpeg_header_size_too_large() {
    // Manually craft a BLP2 with JPEG encoding and a huge header size field.
    const DATA_OFFSET: u32 = 148 + 4; // header + tables + 4-byte size field
    let mut v = blp2_header(0 /*Jpeg*/, 0, 0, 1, 1);
    append_mip_tables(&mut v, DATA_OFFSET, 0);
    // JPEG header size = MAX_JPEG_HEADER + 1 (just over the limit)
    let bad_size: i32 = (64 * 1024 + 1) as i32;
    v.extend_from_slice(&bad_size.to_le_bytes());
    // No actual header bytes follow.

    assert!(BlpFile::from_bytes(v).is_err());
}

/// JPEG encoding with header size = i32::MAX — must not OOM or panic.
#[test]
fn security_jpeg_header_size_i32_max() {
    let mut v = blp2_header(0 /*Jpeg*/, 0, 0, 1, 1);
    append_mip_tables(&mut v, 200, 0);
    v.extend_from_slice(&i32::MAX.to_le_bytes()); // 2 GiB claimed header

    assert!(BlpFile::from_bytes(v).is_err());
}

/// All mip_sizes = u32::MAX with valid offsets — must not cause any panic.
#[test]
fn security_all_mip_sizes_u32_max() {
    let mut data = make_blp2_palette(1, 1, 0, &[[0u8; 4]; 256], vec![0x00]);
    for i in 0..16 {
        let off = offsets::MIP_SIZE_0 + i * 4;
        data[off..off + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    let blp = BlpFile::from_bytes(data).unwrap();
    // All levels should error, not panic.
    for level in 0..16u32 {
        let _ = blp.get_pixels(level);
    }
}

/// Zero-dimension image must not cause divide-by-zero or zero-size allocation issues.
#[test]
fn security_zero_width() {
    // width=0 → after scale and max(1) we get w=1, but the mip data should still
    // be checked.  The important thing is no panic during parsing.
    let data = make_blp2_palette(0, 1, 0, &[[0u8; 4]; 256], vec![0x00]);
    let blp = BlpFile::from_bytes(data).unwrap();
    // Should either succeed (1×1 due to max(1)) or return an error, but not panic.
    let _ = blp.get_pixels(0);
}

/// Mip data contains valid indices but the data slice ends exactly at n_pixels.
/// For alpha_size=0 this is correct; verify it succeeds (regression guard).
#[test]
fn security_palette_data_exactly_n_pixels_no_alpha() {
    let mut palette = [[0u8; 4]; 256];
    palette[0] = [0x00, 0xFF, 0x00, 0xFF]; // green
    let data = make_blp2_palette(2, 2, 0, &palette, vec![0u8; 4]);
    let blp = BlpFile::from_bytes(data).unwrap();
    let (pixels, _, _) = blp.get_pixels(0).unwrap();
    assert_eq!(pixels.len(), 16);
    for chunk in pixels.chunks_exact(4) {
        assert_eq!(chunk[3], 0xFF, "alpha must be opaque when alpha_size=0");
    }
}
