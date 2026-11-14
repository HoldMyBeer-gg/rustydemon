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

#[test]
fn compressed_mesh_is_reported_as_such() {
    let mut f = static_triangle();
    f.flag = 2;
    assert!(matches!(
        Mesh::parse(&f.bytes()),
        Err(MessiahError::Compressed)
    ));
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
