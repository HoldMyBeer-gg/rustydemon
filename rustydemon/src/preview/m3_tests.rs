//! Tests for the StarCraft II model reader, on files built here by hand.

use super::m3::*;

/// One chunk of a test file: tag, version, entry count, bytes.
type Chunk = (&'static str, u32, u32, Vec<u8>);

fn tag_bytes(tag: &str) -> [u8; 4] {
    let mut t: [u8; 4] = tag.as_bytes().try_into().unwrap();
    t.reverse();
    t
}

/// A reference: how many entries, and which chunk holds them.
fn rf(entries: u32, chunk: u32) -> Vec<u8> {
    let mut v = entries.to_le_bytes().to_vec();
    v.extend_from_slice(&chunk.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Lay the chunks out after the header and write the index table last.
/// Chunk numbers in references count the header as chunk 0, so the first
/// chunk passed in is chunk 1 and must be the model.
fn file(chunks: &[Chunk]) -> Vec<u8> {
    let mut out = vec![0u8; 24];
    let mut index = vec![("MD34", 0u32, 1u32, 11u32)];
    for (tag, version, count, bytes) in chunks {
        index.push((tag, out.len() as u32, *count, *version));
        out.extend_from_slice(bytes);
        out.resize(out.len().next_multiple_of(16), 0xAA);
    }
    let index_offset = out.len() as u32;
    for (tag, offset, count, version) in &index {
        out.extend_from_slice(&tag_bytes(tag));
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&version.to_le_bytes());
    }
    put(&mut out, 0, &tag_bytes("MD34"));
    put(&mut out, 4, &index_offset.to_le_bytes());
    put(&mut out, 8, &(index.len() as u32).to_le_bytes());
    put(&mut out, 12, &rf(1, 1));
    out
}

fn chars(s: &str) -> Chunk {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    ("CHAR", 0, b.len() as u32, b)
}

fn u16s(tag: &'static str, v: &[u16]) -> Chunk {
    let b = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    (tag, 0, v.len() as u32, b)
}

/// A vertex in the plainest layout: position, four weights, four lookup
/// slots, normal, one uv, tangent.  32 bytes.
fn vertex(pos: [f32; 3], weights: [u8; 4], slots: [u8; 4], uv: [i16; 2]) -> Vec<u8> {
    let mut v = Vec::new();
    for c in pos {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v.extend_from_slice(&weights);
    v.extend_from_slice(&slots);
    v.extend_from_slice(&[255, 127, 0, 0]); // normal (1, 0, -1) and sign
    for c in uv {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v.extend_from_slice(&[0; 4]); // tangent
    v
}

const PLAIN: u32 = 0x0182_007D;

/// A region of the given version.
fn region(
    version: u32,
    first_vertex: u32,
    vertices: u32,
    first_index: u32,
    indices: u32,
) -> Vec<u8> {
    let size = match version {
        2 => 28,
        3 => 36,
        4 => 40,
        _ => 48,
    };
    let mut r = vec![0u8; size];
    let rest = if version == 2 {
        put(&mut r, 4, &(first_vertex as u16).to_le_bytes());
        put(&mut r, 6, &(vertices as u16).to_le_bytes());
        8
    } else {
        put(&mut r, 8, &first_vertex.to_le_bytes());
        put(&mut r, 12, &vertices.to_le_bytes());
        16
    };
    put(&mut r, rest, &first_index.to_le_bytes());
    put(&mut r, rest + 4, &indices.to_le_bytes());
    r
}

/// Where a region keeps the first slot of its part of the bone lookup.
fn set_first_lookup(region: &mut [u8], version: u32, first: u16) {
    let at = if version == 2 { 18 } else { 26 };
    put(region, at, &first.to_le_bytes());
}

fn batch(region: u16, material_ref: u16) -> Vec<u8> {
    let mut b = vec![0u8; 14];
    put(&mut b, 4, &region.to_le_bytes());
    put(&mut b, 10, &material_ref.to_le_bytes());
    b
}

fn bone(name_chunk: u32, name_len: u32, parent: i16, at: [f32; 3]) -> Vec<u8> {
    let mut b = vec![0u8; 160];
    put(&mut b, 4, &rf(name_len, name_chunk));
    put(&mut b, 20, &parent.to_le_bytes());
    for (k, c) in at.iter().enumerate() {
        put(&mut b, 32 + 4 * k, &c.to_le_bytes());
    }
    put(&mut b, 68 + 12, &1f32.to_le_bytes()); // rotation w
    for k in 0..3 {
        put(&mut b, 112 + 4 * k, &1f32.to_le_bytes()); // scale
    }
    b
}

/// A model of two regions: a triangle of vertices 0 to 2, then a triangle
/// of vertices 3 to 5 whose indices start again at 0.
fn two_regions(region_version: u32) -> Vec<u8> {
    let mut vertices = Vec::new();
    for i in 0..6u8 {
        vertices.extend(vertex(
            [f32::from(i), 2.0, 3.0],
            [255, 0, 0, 0],
            [0, 0, 0, 0],
            [1024, 512],
        ));
    }
    let mut regions = region(region_version, 0, 3, 0, 3);
    let mut second = region(region_version, 3, 3, 3, 3);
    set_first_lookup(&mut second, region_version, 1);
    regions.extend(second);
    let mut batches = batch(0, 0);
    batches.extend(batch(1, 0));

    let mut modl = vec![0u8; 784];
    put(&mut modl, 0, &rf(5, 2));
    put(&mut modl, 80, &rf(2, 12));
    put(&mut modl, 96, &PLAIN.to_le_bytes());
    put(&mut modl, 100, &rf(vertices.len() as u32, 3));
    put(&mut modl, 112, &rf(1, 4));
    put(&mut modl, 124, &rf(2, 15));

    let mut div = vec![0u8; 52];
    put(&mut div, 0, &rf(6, 5));
    put(&mut div, 12, &rf(2, 6));
    put(&mut div, 24, &rf(2, 7));

    let mut matm = 1u32.to_le_bytes().to_vec();
    matm.extend_from_slice(&0u32.to_le_bytes());

    let mut mat = vec![0u8; 268];
    put(&mut mat, 0, &rf(5, 9 + 1));
    put(&mut mat, 52, &rf(1, 11));

    let mut layer = vec![0u8; 356];
    put(&mut layer, 4, &rf(24, 12 + 2));

    let mut bones = bone(13, 5, -1, [0.0, 0.0, 1.0]);
    bones.extend(bone(13, 5, 0, [0.0, 2.0, 0.0]));

    file(&[
        ("MODL", 23, 1, modl),                        // 1
        chars("Unit"),                                // 2
        ("U8__", 0, vertices.len() as u32, vertices), // 3
        ("DIV_", 2, 1, div),                          // 4
        u16s("U16_", &[0, 1, 2, 2, 1, 0]),            // 5
        ("REGN", region_version, 2, regions),         // 6
        ("BAT_", 1, 2, batches),                      // 7
        ("MATM", 0, 1, matm),                         // 8
        ("MAT_", 15, 1, mat),                         // 9
        chars("Skin"),                                // 10
        ("LAYR", 22, 1, layer),                       // 11
        ("BONE", 1, 2, bones),                        // 12
        chars("Root"),                                // 13
        chars("Assets\\Textures\\unit.dds"),          // 14
        u16s("U16_", &[0, 1]),                        // 15
    ])
}

#[test]
fn a_file_is_claimed_by_its_first_four_bytes() {
    assert!(claims("Unit.m3", &two_regions(3)));
    assert!(!claims("Unit.m3", b"MD20 and more bytes after that"));
    assert!(!claims("Unit.m3", b"43"));
}

#[test]
fn vertices_come_out_with_position_normal_and_uv() {
    let m = parse(&two_regions(3)).unwrap();
    assert_eq!(m.name, "Unit");
    assert_eq!(m.positions.len(), 6);
    assert_eq!(m.positions[4], [4.0, 2.0, 3.0]);
    // 1024 and 512 over 2048.
    assert_eq!(m.uvs[0], [0.5, 0.25]);
    let n = m.normals[0];
    assert!((n[0] - 1.0).abs() < 0.01 && n[1].abs() < 0.01 && (n[2] + 1.0).abs() < 0.01);
}

#[test]
fn triangle_indices_restart_in_every_region() {
    for version in [2, 3, 4, 5] {
        let m = parse(&two_regions(version)).unwrap();
        assert_eq!(m.indices, [0, 1, 2, 5, 4, 3], "region version {version}");
        assert_eq!(m.parts.len(), 2);
        assert_eq!((m.parts[1].first_index, m.parts[1].index_count), (3, 3));
    }
}

#[test]
fn a_part_names_its_colour_texture_through_the_material() {
    let m = parse(&two_regions(3)).unwrap();
    assert_eq!(m.materials.len(), 1);
    assert_eq!(m.materials[0].name, "Skin");
    assert_eq!(
        m.materials[0].diffuse.as_deref(),
        Some("Assets\\Textures\\unit.dds")
    );
    assert_eq!(m.parts[0].material, Some(0));
}

#[test]
fn bones_keep_their_parent_and_rest_pose() {
    let m = parse(&two_regions(3)).unwrap();
    assert_eq!(m.bones.len(), 2);
    assert_eq!(m.bones[0].name, "Root");
    assert_eq!(m.bones[0].parent, None);
    assert_eq!(m.bones[1].parent, Some(0));
    assert_eq!(m.bones[1].translation, [0.0, 2.0, 0.0]);
    assert_eq!(m.bones[0].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(m.bones[0].scale, [1.0, 1.0, 1.0]);
}

#[test]
fn a_vertex_is_tied_to_bones_through_its_regions_part_of_the_lookup() {
    let m = parse(&two_regions(3)).unwrap();
    // Slot 0 in the first region is bone 0; in the second region the
    // lookup starts one entry later, so slot 0 is bone 1.
    assert_eq!(m.skin[0][0], (0, 1.0));
    assert_eq!(m.skin[3][0], (1, 1.0));
    assert_eq!(m.skin[3][1].1, 0.0);
}

#[test]
fn a_colour_and_a_second_uv_widen_the_vertex() {
    // Colour adds 4 bytes after the normal, each further uv 4 more.
    let mut wide = Vec::new();
    for i in 0..6u8 {
        let v = vertex([f32::from(i), 0.0, 0.0], [255, 0, 0, 0], [0; 4], [2048, 0]);
        wide.extend_from_slice(&v[..24]);
        wide.extend_from_slice(&[9; 4]); // colour
        wide.extend_from_slice(&v[24..28]); // first uv
        wide.extend_from_slice(&[7; 4]); // second uv
        wide.extend_from_slice(&v[28..]);
    }
    let flags: u32 = PLAIN | 0x200 | 0x40000;
    let mut modl = vec![0u8; 784];
    put(&mut modl, 96, &flags.to_le_bytes());
    put(&mut modl, 100, &rf(wide.len() as u32, 3));
    put(&mut modl, 112, &rf(1, 4));
    let mut div = vec![0u8; 52];
    put(&mut div, 0, &rf(3, 5));
    put(&mut div, 12, &rf(1, 6));
    put(&mut div, 24, &rf(1, 7));
    let data = file(&[
        ("MODL", 23, 1, modl),
        chars("Unit"),
        ("U8__", 0, wide.len() as u32, wide),
        ("DIV_", 2, 1, div),
        u16s("U16_", &[0, 1, 2]),
        ("REGN", 3, 1, region(3, 0, 6, 0, 3)),
        ("BAT_", 1, 1, batch(0, 0)),
    ]);
    let m = parse(&data).unwrap();
    assert_eq!(m.positions.len(), 6);
    assert_eq!(m.positions[5], [5.0, 0.0, 0.0]);
    assert_eq!(m.uvs[5], [1.0, 0.0]);
    assert!(m.materials.is_empty());
    assert_eq!(m.parts[0].material, None);
}

#[test]
fn counts_that_point_past_the_file_are_errors() {
    let good = two_regions(3);
    // The index table offset.
    let mut bad = good.clone();
    put(&mut bad, 4, &0x7FFF_FFF0u32.to_le_bytes());
    assert!(parse(&bad).is_err());
    // The index table length.
    let mut bad = good.clone();
    put(&mut bad, 8, &0x00FF_FFFFu32.to_le_bytes());
    assert!(parse(&bad).is_err());
    // The vertex byte count in the model, at 24 + 100.
    let mut bad = good.clone();
    put(&mut bad, 24 + 100, &0x7FFF_FFFFu32.to_le_bytes());
    assert!(parse(&bad).is_err());
    // A cut file.
    assert!(parse(&good[..good.len() / 2]).is_err());
    assert!(parse(&[]).is_err());
}

#[test]
fn a_triangle_index_past_its_regions_vertices_is_an_error() {
    let mut data = two_regions(3);
    // The first u16 of the face chunk.  Find it by its six known values.
    let faces: Vec<u8> = [0u16, 1, 2, 2, 1, 0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let at = data
        .windows(faces.len())
        .position(|w| w == faces.as_slice())
        .unwrap();
    put(&mut data, at, &500u16.to_le_bytes());
    assert!(parse(&data).is_err());
}

#[test]
fn the_viewer_mesh_has_one_batch_per_part_and_the_models_bounds() {
    let m = parse(&two_regions(3)).unwrap();
    let mesh = to_mesh(&m);
    assert_eq!(mesh.positions.len(), 6);
    assert_eq!(mesh.indices, m.indices);
    assert_eq!(mesh.batches.len(), 2);
    assert_eq!(
        (mesh.batches[1].start_index, mesh.batches[1].index_count),
        (3, 3)
    );
    // No texture is decoded yet, so every batch falls back to flat colour.
    assert!(mesh.materials.is_empty());
    assert_eq!(mesh.batches[0].material_id, u32::MAX);
    assert_eq!(mesh.bbox_min, [0.0, 2.0, 3.0]);
    assert_eq!(mesh.bbox_max, [5.0, 2.0, 3.0]);
}

#[test]
fn the_summary_counts_the_model_and_lists_its_textures() {
    let text = summary(&parse(&two_regions(3)).unwrap());
    assert!(text.starts_with("StarCraft II model"), "{text}");
    for line in [
        "Vertices     6",
        "Triangles    2",
        "Parts        2",
        "Bones        2",
        "Skin: Assets\\Textures\\unit.dds",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}

#[test]
fn the_rigged_export_gives_each_part_its_own_vertices_and_skin() {
    let m = parse(&two_regions(3)).unwrap();
    let g = to_gltf(&m, "Unit.m3");
    assert!(g.z_up);
    assert_eq!(g.name, "Unit");
    assert_eq!(g.bones.len(), 2);
    assert_eq!(g.bones[1].name, "Root");
    assert_eq!(g.bones[1].parent, Some(0));
    assert_eq!(g.bones[1].translation, [0.0, 2.0, 0.0]);

    assert_eq!(g.parts.len(), 2);
    // The second part drew vertices 5, 4, 3; it keeps only those three,
    // numbered from zero in the order they are first used.
    let second = &g.parts[1];
    assert_eq!(second.indices, [0, 1, 2]);
    assert_eq!(second.positions.len(), 3);
    assert_eq!(second.positions[0], [5.0, 2.0, 3.0]);
    assert_eq!(second.uvs[0], [0.5, 0.25]);
    assert_eq!(second.skin[0][0], (1, 1.0));
    assert_eq!(second.material.as_deref(), Some("Skin"));
    assert_eq!(second.name, "Unit_1");
}

#[test]
fn a_model_without_bones_exports_unskinned() {
    let mut m = parse(&two_regions(3)).unwrap();
    m.bones.clear();
    let g = to_gltf(&m, "Unit.m3");
    assert!(g.bones.is_empty());
    assert!(g.parts.iter().all(|p| p.skin.is_empty()));
}

#[test]
fn a_weight_on_a_bone_that_is_not_there_is_dropped() {
    let mut m = parse(&two_regions(3)).unwrap();
    m.skin[0][1] = (99, 0.5);
    let g = to_gltf(&m, "Unit.m3");
    assert_eq!(g.parts[0].skin[0][1], (0, 0.0));
}
