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
//! A helm or shoulder pad is found from the display's model resource:
//!
//! * `ModelFileData`: resource -> every file made for it
//! * `ComponentModelFileData`: file -> the race, sex and side it fits
//! * `ChrRaceXChrModel` + `ChrRaces`: the body's race, and the race whose
//!   models it wears when it has none of its own
//! * `HelmetGeosetData`: the parts of the head a helm hides, per race
//!
//! This works out the painted textures, the parts, and which model hangs
//! on which attachment point of the body.

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
    /// `ItemDisplayInfo.ModelResourcesID`: the models of a helm or a pair
    /// of shoulder pads, left then right; 0 for none.
    pub models: [u32; 2],
    /// `ItemDisplayInfo.ModelMaterialResourcesID`: the texture each of
    /// those models wears.
    pub model_textures: [u32; 2],
    /// `ItemDisplayInfo.HelmetGeosetVis`: the list of parts a helm hides
    /// on a male head, then on a female one.
    pub helm_hides: [u32; 2],
}

/// One file of a model resource and who it fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub file: u32,
    /// 0 male, 1 female, anything else both.
    pub sex: u32,
    /// 0 for every race.
    pub race: u32,
    /// 0 the left shoulder, 1 the right, -1 where sides do not apply.
    pub position: i32,
}

/// The body that wears the items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wearer {
    pub female: bool,
    /// 0 when no race uses the body.
    pub race: u32,
    /// The `(race, sex)` whose models this body wears where it has none
    /// of its own.
    pub fallback: Option<(u32, u32)>,
}

/// A model hung on one of the body's attachment points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attached {
    /// The attachment id in the body's `.m2`.
    pub point: u32,
    /// The model's file.
    pub model: u32,
    /// The file of the texture it wears, 0 for none.
    pub texture: u32,
}

/// Attachment ids of a character body.
pub const RIGHT_SHOULDER_POINT: u32 = 5;
pub const LEFT_SHOULDER_POINT: u32 = 6;
pub const HELM_POINT: u32 = 11;

const HEAD: u32 = 0;
const SHOULDERS: u32 = 1;

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
    /// Model resource -> its files.
    pub model_files: HashMap<u32, Vec<ModelFile>>,
    /// Helm hide list -> `(race, group of parts)`.
    pub hides: HashMap<u32, Vec<(u32, u32)>>,
    /// Character model -> a race that uses it.
    pub races: HashMap<u32, u32>,
    /// Race -> the `(race, sex)` whose models its men, then its women, wear.
    pub fallbacks: HashMap<u32, [(u32, u32); 2]>,
}

/// What a set of worn items adds to a character.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outfit {
    /// `(rectangle type, texture file)`, in painting order.
    pub paints: Vec<(u32, u32)>,
    /// `(group, variant)` of the model parts the items pick.
    pub geosets: Vec<(u32, u32)>,
    /// The models to hang on the body.
    pub attached: Vec<Attached>,
    /// How many models the items call for that no file fits this body.
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
    /// Who wears the items: the body of character model `model`.
    pub fn wearer(&self, model: u32, female: bool) -> Wearer {
        let race = self.races.get(&model).copied().unwrap_or(0);
        let fallback = self
            .fallbacks
            .get(&race)
            .map(|both| both[usize::from(female)])
            .filter(|&(race, _)| race != 0);
        Wearer {
            female,
            race,
            fallback,
        }
    }

    /// The file of a texture resource made for this sex, else one that
    /// fits both.
    fn texture(&self, resource: u32, sex: u32) -> Option<u32> {
        let files = self.files.get(&resource)?;
        files
            .iter()
            .find(|f| f.1 == sex)
            .or_else(|| files.iter().find(|f| f.1 != MALE && f.1 != FEMALE))
            .map(|f| f.0)
    }

    /// The file of a model resource that fits the wearer: one made for
    /// their race, else for the race they borrow from, else for everyone.
    /// `side` narrows a pair of shoulder pads to the left or right one.
    fn model(&self, resource: u32, wearer: &Wearer, side: Option<i32>) -> Option<u32> {
        let files = self.model_files.get(&resource)?;
        let sided = |f: &&ModelFile| side.is_none_or(|s| f.position == s);
        // A pad stored once serves both sides.
        let any_sided = files.iter().any(|f| sided(&f));
        let fits = |race: u32, sex: u32| {
            files
                .iter()
                .filter(|f| !any_sided || sided(f))
                .find(|f| f.race == race && (f.sex == sex || f.sex > FEMALE))
                .map(|f| f.file)
        };
        let sex = if wearer.female { FEMALE } else { MALE };
        fits(wearer.race, sex)
            .or_else(|| wearer.fallback.and_then(|(race, sex)| fits(race, sex)))
            .or_else(|| fits(0, sex))
    }

    /// What wearing `items` does to the body of `wearer`.
    pub fn outfit(&self, items: &[u32], wearer: &Wearer) -> Outfit {
        let female = wearer.female;
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
            let points: &[(u32, Option<i32>)] = match display.slot {
                HEAD => &[(HELM_POINT, None)],
                SHOULDERS => &[
                    (LEFT_SHOULDER_POINT, Some(0)),
                    (RIGHT_SHOULDER_POINT, Some(1)),
                ],
                _ => &[],
            };
            for (place, &(point, side)) in points.iter().enumerate() {
                let resource = display.models[place];
                if resource == 0 {
                    continue;
                }
                match self.model(resource, wearer, side) {
                    Some(model) => out.attached.push(Attached {
                        point,
                        model,
                        texture: self
                            .texture(display.model_textures[place], sex)
                            .unwrap_or(0),
                    }),
                    None => out.models += 1,
                }
            }
            if display.slot == HEAD {
                let list = display.helm_hides[usize::from(female)];
                for &(race, group) in self.hides.get(&list).into_iter().flatten() {
                    if race == wearer.race && picked.insert(group) {
                        out.geosets.push((group, 0));
                    }
                }
            }
            for &(section, resource) in self.sections.get(&id).into_iter().flatten() {
                if let Some(file) = self.texture(resource, sex) {
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
    // ModelResourcesID[2], ModelMaterialResourcesID[2], GeosetGroup[6],
    // HelmetGeosetVis[2]
    columns: &[10, 11, 13, 15],
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

const MODEL_FILE: Source = Source {
    name: "ModelFileData",
    fdid: 1_337_833,
    layout: 0x2AE4_E788,
    // The row id is the file and the parent the resource.
    columns: &[],
};
const MODEL_FIT: Source = Source {
    name: "ComponentModelFileData",
    fdid: 1_349_053,
    layout: 0xAD90_D87A,
    // GenderIndex, RaceID, PositionIndex; the row id is the file.
    columns: &[0, 2, 3],
};
const HELM_HIDES: Source = Source {
    name: "HelmetGeosetData",
    fdid: 2_821_752,
    layout: 0x103B_3B37,
    // RaceID, HideGeosetGroup; the parent is the hide list.
    columns: &[0, 1],
};
const RACE_MODEL: Source = Source {
    name: "ChrRaceXChrModel",
    fdid: 3_490_304,
    layout: 0xA203_BC29,
    // ChrModelID; the parent is the race.
    columns: &[1],
};
const RACE: Source = Source {
    name: "ChrRaces",
    fdid: 1_305_311,
    layout: 0x4F44_C796,
    // Male model fallback race and sex, then the female ones.
    columns: &[41, 42, 43, 44],
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
                for (to, from) in geosets.iter_mut().zip(&r.fields[2]) {
                    *to = *from as u32;
                }
                let pair = |field: usize| {
                    let mut out = [0u32; 2];
                    for (to, from) in out.iter_mut().zip(&r.fields[field]) {
                        *to = *from as u32;
                    }
                    out
                };
                Some((
                    r.id,
                    Display {
                        slot,
                        geosets,
                        models: pair(0),
                        model_textures: pair(1),
                        helm_hides: pair(3),
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

        let fit: HashMap<u32, (u32, u32, i32)> = MODEL_FIT
            .table(fetch)?
            .rows
            .iter()
            .map(|r| {
                let position = r.fields[2].first().copied().unwrap_or(0) as i32;
                (r.id, (first(r, 0), first(r, 1), position))
            })
            .collect();
        let mut model_files: HashMap<u32, Vec<ModelFile>> = HashMap::new();
        for row in &MODEL_FILE.table(fetch)?.rows {
            if let Some(&(sex, race, position)) = fit.get(&row.id) {
                model_files.entry(row.parent).or_default().push(ModelFile {
                    file: row.id,
                    sex,
                    race,
                    position,
                });
            }
        }
        for list in model_files.values_mut() {
            list.sort_unstable_by_key(|f| f.file);
        }

        let mut hides: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        for row in &HELM_HIDES.table(fetch)?.rows {
            hides
                .entry(row.parent)
                .or_default()
                .push((first(row, 0), first(row, 1)));
        }
        for list in hides.values_mut() {
            list.sort_unstable();
        }

        let mut race_models = RACE_MODEL.table(fetch)?.rows;
        race_models.sort_by_key(|r| (r.parent, r.id));
        let mut races = HashMap::new();
        for row in &race_models {
            races.entry(first(row, 0)).or_insert(row.parent);
        }
        let fallbacks = RACE
            .table(fetch)?
            .rows
            .iter()
            .map(|r| {
                (
                    r.id,
                    [(first(r, 0), first(r, 1)), (first(r, 2), first(r, 3))],
                )
            })
            .collect();

        Ok(Self {
            item_display,
            displays,
            sections,
            files,
            model_files,
            hides,
            races,
            fallbacks,
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
    const SHOULDERS: u32 = 104;

    const HUMAN: u32 = 1;
    const ELF: u32 = 10;
    /// A race with no helms of its own, which wears the elf ones.
    const SKY: u32 = 95;

    fn model(file: u32, sex: u32, race: u32, position: i32) -> ModelFile {
        ModelFile {
            file,
            sex,
            race,
            position,
        }
    }

    fn woman() -> Wearer {
        Wearer {
            female: true,
            race: ELF,
            fallback: None,
        }
    }

    fn man() -> Wearer {
        Wearer {
            female: false,
            ..woman()
        }
    }

    /// A chest that paints upper arm (0) and torso (3), gloves that paint
    /// the lower arm (1) and pick a cuff, a helm that is only a model, and
    /// a belt that paints the torso too.
    fn gear() -> Gear {
        let display = |slot, geosets, models: [u32; 2]| Display {
            slot,
            geosets,
            models,
            model_textures: [600, 600],
            helm_hides: [70, 71],
        };
        Gear {
            item_display: HashMap::from([
                (CHEST, 1),
                (GLOVES, 2),
                (HELM, 3),
                (BELT, 4),
                (SHOULDERS, 5),
            ]),
            displays: HashMap::from([
                (1, display(3, [0; 6], [0; 2])),
                (2, display(8, [3, 0, 0, 0, 0, 0], [0; 2])),
                (3, display(0, [0; 6], [40, 0])),
                (5, display(1, [0; 6], [41, 41])),
                (4, display(4, [0; 6], [0; 2])),
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
                (600, vec![(9600, 3)]),
            ]),
            model_files: HashMap::from([
                (
                    40,
                    vec![
                        model(8000, MALE, ELF, -1),
                        model(8001, FEMALE, ELF, -1),
                        model(8002, FEMALE, HUMAN, -1),
                    ],
                ),
                (41, vec![model(8010, 2, 0, 0), model(8011, 2, 0, 1)]),
            ]),
            // The helm's female list: an elf loses her hair and ears, a
            // human only her hair.
            hides: HashMap::from([(71, vec![(ELF, 0), (ELF, 7), (HUMAN, 0)])]),
            races: HashMap::from([(900, ELF), (901, SKY)]),
            fallbacks: HashMap::from([(SKY, [(ELF, MALE), (ELF, FEMALE)])]),
        }
    }

    #[test]
    fn a_table_that_cannot_be_read_is_named() {
        let err = Gear::load(&|_| None).unwrap_err();
        assert!(err.contains("ItemAppearance"), "{err}");
    }

    #[test]
    fn a_painted_piece_gives_the_texture_made_for_the_bodys_sex() {
        let female = gear().outfit(&[CHEST], &woman());
        assert_eq!(female.paints, [(0, 9000), (3, 9010)]);
        let male = gear().outfit(&[CHEST], &man());
        assert_eq!(male.paints, [(0, 9001), (3, 9011)]);
    }

    #[test]
    fn a_texture_made_for_both_sexes_fits_either() {
        assert_eq!(gear().outfit(&[GLOVES], &woman()).paints, [(1, 9020)]);
        assert_eq!(gear().outfit(&[GLOVES], &man()).paints, [(1, 9020)]);
    }

    #[test]
    fn gloves_pick_their_cuff_from_the_models_parts() {
        // Value 3 in the first place of a glove: group 4, variant 4.
        assert_eq!(gear().outfit(&[GLOVES], &woman()).geosets, [(4, 4)]);
        assert!(gear().outfit(&[CHEST], &woman()).geosets.is_empty());
    }

    #[test]
    fn the_belt_is_painted_over_the_chest_whatever_order_they_come_in() {
        let out = gear().outfit(&[BELT, CHEST], &woman());
        assert_eq!(out.paints, [(0, 9000), (3, 9010), (3, 9030)]);
    }

    #[test]
    fn unknown_items_are_counted_not_painted() {
        let out = gear().outfit(&[777, CHEST], &woman());
        assert_eq!((out.models, out.unknown), (0, 1));
        assert_eq!(out.paints.len(), 2);
    }

    fn attached(point: u32, model: u32) -> Attached {
        Attached {
            point,
            model,
            texture: 9600,
        }
    }

    #[test]
    fn a_helm_is_the_model_made_for_the_wearers_race_and_sex() {
        assert_eq!(
            gear().outfit(&[HELM], &woman()).attached,
            [attached(HELM_POINT, 8001)]
        );
        assert_eq!(
            gear().outfit(&[HELM], &man()).attached,
            [attached(HELM_POINT, 8000)]
        );
    }

    #[test]
    fn a_race_with_no_helm_of_its_own_wears_the_one_its_table_names() {
        let sky = gear().wearer(901, true);
        assert_eq!(
            sky,
            Wearer {
                female: true,
                race: SKY,
                fallback: Some((ELF, FEMALE)),
            }
        );
        let out = gear().outfit(&[HELM], &sky);
        assert_eq!(out.attached, [attached(HELM_POINT, 8001)]);
        assert_eq!(out.models, 0);
    }

    #[test]
    fn a_helm_no_race_of_the_wearer_has_is_counted_not_drawn() {
        let orc = Wearer {
            female: true,
            race: 2,
            fallback: None,
        };
        let out = gear().outfit(&[HELM], &orc);
        assert!(out.attached.is_empty());
        assert_eq!(out.models, 1);
    }

    #[test]
    fn shoulders_are_a_left_and_a_right_model() {
        assert_eq!(
            gear().outfit(&[SHOULDERS], &woman()).attached,
            [
                attached(LEFT_SHOULDER_POINT, 8010),
                attached(RIGHT_SHOULDER_POINT, 8011)
            ]
        );
    }

    #[test]
    fn a_helm_hides_the_parts_its_table_lists_for_the_race() {
        assert_eq!(gear().outfit(&[HELM], &woman()).geosets, [(0, 0), (7, 0)]);
        // The male list is another one, and empty here.
        assert!(gear().outfit(&[HELM], &man()).geosets.is_empty());
        // A race that borrows its helms is listed under its own number.
        let sky = gear().wearer(901, true);
        assert!(gear().outfit(&[HELM], &sky).geosets.is_empty());
    }

    #[test]
    fn a_body_no_race_uses_wears_only_what_fits_everyone() {
        let nobody = gear().wearer(12345, false);
        assert_eq!((nobody.race, nobody.fallback), (0, None));
        assert_eq!(gear().outfit(&[SHOULDERS], &nobody).attached.len(), 2);
    }
}
