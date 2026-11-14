use super::material::*;

#[test]
fn each_sub_material_keeps_its_base_map() {
    let mut b = b".MESSIAH\x02\x00\x00\x00\x00\x00\x02\x1b\x00warlock_yuanchuemo_body_mat\x0b\x00PBR_Monster\x00\x01\x00\x00".to_vec();
    let first = "cSkinColor=(1,1,1)\ntBaseMap=baaa36a5-6bbc-4065-a26d-457e49135efc\ntNormalMap=cfa1ba97-a487-43e3-9143-51f4136c165f\n";
    let second = "tBaseMap=49ccfd05-8852-402b-bd94-18167825fa31\ncAlpha=1\n";
    b.extend_from_slice(&(first.len() as u16).to_le_bytes());
    b.extend_from_slice(first.as_bytes());
    b.extend_from_slice(b"\x00\x01\x00\x00");
    b.extend_from_slice(&(second.len() as u16).to_le_bytes());
    b.extend_from_slice(second.as_bytes());
    let m = Material::parse(&b);
    assert_eq!(m.name.as_deref(), Some("warlock_yuanchuemo_body_mat"));
    assert_eq!(
        m.base_maps,
        vec![
            Some("baaa36a5-6bbc-4065-a26d-457e49135efc".to_owned()),
            Some("49ccfd05-8852-402b-bd94-18167825fa31".to_owned()),
        ]
    );
}

#[test]
fn an_empty_base_map_is_none() {
    let b = b".MESSIAH\x02\x00\x00\x00\x00\x00\x01\x05\x00x_mat\x03\x00PBR\x00\x00\x00\x00\x30\x00tBaseMap=00000000-0000-0000-0000-000000000000\ncA=1\n";
    assert_eq!(Material::parse(b).base_maps, vec![None]);
}
