//! The game's armour sets, by name.
//!
//! `ItemSet` names a set ("Lawbringer Armor") and lists the items in it.
//! This reads that table and finds sets by what the user has typed so far.

use super::db2::Table;

pub const FDID: u32 = 1_343_609;

/// Where the columns are, for one layout of the table.
#[derive(Clone, Copy)]
struct Columns {
    layout: u32,
    /// `Name_lang`
    name: usize,
    /// `ItemID[]`
    items: usize,
}

const KNOWN: &[Columns] = &[
    // WoW Forever (wowf), build 1.60.1.
    Columns {
        layout: 0xF790_68A4,
        name: 0,
        items: 4,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSet {
    pub id: u32,
    pub name: String,
    /// The set's items, without the table's empty slots.
    pub items: Vec<u32>,
}

/// Every named set that has at least one item, by name.
pub fn parse(table: &[u8]) -> Result<Vec<ItemSet>, String> {
    let layout = Table::parse(table, &[])?.layout_hash;
    let columns = KNOWN
        .iter()
        .find(|c| c.layout == layout)
        .ok_or_else(|| format!("ItemSet has a layout this version does not know ({layout:08X})"))?;
    parse_with(table, *columns)
}

fn parse_with(table: &[u8], c: Columns) -> Result<Vec<ItemSet>, String> {
    let table = Table::parse_text(table, &[c.name, c.items], &[c.name])?;
    let mut sets: Vec<ItemSet> = table
        .rows
        .iter()
        .filter_map(|row| {
            let name = table.text(*row.fields[0].first()?)?.trim();
            let items: Vec<u32> = row.fields[1]
                .iter()
                .map(|&i| i as u32)
                .filter(|&i| i != 0)
                .collect();
            (!name.is_empty() && !items.is_empty()).then(|| ItemSet {
                id: row.id,
                name: name.to_owned(),
                items,
            })
        })
        .collect();
    sets.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(sets)
}

/// The sets to suggest for what has been typed: up to `limit`, those whose
/// name starts with it first, then those that contain it.  Case is ignored.
/// Nothing typed suggests nothing.
pub fn suggest<'a>(sets: &'a [ItemSet], typed: &str, limit: usize) -> Vec<&'a ItemSet> {
    let typed = typed.trim().to_lowercase();
    if typed.is_empty() {
        return Vec::new();
    }
    let named: Vec<(&ItemSet, String)> = sets.iter().map(|s| (s, s.name.to_lowercase())).collect();
    let starts = named.iter().filter(|(_, n)| n.starts_with(&typed));
    let contains = named
        .iter()
        .filter(|(_, n)| !n.starts_with(&typed) && n.contains(&typed));
    starts
        .chain(contains)
        .take(limit)
        .map(|(s, _)| *s)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::db2::tests::{file, record, text_ref, Section, HAS_ID_LIST};
    use super::*;

    /// The test table's column 0 is plain (the name) and column 3 a list of
    /// two (`[1, 2]`, `[3, 4]` or `[0, 0]` for pallet index 0, 1, 2).
    const TEST: Columns = Columns {
        layout: 0xABCD_0123,
        name: 0,
        items: 3,
    };

    fn table() -> Vec<u8> {
        let strings = b"\0Lawbringer Armor\0Nameless\0".to_vec();
        let mut sections = vec![Section {
            key: 0,
            records: vec![vec![0; 8]; 3],
            ids: vec![208, 5, 6],
            copies: vec![],
            parents: vec![],
            strings,
        }];
        // Row 208: named, items [3, 4].  Row 5: named, no items.  Row 6:
        // items but no name.
        let rows = [(1, 1), (18, 2), (0, 0)];
        for (i, (at, items)) in rows.into_iter().enumerate() {
            let name = if at == 0 {
                0
            } else {
                text_ref(&sections, i, at)
            };
            sections[0].records[i] = record(name, 0, 0, items, 0);
        }
        file(HAS_ID_LIST, &sections)
    }

    #[test]
    fn a_set_has_its_name_and_its_items() {
        assert_eq!(
            parse_with(&table(), TEST).unwrap(),
            [ItemSet {
                id: 208,
                name: "Lawbringer Armor".into(),
                items: vec![3, 4]
            }],
            "sets with no name or no items are left out"
        );
    }

    #[test]
    fn a_table_with_another_layout_is_refused_by_hash() {
        let err = parse(&table()).unwrap_err();
        assert!(err.contains("ABCD0123"), "{err}");
    }

    fn sets(names: &[&str]) -> Vec<ItemSet> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| ItemSet {
                id: i as u32,
                name: (*n).to_owned(),
                items: vec![1],
            })
            .collect()
    }

    fn suggested(names: &[&str], typed: &str, limit: usize) -> Vec<String> {
        suggest(&sets(names), typed, limit)
            .iter()
            .map(|s| s.name.clone())
            .collect()
    }

    const NAMES: [&str; 4] = [
        "Battlegear of Might",
        "Judgement Armor",
        "Lawbringer Armor",
        "Might of the Storm",
    ];

    #[test]
    fn names_that_start_with_what_was_typed_come_first() {
        assert_eq!(
            suggested(&NAMES, "might", 10),
            ["Might of the Storm", "Battlegear of Might"]
        );
    }

    #[test]
    fn case_and_stray_spaces_do_not_matter() {
        assert_eq!(suggested(&NAMES, "  LAWB ", 10), ["Lawbringer Armor"]);
    }

    #[test]
    fn no_more_than_the_limit_are_suggested() {
        assert_eq!(suggested(&NAMES, "armor", 1), ["Judgement Armor"]);
    }

    #[test]
    fn nothing_typed_suggests_nothing() {
        assert!(suggested(&NAMES, "", 10).is_empty());
        assert!(suggested(&NAMES, "   ", 10).is_empty());
        assert!(suggested(&NAMES, "zzz", 10).is_empty());
    }
}
