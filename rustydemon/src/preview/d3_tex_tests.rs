use super::d3_tex::*;

/// A D3 texture SNO: the SNO header, format, size, then one mip entry
/// whose offset counts from byte 16, then the pixel data.
fn texture(format: u32, w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 0x2c8 + 16];
    b[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    b[4..8].copy_from_slice(&0x2Fu32.to_le_bytes());
    for (o, v) in [(0x20, format), (0x24, w), (0x28, h), (0x2c, 1)] {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    b[0x38..0x3c].copy_from_slice(&0x2c8u32.to_le_bytes());
    b[0x3c..0x40].copy_from_slice(&(pixels.len() as u32).to_le_bytes());
    b.extend_from_slice(pixels);
    b
}

/// BC1 block of one solid colour, as its two planes: endpoints, indices.
fn bc1_planes(blocks: &[u16]) -> Vec<u8> {
    let mut ends = Vec::new();
    let mut idx = Vec::new();
    for &c in blocks {
        ends.extend_from_slice(&c.to_le_bytes());
        ends.extend_from_slice(&0u16.to_le_bytes());
        idx.extend_from_slice(&[0; 4]);
    }
    [ends, idx].concat()
}

#[test]
fn planes_are_woven_back_into_blocks() {
    // Two blocks of fields [a a | b b b] stored plane by plane.
    let stored = [1, 1, 2, 2, 10, 10, 10, 20, 20, 20];
    assert_eq!(
        unplane(&stored, &[2, 3]),
        vec![1, 1, 10, 10, 10, 2, 2, 20, 20, 20]
    );
}

#[test]
fn bc1_texture_decodes_from_planes() {
    // 8x4: a red block then a blue block (RGB565).
    let data = texture(9, 8, 4, &bc1_planes(&[0xF800, 0x001F]));
    assert!(looks_like_d3_texture(&data));
    let img = decode(&data).unwrap();
    assert_eq!((img.width, img.height), (8, 4));
    assert_eq!(&img.rgba[..4], &[255, 0, 0, 255]);
    assert_eq!(&img.rgba[4 * 4..4 * 4 + 4], &[0, 0, 255, 255]);
    assert_eq!(img.format, "BC1");
}

#[test]
fn bc3_texture_decodes_from_four_planes() {
    // One block: alpha endpoints 128/128, green colour.
    let mut px = vec![128, 128];
    px.extend_from_slice(&[0; 6]);
    px.extend_from_slice(&0x07E0u16.to_le_bytes());
    px.extend_from_slice(&0u16.to_le_bytes());
    px.extend_from_slice(&[0; 4]);
    let img = decode(&texture(12, 4, 4, &px)).unwrap();
    assert_eq!(&img.rgba[..4], &[0, 255, 0, 128]);
    assert_eq!(img.format, "BC3");
}

#[test]
fn format_11_is_bc2_in_three_planes() {
    // One block: explicit 4-bit alpha 0x8 everywhere, red colour.
    let mut px = vec![0x88; 8];
    px.extend_from_slice(&0xF800u16.to_le_bytes());
    px.extend_from_slice(&0u16.to_le_bytes());
    px.extend_from_slice(&[0; 4]);
    let img = decode(&texture(11, 4, 4, &px)).unwrap();
    assert_eq!(&img.rgba[..4], &[255, 0, 0, 0x88]);
    assert_eq!(img.format, "BC2");
}

#[test]
fn raw_texture_is_bgra() {
    let img = decode(&texture(0, 1, 1, &[1, 2, 3, 4])).unwrap();
    assert_eq!(img.rgba, vec![3, 2, 1, 4]);
}

#[test]
fn sizes_off_the_block_grid_are_cropped() {
    // 6x2 still stores two 4x4 blocks.
    let img = decode(&texture(9, 6, 2, &bc1_planes(&[0xF800, 0xF800]))).unwrap();
    assert_eq!((img.width, img.height), (6, 2));
    assert_eq!(img.rgba.len(), 6 * 2 * 4);
}

#[test]
fn other_sno_files_and_d4_textures_are_not_claimed() {
    let mut actor = texture(9, 4, 4, &bc1_planes(&[0]));
    actor[4..8].copy_from_slice(&0x11Au32.to_le_bytes());
    assert!(!looks_like_d3_texture(&actor));
    assert!(!looks_like_d3_texture(b"\xEF\xBE\xAD\xDE"));
}

#[test]
fn unknown_format_is_an_error_not_a_guess() {
    let err = decode(&texture(4, 4, 4, &[0; 32])).unwrap_err();
    assert!(err.contains("format 4"), "{err}");
}

#[test]
fn mip_past_the_end_is_an_error() {
    let mut data = texture(9, 4, 4, &bc1_planes(&[0]));
    data.truncate(data.len() - 1);
    assert!(decode(&data).is_err());
}
