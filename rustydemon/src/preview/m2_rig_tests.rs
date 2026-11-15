use super::m2_lite::tests::{chunked, rigged};
use super::m2_lite::M2Lite;
use super::m2_rig::model;
use super::{gltf, MeshBatch};

fn batch(start: u32, count: u32, material: u32) -> MeshBatch {
    MeshBatch {
        start_index: start,
        index_count: count,
        material_id: material,
        blend: Default::default(),
    }
}

fn m2() -> M2Lite {
    M2Lite::parse(&rigged()).unwrap()
}

#[test]
fn bones_hang_from_their_parents_by_pivot() {
    let m = model(&m2(), &[0, 1, 0], &[batch(0, 3, 0)], Vec::new());
    let identity = [0.0, 0.0, 0.0, 1.0];
    assert_eq!(
        m.bones,
        [
            gltf::Bone {
                name: String::new(),
                parent: None,
                rotation: identity,
                translation: [0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
            gltf::Bone {
                name: String::new(),
                parent: Some(0),
                rotation: identity,
                translation: [0.0, 0.0, 1.5],
                scale: [1.0; 3],
            },
            gltf::Bone {
                name: String::new(),
                parent: Some(1),
                rotation: identity,
                translation: [0.5, 0.0, 0.5],
                scale: [1.0; 3],
            },
        ]
    );
    assert!(m.z_up);
    assert!(m.animations.is_empty());
}

#[test]
fn vertices_carry_four_weights_scaled_to_one() {
    let m = model(&m2(), &[0, 1, 0], &[batch(0, 3, 1)], Vec::new());
    assert_eq!(m.parts.len(), 1);
    let p = &m.parts[0];
    assert_eq!(p.positions, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
    assert_eq!(p.uvs, [[0.25, 0.5], [0.75, 1.0]]);
    assert_eq!(p.skin[0], [(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]);
    assert_eq!(
        p.skin[1],
        [(1, 128.0 / 255.0), (2, 127.0 / 255.0), (0, 0.0), (0, 0.0)]
    );
    assert_eq!(p.indices, [0, 1, 0]);
}

#[test]
fn each_batch_is_its_own_part_with_its_own_vertices() {
    let textures = vec![
        ("texture_0".to_string(), b"png0".to_vec()),
        ("texture_1".to_string(), b"png1".to_vec()),
    ];
    let m = model(
        &m2(),
        &[0, 1, 1, 1, 0, 0],
        &[batch(0, 3, 0), batch(3, 3, 1)],
        textures.clone(),
    );
    assert_eq!(m.parts.len(), 2);
    assert_eq!(m.parts[0].name, "part_0");
    assert_eq!(m.parts[0].material.as_deref(), Some("texture_0"));
    assert_eq!(m.parts[0].positions, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
    assert_eq!(m.parts[0].indices, [0, 1, 1]);
    // The second batch meets vertex 1 first, so it comes first in that part.
    assert_eq!(m.parts[1].name, "part_1");
    assert_eq!(m.parts[1].material.as_deref(), Some("texture_1"));
    assert_eq!(m.parts[1].positions, [[4.0, 5.0, 6.0], [1.0, 2.0, 3.0]]);
    assert_eq!(m.parts[1].indices, [0, 1, 1]);
    assert_eq!(m.parts[1].skin[0][0], (1, 128.0 / 255.0));
    assert_eq!(m.textures, textures);
}

#[test]
fn a_batch_past_the_index_buffer_is_clipped() {
    let m = model(
        &m2(),
        &[0, 1, 0],
        &[batch(2, 10, 0), batch(5, 3, 0)],
        Vec::new(),
    );
    assert_eq!(m.parts.len(), 1);
    assert_eq!(m.parts[0].indices, [0]);
    assert_eq!(m.parts[0].positions, [[1.0, 2.0, 3.0]]);
}

#[test]
fn a_model_without_bones_exports_unskinned_parts() {
    let m2 = M2Lite::parse(&chunked()).unwrap();
    let m = model(&m2, &[0, 1, 0], &[batch(0, 3, 0)], Vec::new());
    assert!(m.bones.is_empty());
    assert!(m.parts[0].skin.is_empty());
}

#[test]
fn a_bone_index_past_the_skeleton_is_dropped() {
    // The second vertex names bone 2; cut the skeleton down to two bones.
    let mut m2 = m2();
    m2.bones.truncate(2);
    let m = model(&m2, &[0, 1, 0], &[batch(0, 3, 0)], Vec::new());
    assert_eq!(
        m.parts[0].skin[1],
        [(1, 128.0 / 255.0), (0, 0.0), (0, 0.0), (0, 0.0)]
    );
}

#[test]
fn writes_a_glb_through_the_shared_writer() {
    let glb = gltf::to_glb(&model(&m2(), &[0, 1, 0], &[batch(0, 3, 0)], Vec::new()));
    assert_eq!(&glb[..4], b"glTF");
}
