use super::d3_stl::*;

/// A StringList SNO: header, entry array at 0x38 (offset 0x28 from byte
/// 16), then the NUL-terminated strings, offsets counting from byte 16.
fn stringlist(entries: &[(&str, &str, u32)]) -> Vec<u8> {
    let array = entries.len() * 40;
    let mut strings = Vec::new();
    let mut table = Vec::new();
    let put = |s: &str, strings: &mut Vec<u8>| {
        let off = 0x28 + array + strings.len();
        strings.extend_from_slice(s.as_bytes());
        strings.push(0);
        (off as u32, s.len() as u32 + 1)
    };
    for (label, text, hash) in entries {
        let (lo, ln) = put(label, &mut strings);
        let (to, tn) = put(text, &mut strings);
        table.extend_from_slice(&[0; 8]);
        table.extend_from_slice(&lo.to_le_bytes());
        table.extend_from_slice(&ln.to_le_bytes());
        table.extend_from_slice(&[0; 8]);
        table.extend_from_slice(&to.to_le_bytes());
        table.extend_from_slice(&tn.to_le_bytes());
        table.extend_from_slice(&hash.to_le_bytes());
        table.extend_from_slice(&[0; 4]);
    }
    let mut b = vec![0u8; 0x38];
    b[..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    b[4..8].copy_from_slice(&0x2Du32.to_le_bytes());
    b[0x28..0x2c].copy_from_slice(&0x28u32.to_le_bytes());
    b[0x2c..0x30].copy_from_slice(&(array as u32).to_le_bytes());
    b.extend_from_slice(&table);
    b.extend_from_slice(&strings);
    b
}

#[test]
fn entries_pair_each_label_with_its_text() {
    let data = stringlist(&[
        ("A1_BlackMushroom", "Black Mushroom", 0x8E6C_4E88),
        ("A1SaveBlacksmithReward", "Damaged Chestpiece", 1),
    ]);
    let list = parse(&data).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].label, "A1_BlackMushroom");
    assert_eq!(list[0].text, "Black Mushroom");
    assert_eq!(list[0].hash, 0x8E6C_4E88);
    assert_eq!(list[1].text, "Damaged Chestpiece");
}

#[test]
fn text_keeps_its_markup_and_line_breaks() {
    let data = stringlist(&[("Tip", "Press {c:ffff0000}Q{/c}\nto drink", 0)]);
    assert_eq!(
        parse(&data).unwrap()[0].text,
        "Press {c:ffff0000}Q{/c}\nto drink"
    );
}

#[test]
fn only_stl_files_are_claimed() {
    let data = stringlist(&[("a", "b", 0)]);
    assert!(claims("enUS/StringList/Items.stl", &data));
    // Condition SNOs carry the same 0x2D after the magic.
    assert!(!claims("Base/Condition/Foo.cnd", &data));
    assert!(!claims("enUS/StringList/Items.stl", b"not a sno"));
}

#[test]
fn string_past_the_end_is_an_error() {
    let mut data = stringlist(&[("label", "text", 0)]);
    data.truncate(data.len() - 2);
    assert!(parse(&data).is_err());
}

#[test]
fn array_that_is_not_whole_entries_is_an_error() {
    let mut data = stringlist(&[("label", "text", 0)]);
    data[0x2c] = 39;
    assert!(parse(&data).is_err());
}

#[test]
fn listing_shows_label_and_text_per_line() {
    let data = stringlist(&[
        ("A1_BlackMushroom", "Black Mushroom", 0),
        ("Tip", "Two\nlines", 0),
    ]);
    let shown = render(&parse(&data).unwrap());
    assert!(shown.starts_with("2 strings\n"), "{shown}");
    assert!(
        shown.contains("A1_BlackMushroom = Black Mushroom\n"),
        "{shown}"
    );
    // A line break inside a text stays visible as \n so each entry is one line.
    assert!(shown.contains("Tip = Two\\nlines\n"), "{shown}");
}
