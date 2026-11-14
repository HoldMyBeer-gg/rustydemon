use super::d3::*;
use crate::{jenkins96, root::RootHandler, types::Md5Hash, CascError};

fn key(n: u8) -> Md5Hash {
    Md5Hash([n; 16])
}

/// Top-level root: magic, count, then (content key, NUL-terminated name).
fn root(dirs: &[(u8, &str)]) -> Vec<u8> {
    let mut b = ROOT_MAGIC.to_le_bytes().to_vec();
    b.extend_from_slice(&(dirs.len() as u32).to_le_bytes());
    for (k, name) in dirs {
        b.extend_from_slice(&[*k; 16]);
        b.extend_from_slice(name.as_bytes());
        b.push(0);
    }
    b
}

/// A sub-root: SNO files, SNO sub-files, then named files.
fn subdir(snos: &[(u8, u32)], subs: &[(u8, u32, u32)], named: &[(u8, &str)]) -> Vec<u8> {
    let mut b = SUBDIR_MAGIC.to_le_bytes().to_vec();
    b.extend_from_slice(&(snos.len() as u32).to_le_bytes());
    for (k, sno) in snos {
        b.extend_from_slice(&[*k; 16]);
        b.extend_from_slice(&sno.to_le_bytes());
    }
    b.extend_from_slice(&(subs.len() as u32).to_le_bytes());
    for (k, sno, sub) in subs {
        b.extend_from_slice(&[*k; 16]);
        b.extend_from_slice(&sno.to_le_bytes());
        b.extend_from_slice(&sub.to_le_bytes());
    }
    b.extend_from_slice(&(named.len() as u32).to_le_bytes());
    for (k, name) in named {
        b.extend_from_slice(&[*k; 16]);
        b.extend_from_slice(name.as_bytes());
        b.push(0);
    }
    b
}

/// CoreTOC.dat: 70 counts, 70 offsets, 70 more counts, one u32, then per
/// group its (group, SNO, name offset) entries followed by the names.
fn core_toc(entries: &[(u32, u32, &str)]) -> Vec<u8> {
    let mut counts = [0u32; 70];
    let mut offsets = [0u32; 70];
    let mut blocks = Vec::new();
    for g in 0..70u32 {
        let mine: Vec<_> = entries.iter().filter(|e| e.0 == g).collect();
        offsets[g as usize] = blocks.len() as u32;
        counts[g as usize] = mine.len() as u32;
        let mut names = Vec::new();
        for (group, sno, name) in &mine {
            blocks.extend_from_slice(&group.to_le_bytes());
            blocks.extend_from_slice(&sno.to_le_bytes());
            blocks.extend_from_slice(&(names.len() as u32).to_le_bytes());
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        blocks.extend_from_slice(&names);
    }
    let mut b = Vec::new();
    for v in counts.iter().chain(&offsets).chain(&counts) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&blocks);
    b
}

fn packages(names: &[&str]) -> Vec<u8> {
    let mut b = PACKAGES_MAGIC.to_le_bytes().to_vec();
    b.extend_from_slice(&(names.len() as u32).to_le_bytes());
    for n in names {
        b.extend_from_slice(n.as_bytes());
        b.push(0);
    }
    b
}

#[test]
fn root_lists_named_sub_roots() {
    let data = root(&[(1, "Base"), (2, "enUS")]);
    assert!(is_d3_root(&data));
    assert!(!is_d3_root(b"MNDX...."));
    assert_eq!(
        parse_root(&data).unwrap(),
        vec![(key(1), "Base".to_owned()), (key(2), "enUS".to_owned())]
    );
}

#[test]
fn sub_root_holds_sno_sub_and_named_files() {
    let s = parse_subdir(&subdir(&[(3, 305)], &[(4, 900, 7)], &[(5, "CoreTOC.dat")])).unwrap();
    assert_eq!(s.snos, vec![(key(3), 305)]);
    assert_eq!(s.subs, vec![(key(4), 900, 7)]);
    assert_eq!(s.named, vec![(key(5), "CoreTOC.dat".to_owned())]);
}

#[test]
fn truncated_sub_root_is_rejected() {
    let b = subdir(&[(3, 305)], &[], &[]);
    assert!(parse_subdir(&b[..b.len() - 2]).is_err());
}

#[test]
fn core_toc_names_each_sno_and_its_group() {
    let toc = parse_core_toc(&core_toc(&[
        (1, 305, "a2dun_Zolt_Pedestal"),
        (44, 77, "GoldCoins"),
    ]))
    .unwrap();
    assert_eq!(toc[&305], (1, "a2dun_Zolt_Pedestal".to_owned()));
    assert_eq!(toc[&77], (44, "GoldCoins".to_owned()));
}

#[test]
fn group_folders_come_from_package_names() {
    let toc = parse_core_toc(&core_toc(&[
        (1, 1, "Zombie"),
        (1, 2, "Goatman"),
        (9, 3, "Zombie"),
        (34, 4, "OnlyOne"),
    ]))
    .unwrap();
    let pk = parse_packages(&packages(&[
        "Actor\\Zombie.acr",
        "Actor\\Goatman.acr",
        "Appearance\\Zombie.app",
        "SceneGroup\\OnlyOne.scg",
        "Actor\\OnlyOne.acr",
    ]))
    .unwrap();
    let folders = group_folders(&toc, &pk);
    assert_eq!(folders[&1], ("Actor".to_owned(), "acr".to_owned()));
    assert_eq!(folders[&9], ("Appearance".to_owned(), "app".to_owned()));
    // One match in a big folder and one in a folder of its own: the folder
    // whose size fits the group wins.
    assert_eq!(folders[&34], ("SceneGroup".to_owned(), "scg".to_owned()));
}

#[test]
fn handler_names_every_file_by_folder_and_locale() {
    let toc = core_toc(&[(1, 305, "Zombie"), (41, 900, "Rat")]);
    let pk = packages(&[
        "Actor\\Zombie.acr",
        "SoundBank\\Rat.sbk",
        "SoundBank\\Rat\\0007.ogg",
    ]);
    let files: Vec<(Md5Hash, Vec<u8>)> = vec![
        (key(10), root(&[(11, "Base"), (12, "enUS"), (13, "deDE")])),
        (
            key(11),
            subdir(
                &[(20, 305)],
                &[],
                &[(30, "CoreTOC.dat"), (31, "Data_D3\\PC\\Misc\\Packages.dat")],
            ),
        ),
        (
            key(12),
            subdir(&[(21, 900)], &[(22, 900, 7), (23, 900, 8)], &[]),
        ),
        (key(30), toc),
        (key(31), pk),
    ];
    let open = |k: &Md5Hash| -> Result<Vec<u8>, CascError> {
        files
            .iter()
            .find(|f| f.0 == *k)
            .map(|f| f.1.clone())
            .ok_or_else(|| CascError::FileNotFound(k.to_hex()))
    };
    // deDE is not installed: it is skipped, not fatal.
    let h = D3RootHandler::load(&files[0].1, open).unwrap();
    let paths: std::collections::BTreeMap<String, Md5Hash> = h
        .builtin_paths()
        .into_iter()
        .map(|(hash, p)| (p, h.get_all_entries(hash)[0].ckey))
        .collect();
    assert_eq!(paths["Base/Actor/Zombie.acr"], key(20));
    assert_eq!(paths["enUS/SoundBank/Rat.sbk"], key(21));
    assert_eq!(paths["enUS/SoundBank/Rat/0007.ogg"], key(22));
    // Sub-file with no package entry keeps its number and no extension.
    assert_eq!(paths["enUS/SoundBank/Rat/0008"], key(23));
    assert_eq!(paths["Base/CoreTOC.dat"], key(30));
    assert_eq!(paths["Base/Data_D3/PC/Misc/Packages.dat"], key(31));
    assert_eq!(paths.len(), 6);
    assert!(h.has_builtin_paths());
    assert_eq!(h.type_name(), "D3");
    assert_eq!(h.count(), 6);
    assert_eq!(
        h.get_all_entries(jenkins96("base/actor/zombie.acr"))[0].ckey,
        key(20)
    );
}

#[test]
fn sno_without_a_toc_name_is_listed_by_id() {
    let files: Vec<(Md5Hash, Vec<u8>)> = vec![
        (key(10), root(&[(11, "Base")])),
        (key(11), subdir(&[(20, 305)], &[(21, 305, 2)], &[])),
    ];
    let open = |k: &Md5Hash| -> Result<Vec<u8>, CascError> {
        files
            .iter()
            .find(|f| f.0 == *k)
            .map(|f| f.1.clone())
            .ok_or_else(|| CascError::FileNotFound(k.to_hex()))
    };
    let h = D3RootHandler::load(&files[0].1, open).unwrap();
    let mut paths: Vec<String> = h.builtin_paths().into_iter().map(|p| p.1).collect();
    paths.sort();
    assert_eq!(paths, vec!["Base/SNO/305", "Base/SNO/305/0002"]);
}
