//! Tests for the DDS reader, on files built here by hand.

use super::dds::*;

const FOURCC: u32 = 0x4;
const RGB: u32 = 0x40;
const ALPHA: u32 = 0x1;

/// The 128 byte header: magic, sizes, then the pixel format.
fn header(
    width: u32,
    height: u32,
    flags: u32,
    fourcc: &[u8; 4],
    bits: u32,
    masks: [u32; 4],
) -> Vec<u8> {
    let mut h = vec![0u8; 128];
    h[..4].copy_from_slice(b"DDS ");
    h[4..8].copy_from_slice(&124u32.to_le_bytes());
    h[12..16].copy_from_slice(&height.to_le_bytes());
    h[16..20].copy_from_slice(&width.to_le_bytes());
    h[76..80].copy_from_slice(&32u32.to_le_bytes());
    h[80..84].copy_from_slice(&flags.to_le_bytes());
    h[84..88].copy_from_slice(fourcc);
    h[88..92].copy_from_slice(&bits.to_le_bytes());
    for (k, m) in masks.iter().enumerate() {
        h[92 + 4 * k..96 + 4 * k].copy_from_slice(&m.to_le_bytes());
    }
    h
}

fn compressed(width: u32, height: u32, fourcc: &[u8; 4], blocks: &[u8]) -> Vec<u8> {
    let mut f = header(width, height, FOURCC, fourcc, 0, [0; 4]);
    f.extend_from_slice(blocks);
    f
}

/// A colour block where every pixel is pure red.
const RED: [u8; 8] = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
/// A colour block where every pixel is pure blue.
const BLUE: [u8; 8] = [0x1F, 0x00, 0x1F, 0x00, 0, 0, 0, 0];

const BGRA_MASKS: [u32; 4] = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000];

#[test]
fn a_file_is_claimed_by_its_name_and_first_four_bytes() {
    let f = compressed(4, 4, b"DXT1", &RED);
    assert!(claims("Assets/Textures/unit_diffuse.DDS", &f));
    assert!(!claims("unit.dds", b"BLP2 and more"));
    assert!(!claims("unit.png", &f));
    // SC:R keeps something else under this double extension.
    assert!(!claims("unit.dds.grp", &f));
}

#[test]
fn a_dxt1_block_decodes_to_sixteen_pixels() {
    let img = decode(&compressed(4, 4, b"DXT1", &RED)).unwrap();
    assert_eq!((img.width, img.height), (4, 4));
    assert_eq!(img.format, "DXT1");
    assert_eq!(img.rgba.len(), 64);
    assert!(img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [255, 0, 0, 255]));
}

#[test]
fn a_dxt5_block_carries_its_own_alpha() {
    // Alpha endpoints 128 and 128, every pixel on the first.
    let mut block = vec![128, 128, 0, 0, 0, 0, 0, 0];
    block.extend_from_slice(&RED);
    let img = decode(&compressed(4, 4, b"DXT5", &block)).unwrap();
    assert_eq!(img.format, "DXT5");
    assert!(img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [255, 0, 0, 128]));
}

#[test]
fn a_dxt3_block_carries_four_bit_alpha() {
    let mut block = vec![0xFF; 8];
    block.extend_from_slice(&BLUE);
    let img = decode(&compressed(4, 4, b"DXT3", &block)).unwrap();
    assert!(img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [0, 0, 255, 255]));
}

#[test]
fn a_size_that_is_not_whole_blocks_is_cropped() {
    // 5 x 3 takes two blocks side by side: red, then blue.
    let mut blocks = RED.to_vec();
    blocks.extend_from_slice(&BLUE);
    let img = decode(&compressed(5, 3, b"DXT1", &blocks)).unwrap();
    assert_eq!((img.width, img.height), (5, 3));
    assert_eq!(img.rgba.len(), 5 * 3 * 4);
    // Row 0: four red pixels, then the first column of the blue block.
    assert_eq!(&img.rgba[12..16], [255, 0, 0, 255]);
    assert_eq!(&img.rgba[16..20], [0, 0, 255, 255]);
}

#[test]
fn plain_32_bit_pixels_follow_the_channel_masks() {
    let mut f = header(2, 1, RGB | ALPHA, &[0; 4], 32, BGRA_MASKS);
    f.extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 1, 2, 3, 4]);
    let img = decode(&f).unwrap();
    assert_eq!(img.rgba, [0x30, 0x20, 0x10, 0x40, 3, 2, 1, 4]);
    assert_eq!(img.format, "32 bit");
}

#[test]
fn pixels_without_an_alpha_channel_are_opaque() {
    let mut masks = BGRA_MASKS;
    masks[3] = 0;
    let mut f = header(1, 1, RGB, &[0; 4], 32, masks);
    f.extend_from_slice(&[0x10, 0x20, 0x30, 0x00]);
    assert_eq!(decode(&f).unwrap().rgba, [0x30, 0x20, 0x10, 255]);

    let mut f = header(1, 1, RGB, &[0; 4], 24, masks);
    f.extend_from_slice(&[0x10, 0x20, 0x30]);
    let img = decode(&f).unwrap();
    assert_eq!(img.rgba, [0x30, 0x20, 0x10, 255]);
    assert_eq!(img.format, "24 bit");
}

#[test]
fn a_file_too_short_for_its_size_is_an_error() {
    let f = compressed(8, 8, b"DXT1", &RED);
    let e = decode(&f).unwrap_err();
    assert!(e.contains("8x8"), "{e}");
    assert!(decode(&f[..100]).is_err());
    assert!(decode(&[]).is_err());
}

#[test]
fn an_absurd_or_empty_size_is_an_error_before_anything_is_allocated() {
    assert!(decode(&compressed(0x4000_0000, 0x4000_0000, b"DXT1", &RED)).is_err());
    assert!(decode(&compressed(0, 4, b"DXT1", &RED)).is_err());
    let mut f = header(0xFFFF_FFFF, 0xFFFF_FFFF, RGB, &[0; 4], 32, BGRA_MASKS);
    f.extend_from_slice(&[0; 16]);
    assert!(decode(&f).is_err());
}

#[test]
fn a_format_that_is_not_read_is_named_in_the_error() {
    let e = decode(&compressed(4, 4, b"ATI2", &[0; 16])).unwrap_err();
    assert!(e.contains("ATI2"), "{e}");
}
