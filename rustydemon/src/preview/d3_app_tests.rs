use super::d3_app::*;

const HEADER: usize = 0x240;

/// Builds an Appearance SNO the way the game lays it out: fixed header
/// fields pointing (from byte 16) at regions appended after it.
struct Builder {
    b: Vec<u8>,
}

impl Builder {
    fn new() -> Self {
        let mut b = vec![0u8; HEADER];
        b[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        b[4..8].copy_from_slice(&0x104u32.to_le_bytes());
        Builder { b }
    }
    fn put(&mut self, at: usize, v: u32) {
        self.b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    /// Append a region, returning its offset from byte 16.
    fn region(&mut self, bytes: &[u8]) -> u32 {
        while !self.b.len().is_multiple_of(8) {
            self.b.push(0);
        }
        let at = self.b.len();
        self.b.extend_from_slice(bytes);
        (at - 16) as u32
    }
    fn table(&mut self, at: usize, count: usize, bytes: &[u8]) {
        let off = self.region(bytes);
        self.put(at, count as u32);
        self.put(at + 4, off);
        self.put(at + 8, bytes.len() as u32);
    }
}

fn name_field(name: &str, len: usize) -> Vec<u8> {
    let mut v = name.as_bytes().to_vec();
    v.resize(len, 0);
    v
}

fn bone(name: &str, parent: i32, t: [f32; 3]) -> Vec<u8> {
    let mut b = name_field(name, 64);
    b.extend_from_slice(&parent.to_le_bytes());
    b.resize(172, 0);
    for v in [0.0f32, 0.0, 0.0, 1.0, t[0], t[1], t[2], 1.0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.resize(312, 0);
    b
}

/// A bone whose record also holds the inverse of its bind pose at 0xEC:
/// quaternion x y z w, translation, scale.
fn bound_bone(name: &str, parent: i32, t: [f32; 3], inverse: [f32; 8]) -> Vec<u8> {
    let mut b = bone(name, parent, t);
    for (i, v) in inverse.iter().enumerate() {
        b[0xEC + i * 4..0xEC + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    b
}

/// The goblin, with a bind pose that is not its rest pose: the bag rests
/// at x = 0.5 under the pelvis but was skinned a quarter turn about Z
/// away, at half size.
fn bound_goblin() -> Vec<u8> {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let mut b = Builder::new();
    let bones = [
        bound_bone(
            "Pelvis",
            -1,
            [0.0, 0.0, 2.0],
            [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -2.0, 1.0],
        ),
        bound_bone(
            "Bag",
            0,
            [0.5, 0.0, 0.0],
            [0.0, 0.0, half, half, 7.0, 8.0, 9.0, 0.5],
        ),
    ]
    .concat();
    b.table(0x24, 2, &bones);
    let parts = part(&mut b, "A_normal_mat");
    b.table(0xa8, 1, &parts);
    b.b
}

fn close(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> bool {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .all(|(x, y)| (x - y).abs() < 1e-6)
}

/// One triangle part. Vertices at (0,0,0), (1,0,0), (0,0,2); UVs at the
/// corners of the 0x8000..0x8200 range; first vertex split 0.75/0.25.
fn part(b: &mut Builder, material: &str) -> Vec<u8> {
    let mut verts = Vec::new();
    for (p, uv) in [
        ([0.0f32, 0.0, 0.0], [0x8000u16, 0x8000]),
        ([1.0, 0.0, 0.0], [0x8200, 0x8000]),
        ([0.0, 0.0, 2.0], [0x8000, 0x8200]),
    ] {
        let mut v = Vec::new();
        for c in p {
            v.extend_from_slice(&c.to_le_bytes());
        }
        v.resize(24, 0);
        v.extend_from_slice(&uv[0].to_le_bytes());
        v.extend_from_slice(&uv[1].to_le_bytes());
        v.resize(44, 0);
        verts.extend(v);
    }
    let mut weights = Vec::new();
    for (i, pairs) in [
        [(0u32, 0.75f32), (1, 0.25), (0, 0.0)],
        [(1, 1.0), (0, 0.0), (0, 0.0)],
        [(1, 1.0), (0, 0.0), (0, 0.0)],
    ]
    .iter()
    .enumerate()
    {
        let _ = i;
        for (bone, w) in pairs {
            weights.extend_from_slice(&bone.to_le_bytes());
            weights.extend_from_slice(&w.to_le_bytes());
        }
    }
    let idx: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
    let (vo, wo, io) = (b.region(&verts), b.region(&weights), b.region(&idx));
    let mut r = vec![0u8; 400];
    let mut put = |at: usize, v: u32| r[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(4, 3);
    put(8, vo);
    put(0xc, verts.len() as u32);
    put(0x14, 3);
    put(0x18, wo);
    put(0x1c, weights.len() as u32);
    put(0x28, 3);
    put(0x2c, io);
    put(0x30, idx.len() as u32);
    r[0x5c..0x5c + 64].copy_from_slice(&name_field(material, 64));
    r
}

/// A material whose bindings list `slots` as (slot, SNO), 160 bytes each.
fn material(b: &mut Builder, name: &str, slots: &[(u32, u32)]) -> Vec<u8> {
    let mut bind = Vec::new();
    for (slot, sno) in slots {
        let mut e = vec![0u8; 160];
        e[8..12].copy_from_slice(&slot.to_le_bytes());
        e[0x10..0x14].copy_from_slice(&sno.to_le_bytes());
        bind.extend(e);
    }
    let bo = b.region(&bind);
    let mut blk = vec![0u8; 0x4d8];
    blk[0..4].copy_from_slice(&4u32.to_le_bytes());
    blk[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    blk[8..12].copy_from_slice(&bo.to_le_bytes());
    blk[12..16].copy_from_slice(&(slots.len() as u32).to_le_bytes());
    let blo = b.region(&blk);
    let mut e = name_field(name, 128);
    e.extend_from_slice(&blo.to_le_bytes());
    e.extend_from_slice(&(blk.len() as u32).to_le_bytes());
    e.resize(144, 0);
    e
}

fn goblin() -> Vec<u8> {
    let mut b = Builder::new();
    let bones = [
        bone("Pelvis", -1, [0.0, 0.0, 2.0]),
        bone("Bag", 0, [0.5, 0.0, 0.0]),
    ]
    .concat();
    b.table(0x24, 2, &bones);
    let parts = [
        part(&mut b, "A_bagOnFloor_mat"),
        part(&mut b, "A_normal_mat"),
    ]
    .concat();
    b.table(0xa8, 2, &parts);
    let mats = [
        material(&mut b, "A_normal_mat", &[(1, 49462), (6, 48623)]),
        material(&mut b, "A_bagOnFloor_mat", &[(6, 48623), (1, 777)]),
    ]
    .concat();
    b.table(0x1b4, 2, &mats);
    let looks = [name_field("A", 64), name_field("bagOnFloor", 64)].concat();
    let lo = b.region(&looks);
    b.put(0x1c8, lo);
    b.put(0x1cc, looks.len() as u32);
    b.b
}

#[test]
fn bones_parts_and_looks_parse() {
    let app = parse(&goblin()).unwrap();
    assert_eq!(app.bones.len(), 2);
    assert_eq!(app.bones[1].name, "Bag");
    assert_eq!(app.bones[1].parent, 0);
    assert_eq!(app.bones[1].translation, [0.5, 0.0, 0.0]);
    assert_eq!(app.bones[0].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(app.looks, vec!["A", "bagOnFloor"]);
    let p = &app.parts[1];
    assert_eq!(p.material, "A_normal_mat");
    assert_eq!(p.positions[2], [0.0, 0.0, 2.0]);
    assert_eq!(p.uvs[1], [1.0, 0.0]);
    assert_eq!(p.uvs[2], [0.0, 1.0]);
    assert_eq!(p.indices, vec![0, 1, 2]);
    assert_eq!(p.weights[0], [(0, 0.75), (1, 0.25), (0, 0.0)]);
}

#[test]
fn a_material_names_its_colour_map_first() {
    let app = parse(&goblin()).unwrap();
    let m = app.material("A_normal_mat").unwrap();
    assert_eq!(m.colour_maps[0], 49462);
    // Slot 1 is found wherever it sits in the bindings.
    assert_eq!(
        app.material("A_bagOnFloor_mat").unwrap().colour_maps[0],
        777
    );
}

#[test]
fn the_normal_part_is_drawn_and_alternatives_are_not() {
    let app = parse(&goblin()).unwrap();
    assert_eq!(drawn_parts(&app), vec![1]);
}

#[test]
fn every_part_is_drawn_when_none_is_named_normal() {
    let mut app = parse(&goblin()).unwrap();
    for p in &mut app.parts {
        p.material = "Pedestal_Shader".into();
    }
    assert_eq!(drawn_parts(&app), vec![0, 1]);
}

#[test]
fn only_app_files_are_claimed() {
    let data = goblin();
    assert!(claims("Base/Appearance/treasureGoblin_A.app", &data));
    assert!(!claims("Base/Actor/treasureGoblin_A.acr", &data));
    assert!(!claims("Base/Appearance/x.app", b"nope"));
}

#[test]
fn truncated_appearance_is_an_error() {
    let data = goblin();
    assert!(parse(&data[..data.len() / 2]).is_err());
}

#[test]
fn glb_holds_skin_meshes_and_textures() {
    let app = parse(&goblin()).unwrap();
    let png = vec![0x89, b'P', b'N', b'G'];
    let glb = to_glb(&app, &[("A_normal_mat".to_owned(), png)], None, &[0, 1]);
    assert_eq!(&glb[..4], b"glTF");
    assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
        glb.len()
    );
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    assert_eq!(json_len % 4, 0);
    let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
    for want in [
        "\"skins\"",
        "\"inverseBindMatrices\"",
        "\"JOINTS_0\"",
        "\"WEIGHTS_0\"",
        "\"name\":\"Bag\"",
        "\"name\":\"A_normal_mat\"",
        "\"name\":\"A_bagOnFloor_mat\"",
        "\"mimeType\":\"image/png\"",
        "\"baseColorTexture\"",
    ] {
        assert!(json.contains(want), "{want} missing from {json}");
    }
    // Z-up (0,0,2) becomes Y-up (0,2,0): the bounds carry it.
    assert!(json.contains("\"max\":[1,2,0]"), "{json}");
    let bin_at = 20 + json_len;
    assert_eq!(&glb[bin_at + 4..bin_at + 8], b"BIN\0");
}

#[test]
fn a_bone_carries_the_inverse_of_its_bind_pose() {
    let app = parse(&bound_goblin()).unwrap();
    let pelvis = app.bones[0].inverse_bind.expect("pelvis bind");
    assert!(close(
        &pelvis,
        &[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, -2.0],
            [0.0, 0.0, 0.0, 1.0],
        ]
    ));
    // A quarter turn about Z at half size, then the translation as stored.
    let bag = app.bones[1].inverse_bind.expect("bag bind");
    assert!(
        close(
            &bag,
            &[
                [0.0, -0.5, 0.0, 7.0],
                [0.5, 0.0, 0.0, 8.0],
                [0.0, 0.0, 0.5, 9.0],
                [0.0, 0.0, 0.0, 1.0],
            ]
        ),
        "{bag:?}"
    );
}

#[test]
fn a_bone_without_a_bind_rotation_has_no_bind_pose() {
    // The plain fixture leaves the bind fields zero, which is no rotation.
    let app = parse(&goblin()).unwrap();
    assert!(app.bones.iter().all(|b| b.inverse_bind.is_none()));
}

#[test]
fn the_export_skins_the_model_in_its_bind_pose() {
    let app = parse(&bound_goblin()).unwrap();
    let model = to_gltf(&app, &[], None, &[0]);
    assert_eq!(model.inverse_bind.len(), 2);
    assert_eq!(model.inverse_bind[1], app.bones[1].inverse_bind.unwrap());
    // The rest pose the animations are keyed against is left alone.
    assert_eq!(model.bones[1].translation, [0.5, 0.0, 0.0]);
}

#[test]
fn the_export_falls_back_to_the_rest_pose_when_a_bind_pose_is_missing() {
    let app = parse(&goblin()).unwrap();
    assert!(to_gltf(&app, &[], None, &[0, 1]).inverse_bind.is_empty());
}

/// A material with one 248-byte record per look, shown where `shown` says.
fn material_in_looks(b: &mut Builder, name: &str, shown: &[bool]) -> Vec<u8> {
    let mut blk = Vec::new();
    for &on in shown {
        let mut e = vec![0u8; 248];
        e[0..4].copy_from_slice(&(4u32 | on as u32).to_le_bytes());
        e[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        blk.extend(e);
    }
    let blo = b.region(&blk);
    let mut e = name_field(name, 128);
    e.extend_from_slice(&blo.to_le_bytes());
    e.extend_from_slice(&(blk.len() as u32).to_le_bytes());
    e.resize(144, 0);
    e
}

/// Greed's death chest in miniature: her body, the chest, the bloody cloth
/// that only the second death shows, and an emitter surface no look shows.
fn chest() -> Appearance {
    let looks = ["A", "invisible", "noChest", "chestOnly", "A_death2"];
    let mats: [(&str, [bool; 5]); 4] = [
        ("A_normal_mat", [true, false, true, false, true]),
        ("Chest_mat", [true, false, false, true, true]),
        ("A_Cloth_Mat", [false, false, false, false, true]),
        ("FX_EMIT", [false; 5]),
    ];
    let mut b = Builder::new();
    let bones = bone("Root", -1, [0.0; 3]);
    b.table(0x24, 1, &bones);
    let mut parts = Vec::new();
    for m in [
        "FX_EMIT",
        "A_Cloth_Mat",
        "A_normal_mat",
        "Chest_mat",
        "A_Cloth_Mat",
    ] {
        parts.extend(part(&mut b, m));
    }
    b.table(0xa8, 5, &parts);
    let mut table = Vec::new();
    for (name, shown) in &mats {
        table.extend(material_in_looks(&mut b, name, shown));
    }
    b.table(0x1b4, mats.len(), &table);
    let names: Vec<u8> = looks.iter().flat_map(|l| name_field(l, 64)).collect();
    let lo = b.region(&names);
    b.put(0x1c8, lo);
    b.put(0x1cc, names.len() as u32);
    parse(&b.b).unwrap()
}

#[test]
fn a_material_records_the_looks_it_is_shown_in() {
    let app = chest();
    assert_eq!(
        app.material("Chest_mat").unwrap().shown,
        [true, false, false, true, true]
    );
    assert_eq!(app.material("FX_EMIT").unwrap().shown, [false; 5]);
}

#[test]
fn a_look_shows_the_parts_whose_material_it_shows() {
    let app = chest();
    assert_eq!(parts_in_look(&app, 0), vec![2, 3], "A: body and chest");
    assert_eq!(parts_in_look(&app, 3), vec![3], "chestOnly");
    assert_eq!(
        parts_in_look(&app, 4),
        vec![1, 2, 3, 4],
        "A_death2 adds the cloth"
    );
    assert!(parts_in_look(&app, 1).is_empty(), "invisible");
    assert!(parts_in_look(&app, 9).is_empty(), "no such look");
}

#[test]
fn the_first_look_is_what_gets_drawn() {
    // Not every part, and not the old guess by material name: the cloth
    // and the emitter surface stay out.
    assert_eq!(drawn_parts(&chest()), vec![2, 3]);
}

#[test]
fn looks_that_show_nothing_are_not_offered() {
    let offered: Vec<(usize, String)> = offered_looks(&chest());
    let names: Vec<&str> = offered.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, ["A", "noChest", "chestOnly", "A_death2"]);
    assert_eq!(offered[3].0, 4, "each keeps its place in the file");
}

#[test]
fn a_model_without_look_records_offers_no_looks() {
    // The goblin fixture's records show nothing anywhere, so the old rule
    // picks the parts and there is nothing to choose between.
    let app = parse(&goblin()).unwrap();
    assert!(offered_looks(&app).is_empty());
    assert_eq!(drawn_parts(&app), vec![1]);
}

#[test]
fn the_export_holds_the_parts_it_is_given() {
    let app = chest();
    let names = |parts: &[usize]| -> Vec<String> {
        to_gltf(&app, &[], None, parts)
            .parts
            .into_iter()
            .map(|p| p.name)
            .collect()
    };
    assert_eq!(names(&parts_in_look(&app, 3)), ["Chest_mat"]);
    assert_eq!(
        names(&parts_in_look(&app, 4)),
        ["A_Cloth_Mat", "A_normal_mat", "Chest_mat", "A_Cloth_Mat"]
    );
}
