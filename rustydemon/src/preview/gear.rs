//! What wearing an item does to a character.
//!
//! Most armour is not a model.  A chestpiece is three textures painted onto
//! the body sheet (upper arms, upper torso, lower torso); gloves are two
//! more and a different cuff from the model's own set of parts.  Only helms
//! and shoulders are models of their own.
//!
//! The chain from an item to those textures:
//!
//! * `ItemModifiedAppearance`: item -> appearance
//! * `ItemAppearance`: appearance -> display, and which slot it is
//! * `ItemDisplayInfo`: the parts to switch, and any models
//! * `ItemDisplayInfoMaterialRes`: display -> (rectangle, texture resource)
//! * `TextureFileData` + `ComponentTextureFileData`: resource -> one file
//!   per sex
//!
//! This works out the painted textures and the parts.  Models are counted
//! so the caller can say they are not drawn yet.

use std::collections::{HashMap, HashSet};

use super::character::{Source, TEXTURE_FILE};

/// `ComponentTextureFileData.GenderIndex`: 0 male, 1 female, anything else
/// (or no row) fits both.
const MALE: u32 = 0;
const FEMALE: u32 = 1;

/// One appearance an item can have.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Display {
    /// `ItemAppearance.DisplayType`: 0 head, 1 shoulders, 2 shirt, 3 chest,
    /// 4 waist, 5 legs, 6 feet, 7 wrists, 8 hands.
    pub slot: u32,
    /// `ItemDisplayInfo.GeosetGroup`: 0 leaves a group of parts alone,
    /// `n` picks its variant `n + 1`.
    pub geosets: [u32; 6],
    /// Whether the piece is (also) a model: a helm, a shoulder pad.
    pub has_model: bool,
}

/// The rows of the item tables that dressing a character needs.
#[derive(Debug, Default)]
pub struct Gear {
    /// Item -> display.
    pub item_display: HashMap<u32, u32>,
    pub displays: HashMap<u32, Display>,
    /// Display -> `(rectangle type, texture resource)`.
    pub sections: HashMap<u32, Vec<(u32, u32)>>,
    /// Texture resource -> `(file, sex)`.
    pub files: HashMap<u32, Vec<(u32, u32)>>,
}

/// What a set of worn items adds to a character.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outfit {
    /// `(rectangle type, texture file)`, in painting order.
    pub paints: Vec<(u32, u32)>,
    /// `(group, variant)` of the model parts the items pick.
    pub geosets: Vec<(u32, u32)>,
    /// How many of the items are models, which are not drawn.
    pub models: usize,
    /// How many of the items the tables have no appearance for.
    pub unknown: usize,
}

/// The groups of model parts a slot's `GeosetGroup` values pick from, in
/// order.  Gloves pick a cuff (group 4) and a hand attachment (23).
fn groups(slot: u32) -> &'static [u32] {
    match slot {
        0 => &[27, 21],
        1 => &[26],
        2 => &[8, 10],
        3 => &[8, 10, 13, 22, 28],
        4 => &[18],
        5 => &[11, 9, 13],
        6 => &[5, 20],
        8 => &[4, 23],
        _ => &[],
    }
}

/// When two pieces paint the same rectangle the later one covers the
/// earlier: trousers, then boots over them, then the chest, bracers,
/// gloves over the bracers, and the belt on top.
fn layer(slot: u32) -> u32 {
    match slot {
        2 => 0,
        5 => 1,
        6 => 2,
        3 => 3,
        7 => 4,
        8 => 5,
        4 => 6,
        _ => 7,
    }
}

impl Gear {
    /// What wearing `items` does to a body of the given sex.
    pub fn outfit(&self, items: &[u32], female: bool) -> Outfit {
        let mut out = Outfit::default();
        let mut worn: Vec<(u32, &Display)> = Vec::new();
        for item in items {
            let display = self.item_display.get(item);
            match display.and_then(|d| Some((*d, self.displays.get(d)?))) {
                Some(found) => worn.push(found),
                None => out.unknown += 1,
            }
        }
        worn.sort_by_key(|(_, d)| layer(d.slot));

        let sex = if female { FEMALE } else { MALE };
        let mut picked: HashSet<u32> = HashSet::new();
        for (id, display) in worn {
            out.models += usize::from(display.has_model);
            for &(section, resource) in self.sections.get(&id).into_iter().flatten() {
                let files = self.files.get(&resource).map(Vec::as_slice).unwrap_or(&[]);
                // The file made for this sex, else one that fits both.
                let file = files
                    .iter()
                    .find(|f| f.1 == sex)
                    .or_else(|| files.iter().find(|f| f.1 != MALE && f.1 != FEMALE));
                if let Some(&(file, _)) = file {
                    out.paints.push((section, file));
                }
            }
            for (&group, &value) in groups(display.slot).iter().zip(&display.geosets) {
                if value != 0 && picked.insert(group) {
                    out.geosets.push((group, value + 1));
                }
            }
        }
        out
    }
}

// WoW Forever (wowf), build 1.60.1.
const APPEARANCE_OF_ITEM: Source = Source {
    name: "ItemModifiedAppearance",
    fdid: 982_457,
    layout: 0x03A6_C979,
    // ItemAppearanceID, OrderIndex; the parent is the item.
    columns: &[3, 4],
};
const APPEARANCE: Source = Source {
    name: "ItemAppearance",
    fdid: 982_462,
    layout: 0x481C_4281,
    // DisplayType, ItemDisplayInfoID
    columns: &[0, 1],
};
const DISPLAY: Source = Source {
    name: "ItemDisplayInfo",
    fdid: 1_266_429,
    layout: 0x9F3A_B8A9,
    // ModelResourcesID[2], GeosetGroup[6]
    columns: &[10, 13],
};
const DISPLAY_TEXTURE: Source = Source {
    name: "ItemDisplayInfoMaterialRes",
    fdid: 1_280_614,
    layout: 0xAA46_2C0E,
    // ComponentSection, MaterialResourcesID; the parent is the display.
    columns: &[0, 1],
};
const TEXTURE_SEX: Source = Source {
    name: "ComponentTextureFileData",
    fdid: 1_278_239,
    layout: 0xB32B_030A,
    // GenderIndex; the row id is the file.
    columns: &[0],
};

impl Gear {
    /// Read the item tables.  `fetch` opens a file of the archive by
    /// FileDataID.  The error names the table that could not be used.
    pub fn load(fetch: &dyn Fn(u32) -> Option<Vec<u8>>) -> Result<Self, String> {
        let first = |row: &super::db2::Row, field: usize| {
            row.fields[field].first().copied().unwrap_or(0) as u32
        };

        // Appearance -> (slot, display).
        let appearances: HashMap<u32, (u32, u32)> = APPEARANCE
            .table(fetch)?
            .rows
            .iter()
            .map(|r| (r.id, (first(r, 0), first(r, 1))))
            .collect();

        // An item's first appearance is the one it has with no modifier.
        let mut of_item = APPEARANCE_OF_ITEM.table(fetch)?.rows;
        of_item.sort_by_key(|r| (first(r, 1), r.id));
        let mut item_display = HashMap::new();
        let mut slot_of_display: HashMap<u32, u32> = HashMap::new();
        for row in &of_item {
            if let Some(&(slot, display)) = appearances.get(&first(row, 0)) {
                item_display.entry(row.parent).or_insert(display);
                slot_of_display.entry(display).or_insert(slot);
            }
        }

        let displays = DISPLAY
            .table(fetch)?
            .rows
            .iter()
            .filter_map(|r| {
                let slot = *slot_of_display.get(&r.id)?;
                let mut geosets = [0u32; 6];
                for (to, from) in geosets.iter_mut().zip(&r.fields[1]) {
                    *to = *from as u32;
                }
                let has_model = r.fields[0].iter().any(|&m| m != 0);
                Some((
                    r.id,
                    Display {
                        slot,
                        geosets,
                        has_model,
                    },
                ))
            })
            .collect();

        let mut sections: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        for row in &DISPLAY_TEXTURE.table(fetch)?.rows {
            sections
                .entry(row.parent)
                .or_default()
                .push((first(row, 0), first(row, 1)));
        }
        for list in sections.values_mut() {
            list.sort_unstable();
        }

        let sex_of_file: HashMap<u32, u32> = TEXTURE_SEX
            .table(fetch)?
            .rows
            .iter()
            .map(|r| (r.id, first(r, 0)))
            .collect();
        let mut files: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        for row in &TEXTURE_FILE.table(fetch)?.rows {
            // A file with no row of its own fits both sexes.
            let sex = sex_of_file.get(&row.id).copied().unwrap_or(u32::MAX);
            files.entry(row.parent).or_default().push((row.id, sex));
        }
        for list in files.values_mut() {
            list.sort_unstable();
        }

        Ok(Self {
            item_display,
            displays,
            sections,
            files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHEST: u32 = 100;
    const GLOVES: u32 = 101;
    const HELM: u32 = 102;
    const BELT: u32 = 103;

    /// A chest that paints upper arm (0) and torso (3), gloves that paint
    /// the lower arm (1) and pick a cuff, a helm that is only a model, and
    /// a belt that paints the torso too.
    fn gear() -> Gear {
        let display = |slot, geosets, has_model| Display {
            slot,
            geosets,
            has_model,
        };
        Gear {
            item_display: HashMap::from([(CHEST, 1), (GLOVES, 2), (HELM, 3), (BELT, 4)]),
            displays: HashMap::from([
                (1, display(3, [0; 6], false)),
                (2, display(8, [3, 0, 0, 0, 0, 0], false)),
                (3, display(0, [0; 6], true)),
                (4, display(4, [0; 6], false)),
            ]),
            sections: HashMap::from([
                (1, vec![(0, 500), (3, 501)]),
                (2, vec![(1, 502)]),
                (4, vec![(3, 503)]),
            ]),
            files: HashMap::from([
                (500, vec![(9000, FEMALE), (9001, MALE)]),
                (501, vec![(9011, MALE), (9010, FEMALE)]),
                // One file for both sexes.
                (502, vec![(9020, 3)]),
                (503, vec![(9030, 3)]),
            ]),
        }
    }

    #[test]
    fn a_table_that_cannot_be_read_is_named() {
        let err = Gear::load(&|_| None).unwrap_err();
        assert!(err.contains("ItemAppearance"), "{err}");
    }

    #[test]
    fn a_painted_piece_gives_the_texture_made_for_the_bodys_sex() {
        let female = gear().outfit(&[CHEST], true);
        assert_eq!(female.paints, [(0, 9000), (3, 9010)]);
        let male = gear().outfit(&[CHEST], false);
        assert_eq!(male.paints, [(0, 9001), (3, 9011)]);
    }

    #[test]
    fn a_texture_made_for_both_sexes_fits_either() {
        assert_eq!(gear().outfit(&[GLOVES], true).paints, [(1, 9020)]);
        assert_eq!(gear().outfit(&[GLOVES], false).paints, [(1, 9020)]);
    }

    #[test]
    fn gloves_pick_their_cuff_from_the_models_parts() {
        // Value 3 in the first place of a glove: group 4, variant 4.
        assert_eq!(gear().outfit(&[GLOVES], true).geosets, [(4, 4)]);
        assert!(gear().outfit(&[CHEST], true).geosets.is_empty());
    }

    #[test]
    fn the_belt_is_painted_over_the_chest_whatever_order_they_come_in() {
        let out = gear().outfit(&[BELT, CHEST], true);
        assert_eq!(out.paints, [(0, 9000), (3, 9010), (3, 9030)]);
    }

    #[test]
    fn models_and_unknown_items_are_counted_not_painted() {
        let out = gear().outfit(&[HELM, 777, CHEST], true);
        assert_eq!((out.models, out.unknown), (1, 1));
        assert_eq!(out.paints.len(), 2);
    }
}
