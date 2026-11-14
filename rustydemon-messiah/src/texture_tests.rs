use super::texture::*;

fn mip(w: u16, h: u16, data: &[u8], packed: bool) -> Vec<u8> {
    let payload: Vec<u8> = if packed {
        let mut p = b"ZZZ4".to_vec();
        p.extend_from_slice(&(data.len() as u32).to_le_bytes());
        p.extend(lz4_flex::block::compress(data));
        p
    } else {
        let mut p = b"NNNN".to_vec();
        p.extend_from_slice(data);
        p
    };
    let mut m = Vec::new();
    m.extend_from_slice(&((16 + payload.len()) as u32).to_le_bytes());
    m.extend_from_slice(&w.to_le_bytes());
    m.extend_from_slice(&h.to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes());
    m.extend_from_slice(&(w * 4).to_le_bytes());
    m.extend_from_slice(&(data.len() as u32).to_le_bytes());
    m.extend(payload);
    m
}

fn texture(format: u8, mips: &[Vec<u8>], w: u16, h: u16) -> Vec<u8> {
    let mut b = vec![2, 2, 2, 1, 1, format, mips.len() as u8, 0, 0, 0, 0, 1];
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&h.to_le_bytes());
    b.extend([0u8; 16]);
    let body: Vec<u8> = mips.concat();
    b.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
    b.extend([1, format, mips.len() as u8, 0]);
    b.extend(body);
    b
}

#[test]
fn mips_parse_smallest_first_and_unpack() {
    let small = vec![7u8; 16];
    let big = vec![9u8; 64];
    let t = Texture::parse(&texture(
        0x19,
        &[mip(4, 4, &small, false), mip(8, 8, &big, true)],
        8,
        8,
    ))
    .unwrap();
    assert_eq!(t.format, 0x19);
    assert_eq!((t.width, t.height), (8, 8));
    assert_eq!(t.mips.len(), 2);
    let top = t.largest_mip().unwrap();
    assert_eq!((top.width, top.height), (8, 8));
    assert_eq!(top.data, big);
}

#[test]
fn magic_identifies_textures() {
    assert!(looks_like_texture(&texture(
        0x19,
        &[mip(4, 4, &[0; 16], false)],
        4,
        4
    )));
    assert!(!looks_like_texture(b".MESSIAH\x02\x00\x00\x00"));
}

#[test]
fn mip_with_a_wrong_size_is_rejected() {
    let mut m = mip(4, 4, &[0; 16], false);
    m[12] = 99;
    assert!(Texture::parse(&texture(0x19, &[m], 4, 4)).is_err());
}
