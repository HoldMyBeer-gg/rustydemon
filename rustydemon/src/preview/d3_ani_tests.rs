use super::d3_ani::*;
use super::d3_app::{to_glb, Appearance, Bone};

/// (bone name, translation keys, rotation keys as i16 x y z w)
type Keys<'a> = (&'a str, Vec<(u32, [f32; 3])>, Vec<(u32, [i16; 4])>);

/// Builds an Anim SNO: header fields at fixed offsets, regions appended,
/// offsets counted from byte 16.
fn anim(model: u32, bones: &[&str], keys: &[Keys]) -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    b[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    b[4..8].copy_from_slice(&0x76u32.to_le_bytes());
    let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut b, 0x24, model);
    let region = |b: &mut Vec<u8>, bytes: &[u8]| -> u32 {
        while !b.len().is_multiple_of(8) {
            b.push(0);
        }
        let at = b.len();
        b.extend_from_slice(bytes);
        (at - 16) as u32
    };
    let mut names = Vec::new();
    for n in bones {
        let mut v = n.as_bytes().to_vec();
        v.resize(64, 0);
        names.extend(v);
    }
    let no = region(&mut b, &names);
    put(&mut b, 0xc0, bones.len() as u32);
    put(&mut b, 0xc4, no);
    put(&mut b, 0xc8, names.len() as u32);

    let mut last = 0;
    let mut tables: [Vec<u8>; 3] = Default::default();
    for name in bones {
        let k = keys.iter().find(|k| k.0 == *name);
        let mut t = Vec::new();
        let mut r = Vec::new();
        let mut s = Vec::new();
        if let Some((_, tk, rk)) = k {
            for (f, v) in tk {
                last = last.max(*f);
                t.extend_from_slice(&f.to_le_bytes());
                for c in v {
                    t.extend_from_slice(&c.to_le_bytes());
                }
            }
            for (f, q) in rk {
                last = last.max(*f);
                r.extend_from_slice(&f.to_le_bytes());
                for c in q {
                    r.extend_from_slice(&c.to_le_bytes());
                }
            }
            s.extend_from_slice(&0u32.to_le_bytes());
            s.extend_from_slice(&1.0f32.to_le_bytes());
        }
        for (i, (data, each)) in [(t, 16), (r, 12), (s, 8)].into_iter().enumerate() {
            let off = if data.is_empty() {
                0
            } else {
                region(&mut b, &data)
            };
            let mut e = vec![0u8; 24];
            e[0..4].copy_from_slice(&((data.len() / each) as u32).to_le_bytes());
            e[4..8].copy_from_slice(&off.to_le_bytes());
            e[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());
            tables[i].extend(e);
        }
    }
    for (i, hdr) in [0xd8usize, 0xec, 0xfc].into_iter().enumerate() {
        let t = tables[i].clone();
        let off = region(&mut b, &t);
        put(&mut b, hdr, bones.len() as u32);
        put(&mut b, hdr + 4, off);
        put(&mut b, hdr + 8, t.len() as u32);
    }
    let half = (last + 1) as f32 / 2.0;
    b[0xa0..0xa4].copy_from_slice(&half.to_le_bytes());
    b
}

fn sample() -> Vec<u8> {
    anim(
        16482,
        &["Pelvis", "Tail"],
        &[(
            "Pelvis",
            vec![(0, [0.0, 0.0, 1.0]), (59, [0.0, 0.0, 2.0])],
            vec![(0, [0, 0, 0, 32767]), (59, [0, 0, 23170, 23170])],
        )],
    )
}

#[test]
fn header_names_the_model_and_the_length() {
    let a = parse(&sample()).unwrap();
    assert_eq!(a.model_sno, 16482);
    assert_eq!(a.key_frames, 60);
    assert!((a.seconds() - 1.0).abs() < 1e-6);
}

#[test]
fn tracks_decode_per_bone() {
    let a = parse(&sample()).unwrap();
    assert_eq!(a.bones.len(), 2);
    let p = &a.bones[0];
    assert_eq!(p.name, "Pelvis");
    assert_eq!(
        p.translation,
        vec![(0, [0.0, 0.0, 1.0]), (59, [0.0, 0.0, 2.0])]
    );
    assert_eq!(p.rotation[0], (0, [0.0, 0.0, 0.0, 1.0]));
    let (f, q) = p.rotation[1];
    assert_eq!(f, 59);
    // i16 / 32767, renormalised: a 90 degree turn about Z.
    assert!((q[2] - 0.70710677).abs() < 1e-4 && (q[3] - 0.70710677).abs() < 1e-4);
    assert_eq!(p.scale, vec![(0, 1.0)]);
    assert!(a.bones[1].translation.is_empty());
}

#[test]
fn only_ani_files_are_claimed() {
    let data = sample();
    assert!(claims("Base/Anim/treasureGoblin_Run_01.ani", &data));
    assert!(!claims("Base/Appearance/treasureGoblin_A.app", &data));
    assert!(!claims("Base/Anim/x.ani", b"nope"));
}

#[test]
fn truncated_animation_is_an_error() {
    let data = sample();
    assert!(parse(&data[..data.len() - 30]).is_err());
}

#[test]
fn glb_carries_the_animation_on_bones_matched_by_name() {
    let app = Appearance {
        bones: vec![
            Bone {
                name: "Root".into(),
                parent: -1,
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.0; 3],
                scale: 1.0,
            },
            Bone {
                name: "Pelvis".into(),
                parent: 0,
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0.0, 0.0, 1.0],
                scale: 1.0,
            },
        ],
        ..Default::default()
    };
    let a = parse(&sample()).unwrap();
    let glb = to_glb(&app, &[], Some(&a));
    let n = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let json = std::str::from_utf8(&glb[20..20 + n]).unwrap();
    assert!(json.contains("\"animations\""), "{json}");
    // Pelvis is node 1 in the model; Tail is not in the model and is skipped.
    assert!(
        json.contains("\"target\":{\"node\":1,\"path\":\"translation\"}"),
        "{json}"
    );
    assert!(
        json.contains("\"target\":{\"node\":1,\"path\":\"rotation\"}"),
        "{json}"
    );
    assert!(
        json.contains("\"target\":{\"node\":1,\"path\":\"scale\"}"),
        "{json}"
    );
    assert!(!json.contains("\"node\":0,\"path\""), "{json}");
    // Key 59 at 60 keys a second ends just under one second.
    assert!(json.contains("\"max\":[0.98333"), "{json}");
}
