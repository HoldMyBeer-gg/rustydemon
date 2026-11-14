use super::gltf::*;
use std::f32::consts::FRAC_1_SQRT_2;

fn json(glb: &[u8]) -> String {
    assert_eq!(&glb[..4], b"glTF");
    assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
        glb.len()
    );
    let n = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    assert_eq!(n % 4, 0);
    assert_eq!(&glb[20 + n + 4..20 + n + 8], b"BIN\0");
    String::from_utf8(glb[20..20 + n].to_vec()).unwrap()
}

fn bone(name: &str, parent: Option<usize>, t: [f32; 3]) -> Bone {
    Bone {
        name: name.into(),
        parent,
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation: t,
        scale: [1.0; 3],
    }
}

fn triangle(material: Option<&str>, skin: bool) -> Part {
    Part {
        name: "body".into(),
        material: material.map(str::to_owned),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 2.0]],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        skin: if skin {
            vec![
                [(0, 3.0), (1, 1.0), (0, 0.0), (0, 0.0)],
                [(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
                [(1, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)],
            ]
        } else {
            vec![]
        },
        indices: vec![0, 1, 2],
    }
}

#[test]
fn z_up_models_are_turned_y_up() {
    let mut m = Model {
        parts: vec![triangle(None, false)],
        z_up: true,
        ..Default::default()
    };
    assert!(json(&to_glb(&m)).contains("\"max\":[1,2,0]"));
    m.z_up = false;
    assert!(json(&to_glb(&m)).contains("\"max\":[1,0,2]"));
}

#[test]
fn skinned_parts_carry_four_joint_weights_normalised() {
    let m = Model {
        bones: vec![
            bone("Root", None, [0.0; 3]),
            bone("", Some(0), [0.0, 1.0, 0.0]),
        ],
        parts: vec![triangle(None, true)],
        ..Default::default()
    };
    let glb = to_glb(&m);
    let j = json(&glb);
    for want in [
        "\"skins\"",
        "\"inverseBindMatrices\"",
        "\"JOINTS_0\"",
        "\"WEIGHTS_0\"",
        "\"children\":[1]",
    ] {
        assert!(j.contains(want), "{want} missing: {j}");
    }
    // An unnamed bone gets a label from its index.
    assert!(j.contains("\"name\":\"bone_001\""), "{j}");
    // 3:1 becomes 0.75 / 0.25 in the weights buffer.
    let bin = &glb[20 + u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize + 8..];
    let has = |v: f32| bin.windows(4).any(|w| w == v.to_le_bytes());
    assert!(has(0.75) && has(0.25));
}

#[test]
fn textures_attach_by_material_name() {
    let m = Model {
        parts: vec![
            triangle(Some("skin"), false),
            triangle(Some("missing"), false),
        ],
        textures: vec![("skin".into(), vec![0x89, b'P', b'N', b'G'])],
        ..Default::default()
    };
    let j = json(&to_glb(&m));
    assert!(j.contains("\"mimeType\":\"image/png\""));
    assert!(j.contains("\"baseColorTexture\""));
    assert_eq!(j.matches("\"material\":0").count(), 1, "{j}");
}

#[test]
fn animation_channels_target_bones_by_index() {
    let m = Model {
        bones: vec![
            bone("Root", None, [0.0; 3]),
            bone("Arm", Some(0), [1.0, 0.0, 0.0]),
        ],
        animations: vec![Animation {
            name: "wave".into(),
            channels: vec![
                Channel {
                    bone: 1,
                    times: vec![0.0, 0.5],
                    values: Values::Rotation(vec![
                        [0.0, 0.0, 0.0, 1.0],
                        [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
                    ]),
                },
                Channel {
                    bone: 7,
                    times: vec![0.0],
                    values: Values::Scale(vec![[1.0; 3]]),
                },
            ],
        }],
        ..Default::default()
    };
    let j = json(&to_glb(&m));
    assert!(j.contains("\"name\":\"wave\""), "{j}");
    assert!(
        j.contains("\"target\":{\"node\":1,\"path\":\"rotation\"}"),
        "{j}"
    );
    // A channel for a bone the skeleton does not have is dropped.
    assert!(!j.contains("\"path\":\"scale\""), "{j}");
    assert!(j.contains("\"max\":[0.5]"), "{j}");
}
