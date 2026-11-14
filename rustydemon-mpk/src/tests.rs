use std::fs;

use md5::{Digest, Md5};

use super::*;

/// Build an `.mpkinfo` the way the game lays it out: version, count,
/// entries of (u16 name length, name, u32 offset, u32 size, u32 archive
/// field), then an MD5 of everything before it.
fn mpkinfo(entries: &[(&str, u32, u32, u32)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, off, size, field) in entries {
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&off.to_le_bytes());
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&field.to_le_bytes());
    }
    let digest = Md5::digest(&b);
    b.extend_from_slice(&digest);
    b
}

fn zzz4(plain: &[u8]) -> Vec<u8> {
    let mut b = b"ZZZ4".to_vec();
    b.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    b.extend_from_slice(&lz4_flex::block::compress(plain));
    b
}

#[test]
fn index_entries_parse() {
    let info = mpkinfo(&[
        ("Package/a.anim", 0, 10, 0),
        ("Package/b", 10, 4, (3 << 1) | 1),
    ]);
    let entries = parse_index(&info).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "Package/a.anim");
    assert_eq!(
        (entries[0].offset, entries[0].size, entries[0].archive),
        (0, 10, 0)
    );
    assert!(!entries[0].flag);
    assert_eq!(
        (entries[1].offset, entries[1].size, entries[1].archive),
        (10, 4, 3)
    );
    assert!(entries[1].flag);
}

#[test]
fn index_with_a_bad_checksum_is_rejected() {
    let mut info = mpkinfo(&[("Package/a", 0, 1, 0)]);
    info[10] ^= 0xFF;
    assert!(matches!(parse_index(&info), Err(MpkError::Checksum)));
}

#[test]
fn truncated_index_is_rejected() {
    let info = mpkinfo(&[("Package/a", 0, 1, 0)]);
    assert!(parse_index(&info[..12]).is_err());
}

#[test]
fn zzz4_payload_decompresses() {
    let plain = b"CHAR::ANIM CHAR::ANIM CHAR::ANIM the quick crab".repeat(4);
    assert_eq!(decode_payload(&zzz4(&plain)).unwrap(), plain);
}

#[test]
fn raw_payload_passes_through() {
    let raw = b".MESSIAH\x02\x00\x00\x00 raw object".to_vec();
    assert_eq!(decode_payload(&raw).unwrap(), raw);
}

#[test]
fn zzz4_with_a_wrong_size_is_an_error() {
    let mut z = zzz4(b"hello hello hello");
    z[4] = 99;
    assert!(decode_payload(&z).is_err());
}

#[test]
fn archive_reads_files_from_both_packages() {
    let dir = std::env::temp_dir().join(format!("mpk-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let anim = zzz4(&b"anim bytes ".repeat(20));
    let obj = b".MESSIAH object".to_vec();
    // Resources.mpk holds anim, Resources2.mpk holds obj at offset 3.
    fs::write(dir.join("Resources.mpk"), &anim).unwrap();
    let mut r2 = b"xyz".to_vec();
    r2.extend_from_slice(&obj);
    fs::write(dir.join("Resources2.mpk"), &r2).unwrap();
    fs::write(
        dir.join("Resources.mpkinfo"),
        mpkinfo(&[
            ("Package/a.anim", 0, anim.len() as u32, 0),
            ("Package/b", 3, obj.len() as u32, 2 << 1),
        ]),
    )
    .unwrap();
    let cfg = b"engine config".to_vec();
    fs::write(dir.join("Engine.mpk"), &cfg).unwrap();
    fs::write(
        dir.join("Engine.mpkinfo"),
        mpkinfo(&[("Engine/Config/x.bin", 0, cfg.len() as u32, 0)]),
    )
    .unwrap();

    let archive = MpkArchive::open(&dir).unwrap();
    assert_eq!(archive.len(), 3);
    let read = |p: &str| {
        let i = archive.entries().iter().position(|e| e.path == p).unwrap();
        archive.read(i).unwrap()
    };
    assert_eq!(read("Package/a.anim"), b"anim bytes ".repeat(20));
    assert_eq!(read("Package/b"), obj);
    assert_eq!(read("Engine/Config/x.bin"), cfg);

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn entry_past_the_end_of_its_archive_is_an_error() {
    let dir = std::env::temp_dir().join(format!("mpk-test-short-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Resources.mpk"), b"tiny").unwrap();
    fs::write(
        dir.join("Resources.mpkinfo"),
        mpkinfo(&[("Package/a", 2, 50, 0)]),
    )
    .unwrap();
    let archive = MpkArchive::open(&dir).unwrap();
    assert!(archive.read(0).is_err());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn directory_without_mpkinfo_is_not_an_archive() {
    let dir = std::env::temp_dir().join(format!("mpk-test-empty-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    assert!(MpkArchive::open(&dir).is_err());
    fs::remove_dir_all(&dir).unwrap();
}
