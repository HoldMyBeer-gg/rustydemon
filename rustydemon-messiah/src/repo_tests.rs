use super::repository::*;

/// (GUID, name, folder index, type index, dependencies)
type Entry<'a> = ([u8; 16], &'a str, u16, u16, &'a [[u8; 16]]);

/// Build a registry the way the game lays it out.
fn registry(entries: &[Entry]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    let types = b"Texture2D;ParticleSystem;LodModel;Model;Mesh;Material";
    b.extend_from_slice(&(types.len() as u16).to_le_bytes());
    b.extend_from_slice(types);
    let folders = b"@^_^@;Char/new_dl_f_warlock";
    b.extend_from_slice(&0xffffu16.to_le_bytes());
    b.extend_from_slice(&(folders.len() as u32).to_le_bytes());
    b.extend_from_slice(folders);
    for (guid, name, folder, kind, deps) in entries {
        b.extend_from_slice(&[5, 0, 1, 0, 0]);
        b.extend_from_slice(guid);
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&folder.to_le_bytes());
        b.extend_from_slice(&kind.to_le_bytes());
        b.extend_from_slice(&(deps.len() as u16).to_le_bytes());
        for d in *deps {
            b.extend_from_slice(d);
        }
    }
    b
}

const MESH: [u8; 16] = [
    0x62, 0x25, 0xff, 0x69, 0xc0, 0x25, 0x48, 0xc6, 0xa7, 0x4e, 0x6e, 0xa0, 0xc5, 0x85, 0xab, 0xd4,
];
const MAT: [u8; 16] = [0xf3; 16];
const MODEL: [u8; 16] = [0x3f; 16];
const TEX: [u8; 16] = [0xba; 16];

fn sample() -> Repository {
    Repository::parse(&registry(&[
        (TEX, "warlock_yuanchemo_01_d", 1, 0, &[]),
        (MESH, "Mesh_warlock_yuanchuemo_body", 1, 4, &[]),
        (MAT, "warlock_yuanchuemo_body_mat", 1, 5, &[TEX]),
        (MODEL, "warlock_yuanchuemo_body", 1, 3, &[MAT, MESH]),
    ]))
    .unwrap()
}

#[test]
fn entries_parse_with_names_folders_and_types() {
    let r = sample();
    assert_eq!(r.len(), 4);
    let mesh = r.get("6225ff69-c025-48c6-a74e-6ea0c585abd4").unwrap();
    assert_eq!(mesh.name, "Mesh_warlock_yuanchuemo_body");
    assert_eq!(r.folder(mesh), "Char/new_dl_f_warlock");
    assert_eq!(r.kind(mesh), "Mesh");
}

#[test]
fn readable_paths_name_every_entry() {
    let r = sample();
    let mesh = r.get("6225ff69-c025-48c6-a74e-6ea0c585abd4").unwrap();
    assert_eq!(
        r.readable_path(mesh),
        "Char/new_dl_f_warlock/Mesh_warlock_yuanchuemo_body.mesh"
    );
    let guid = Some("6225ff69-c025-48c6-a74e-6ea0c585abd4".to_owned());
    assert_eq!(
        r.guid_for_path("Char/new_dl_f_warlock/Mesh_warlock_yuanchuemo_body.mesh"),
        guid
    );
    assert_eq!(
        r.guid_for_path("Package/62/6225ff69-c025-48c6-a74e-6ea0c585abd4"),
        guid
    );
    assert_eq!(
        r.guid_for_path("Package/62/6225ff69-c025-48c6-a74e-6ea0c585abd4.6"),
        guid
    );
    let tex = r.get(&uuid_string(&TEX)).unwrap();
    assert_eq!(
        r.readable_path(tex),
        "Char/new_dl_f_warlock/warlock_yuanchemo_01_d.texture2d"
    );
}

#[test]
fn a_mesh_finds_its_materials_through_the_model_that_uses_it() {
    let r = sample();
    let mats = r.materials_for_mesh("6225ff69-c025-48c6-a74e-6ea0c585abd4");
    assert_eq!(mats, vec![uuid_string(&MAT)]);
}

#[test]
fn truncated_registry_is_rejected() {
    let b = registry(&[(TEX, "warlock_yuanchemo_01_d", 1, 0, &[])]);
    assert!(Repository::parse(&b[..b.len() - 3]).is_err());
}

#[test]
fn package_paths_follow_the_guid() {
    assert_eq!(
        package_path("6225ff69-c025-48c6-a74e-6ea0c585abd4"),
        "Package/62/6225ff69-c025-48c6-a74e-6ea0c585abd4"
    );
}
