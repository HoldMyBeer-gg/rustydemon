use super::*;

/// Build a mesh object the way the game lays it out.
struct Fixture {
    flag: u16,
    formats: [&'static str; 4],
    /// (index start, index count, vertex start, vertex count)
    submeshes: Vec<[u32; 4]>,
    indices: Vec<u16>,
    vertex_count: u32,
    /// One stream's bytes per entry in `formats` that is not "None".
    streams: Vec<Vec<u8>>,
}

impl Fixture {
    fn bytes(&self) -> Vec<u8> {
        let mut b = b".MESSIAH".to_vec();
        b.extend_from_slice(&8u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&self.flag.to_le_bytes());
        b.extend_from_slice(&(self.submeshes.len() as u16).to_le_bytes());
        b.extend_from_slice(&self.vertex_count.to_le_bytes());
        b.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        for f in self.formats {
            b.extend_from_slice(&(f.len() as u16).to_le_bytes());
            b.extend_from_slice(f.as_bytes());
        }
        for v in [-1.0f32, -2.0, -3.0, 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 3.7] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for s in &self.submeshes {
            for v in s {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for i in &self.indices {
            b.extend_from_slice(&i.to_le_bytes());
        }
        for s in &self.streams {
            b.extend_from_slice(s);
        }
        b.extend(std::iter::repeat_n(0u8, 52 * self.submeshes.len()));
        b
    }
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn half(x: f32) -> [u8; 2] {
    // Exact for the small values used here.
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    if x == 0.0 {
        return sign.to_le_bytes();
    }
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = ((bits >> 13) & 0x3ff) as u16;
    (sign | ((exp as u16) << 10) | mant).to_le_bytes()
}

/// A triangle in the single interleaved layout static props use.
fn static_triangle() -> Fixture {
    let mut stream = Vec::new();
    for (p, uv) in [
        ([0.0, 0.0, 0.0], [0.0, 0.0]),
        ([1.0, 0.0, 0.0], [1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 1.0]),
    ] {
        stream.extend(f32s(&p));
        stream.extend([0x80, 0x80, 0xff, 0x00]); // N4B
        stream.extend(f32s(&uv));
    }
    Fixture {
        flag: 0,
        formats: ["P3F_N4B_T2F", "None", "None", "None"],
        submeshes: vec![[0, 3, 0, 3]],
        indices: vec![0, 1, 2],
        vertex_count: 3,
        streams: vec![stream],
    }
}

#[test]
fn static_mesh_parses() {
    let mesh = Mesh::parse(&static_triangle().bytes()).unwrap();
    assert_eq!(
        mesh.positions,
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
    );
    assert_eq!(mesh.uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
    assert_eq!(mesh.indices, vec![0, 1, 2]);
    assert_eq!(mesh.bbox_min, [-1.0, -2.0, -3.0]);
    assert_eq!(mesh.bbox_max, [1.0, 2.0, 3.0]);
    assert_eq!(
        mesh.submeshes,
        vec![Submesh {
            start_index: 0,
            index_count: 3,
            start_vertex: 0,
            vertex_count: 3
        }]
    );
    assert_eq!(mesh.formats, vec!["P3F_N4B_T2F".to_owned()]);
}

/// Skinned meshes split position, tangent frame + UVs, and bone weights
/// into separate streams, with UVs stored as halves.
#[test]
fn skinned_mesh_reads_uvs_from_the_second_stream() {
    let positions = f32s(&[0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0]);
    let mut q_t = Vec::new();
    for uv in [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
        q_t.extend([0u8; 8]); // Q4H
        q_t.extend(half(uv[0]));
        q_t.extend(half(uv[1]));
    }
    let weights = vec![0u8; 8 * 4];
    let f = Fixture {
        flag: 0,
        formats: ["P3F", "Q4H_T2H", "W4B_I4B", "None"],
        submeshes: vec![[0, 3, 0, 3], [3, 3, 1, 3]],
        indices: vec![0, 1, 2, 1, 3, 2],
        vertex_count: 4,
        streams: vec![positions, q_t, weights],
    };
    let mesh = Mesh::parse(&f.bytes()).unwrap();
    assert_eq!(mesh.positions[3], [2.0, 2.0, 0.0]);
    assert_eq!(
        mesh.uvs,
        vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
    );
    assert_eq!(mesh.indices, vec![0, 1, 2, 1, 3, 2]);
    assert_eq!(mesh.submeshes.len(), 2);
    assert_eq!(mesh.submeshes[1].start_index, 3);
}

/// Pack values least significant bit first, `bits` each.
fn pack(values: &[u32], bits: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u64, 0u32);
    for &v in values {
        acc |= u64::from(v) << n;
        n += u32::from(bits);
        while n >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            n -= 8;
        }
    }
    if n > 0 {
        out.push(acc as u8);
    }
    out
}

/// One quantised element: count, range and minimum (float types), bits, data.
fn element(values: &[u32], bits: u8, range_min: Option<(f32, f32)>) -> Vec<u8> {
    let mut b = (values.len() as u32).to_le_bytes().to_vec();
    if let Some((range, min)) = range_min {
        b.extend(range.to_le_bytes());
        b.extend(min.to_le_bytes());
    }
    b.push(bits);
    b.extend(pack(values, bits));
    b
}

/// A quad stored with flag 2, the way large character meshes are.
fn compressed_quad() -> Vec<u8> {
    let mut b = b".MESSIAH".to_vec();
    b.extend(8u32.to_le_bytes());
    b.extend(0u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(4u32.to_le_bytes());
    b.extend(6u32.to_le_bytes());
    for f in ["P3F_N4B_T2F", "None", "None", "None"] {
        b.extend((f.len() as u16).to_le_bytes());
        b.extend(f.as_bytes());
    }
    for v in [-2.0f32, -2.0, 0.0, 2.0, 2.0, 0.0, 0.0, 0.0, 0.0, 2.9] {
        b.extend(v.to_le_bytes());
    }
    for v in [0u32, 6, 0, 4] {
        b.extend(v.to_le_bytes());
    }
    // Indices: count, bits, packed.
    b.extend(element(&[0, 2, 1, 2, 0, 3], 2, None));
    // Positions span -2..2 on x and y: min -2, range 4, 16 bits.
    let corners = [[0, 0, 0], [65535, 0, 0], [65535, 65535, 0], [0, 65535, 0]];
    let flat: Vec<u32> = corners.iter().flatten().copied().collect();
    b.extend(element(&flat, 16, Some((4.0, -2.0))));
    b.extend(element(&[127, 127, 255, 0].repeat(4), 8, None));
    // UVs 0..1 at 11 bits.
    let uv = [0, 0, 2047, 0, 2047, 2047, 0, 2047];
    b.extend(element(&uv, 11, Some((1.0, 0.0))));
    b.extend([0u8; 52]);
    b
}

#[test]
fn compressed_mesh_decodes() {
    let mesh = Mesh::parse(&compressed_quad()).unwrap();
    assert_eq!(mesh.indices, vec![0, 2, 1, 2, 0, 3]);
    assert_eq!(mesh.positions[0], [-2.0, -2.0, -2.0]);
    assert_eq!(mesh.positions[2], [2.0, 2.0, -2.0]);
    assert_eq!(mesh.uvs[2], [1.0, 1.0]);
    assert_eq!(mesh.formats, vec!["P3F_N4B_T2F".to_owned()]);
}

#[test]
fn truncated_compressed_mesh_is_rejected() {
    let b = compressed_quad();
    assert!(Mesh::parse(&b[..b.len() - 60]).is_err());
}

#[test]
fn index_past_the_vertex_count_is_rejected() {
    let mut f = static_triangle();
    f.indices = vec![0, 1, 9];
    assert!(Mesh::parse(&f.bytes()).is_err());
}

#[test]
fn truncated_mesh_is_rejected() {
    let b = static_triangle().bytes();
    assert!(Mesh::parse(&b[..b.len() - 60]).is_err());
}

#[test]
fn unknown_vertex_element_is_rejected() {
    let mut f = static_triangle();
    f.formats[0] = "P3F_Z9Q";
    assert!(Mesh::parse(&f.bytes()).is_err());
}

#[test]
fn objects_without_vertex_layouts_are_not_meshes() {
    let mut b = b".MESSIAH".to_vec();
    b.extend_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(b"\x00\x00\x01\x16\x00danshou");
    assert!(!looks_like_mesh(&b));
    assert!(looks_like_mesh(&static_triangle().bytes()));
}
