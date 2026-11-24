//! What a player character looks like before any choices are made.
//!
//! A character's `.m2` leaves its skin, hair and eye textures empty, and
//! holds every hairstyle and ear shape at once.  The game fills those in
//! from the character creation screen: each option (skin colour, face, hair
//! style) has choices, and a choice brings textures and model parts with it.
//!
//! The textures are not used as they are.  A body skin is painted in layers
//! onto one sheet: the bare skin first, then the face, underwear and
//! markings, each into its own rectangle of the sheet.  The tables say which
//! layer goes where and how it is mixed in.
//!
//! This takes the first choice of every option, which is what the creation
//! screen shows before the player touches anything.

use std::collections::{HashMap, HashSet};

/// A layer mask that means "the whole sheet".
const WHOLE_SHEET: u32 = u32::MAX;

/// One body: a race and sex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub id: u32,
    /// The creature display that names the body's `.m2`.
    pub display: u32,
    /// Which set of sheets, rectangles and layers the body uses.
    pub layout: u32,
}

/// One thing the creation screen lets the player pick, or one pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ordered {
    pub id: u32,
    /// The model an option belongs to, or the option a choice belongs to.
    pub owner: u32,
    /// Position on screen; the lowest comes first.
    pub order: u32,
}

/// One effect of a choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Element {
    pub choice: u32,
    /// When set, the effect only applies together with this other choice:
    /// a face texture exists once per skin colour.
    pub related: u32,
    /// A model part to show, or 0.
    pub geoset: u32,
    /// A texture to paint, or 0.
    pub material: u32,
}

/// Where a kind of texture is painted and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub layout: u32,
    /// The model texture slot type the sheet fills (1 is the body skin).
    pub texture_type: u32,
    /// Painting order; the lowest goes on first.
    pub order: u32,
    pub blend: u32,
    /// One bit per rectangle type the layer covers, or all bits for the
    /// whole sheet.
    pub sections: u32,
    /// The kind of texture: bare skin, face, underwear and so on.
    pub target: u32,
}

/// The size of one sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sheet {
    pub layout: u32,
    pub texture_type: u32,
    pub width: u32,
    pub height: u32,
}

/// A named rectangle of the body sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub layout: u32,
    pub kind: u32,
    /// `[x, y, width, height]`
    pub rect: [u32; 4],
}

/// The rows of the game's character tables that deciding a look needs.
#[derive(Debug, Default)]
pub struct Data {
    pub models: Vec<Model>,
    pub options: Vec<Ordered>,
    pub choices: Vec<Ordered>,
    pub elements: Vec<Element>,
    /// Material id -> (kind of texture, texture resource).
    pub materials: HashMap<u32, (u32, u32)>,
    /// Geoset row id -> (group, variant).
    pub geosets: HashMap<u32, (u32, u32)>,
    /// Texture resource -> file.
    pub texture_files: HashMap<u32, u32>,
    pub layers: Vec<Layer>,
    pub sheets: Vec<Sheet>,
    pub sections: Vec<Section>,
    /// Creature display -> the `.m2` it draws.
    pub display_files: HashMap<u32, u32>,
}

/// One texture painted onto a sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paint {
    pub file: u32,
    /// `[x, y, width, height]` on the sheet.
    pub rect: [u32; 4],
    pub blend: u32,
}

/// How to make the texture for one kind of model texture slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetPlan {
    pub texture_type: u32,
    pub width: u32,
    pub height: u32,
    /// In painting order.
    pub paints: Vec<Paint>,
}

/// A character's default appearance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Look {
    /// `(group, variant)` of each model part the choices pick.
    pub geosets: Vec<(u32, u32)>,
    pub sheets: Vec<SheetPlan>,
}

impl Data {
    /// The default look of the character whose body is the `.m2` with
    /// FileDataID `model_file`.  `None` when no character uses that file.
    pub fn look(&self, model_file: u32) -> Option<Look> {
        let model = self
            .models
            .iter()
            .find(|m| self.display_files.get(&m.display) == Some(&model_file))?;

        // The first choice of each of the model's options.
        let options: HashSet<u32> = self
            .options
            .iter()
            .filter(|o| o.owner == model.id)
            .map(|o| o.id)
            .collect();
        let mut first: HashMap<u32, &Ordered> = HashMap::new();
        let mut offered: HashSet<u32> = HashSet::new();
        for c in self.choices.iter().filter(|c| options.contains(&c.owner)) {
            offered.insert(c.id);
            let best = first.entry(c.owner).or_insert(c);
            if (c.order, c.id) < (best.order, best.id) {
                *best = c;
            }
        }
        let chosen: HashSet<u32> = first.values().map(|c| c.id).collect();

        // Every group of parts some choice of this model could pick.  One
        // the chosen ones leave alone is emptied: "no necklace" has no row
        // of its own, it is the absence of one.
        let mut controlled: HashSet<u32> = HashSet::new();
        let mut geosets = Vec::new();
        // Kind of texture -> file.  The first one named wins.
        let mut files: HashMap<u32, u32> = HashMap::new();
        for e in &self.elements {
            let part = self.geosets.get(&e.geoset).copied();
            if let Some((group, _)) = part.filter(|_| offered.contains(&e.choice)) {
                controlled.insert(group);
            }
            if !chosen.contains(&e.choice) || (e.related != 0 && !chosen.contains(&e.related)) {
                continue;
            }
            if let Some(part) = part {
                geosets.push(part);
            }
            let file = self
                .materials
                .get(&e.material)
                .and_then(|&(target, resource)| {
                    Some((target, *self.texture_files.get(&resource)?))
                });
            if let Some((target, file)) = file {
                files.entry(target).or_insert(file);
            }
        }
        for group in controlled {
            if !geosets.iter().any(|g| g.0 == group) {
                geosets.push((group, 0));
            }
        }
        geosets.sort_unstable();
        geosets.dedup();

        let mut sheets = Vec::new();
        for sheet in self.sheets.iter().filter(|s| s.layout == model.layout) {
            let mut layers: Vec<&Layer> = self
                .layers
                .iter()
                .filter(|l| l.layout == model.layout && l.texture_type == sheet.texture_type)
                .collect();
            layers.sort_by_key(|l| l.order);
            let paints: Vec<Paint> = layers
                .iter()
                .filter_map(|l| {
                    Some(Paint {
                        file: *files.get(&l.target)?,
                        rect: self.rect(sheet, l.sections)?,
                        blend: l.blend,
                    })
                })
                .collect();
            if !paints.is_empty() {
                sheets.push(SheetPlan {
                    texture_type: sheet.texture_type,
                    width: sheet.width,
                    height: sheet.height,
                    paints,
                });
            }
        }
        Some(Look { geosets, sheets })
    }

    /// The rectangle of `sheet` that a layer with this section mask covers.
    fn rect(&self, sheet: &Sheet, mask: u32) -> Option<[u32; 4]> {
        if mask == WHOLE_SHEET {
            return Some([0, 0, sheet.width, sheet.height]);
        }
        self.sections
            .iter()
            .find(|s| s.layout == sheet.layout && s.kind < 32 && mask >> s.kind & 1 == 1)
            .map(|s| s.rect)
    }
}

/// One of the game's tables: where it is and which of its columns are read.
/// Tables carry no column names, so the layout this was worked out for is
/// listed by its hash; another layout is refused, not guessed at.
struct Source {
    name: &'static str,
    fdid: u32,
    layout: u32,
    columns: &'static [usize],
}

// WoW Forever (wowf), build 1.60.1.
const CHR_MODEL: Source = Source {
    name: "ChrModel",
    fdid: 3_384_313,
    layout: 0x03FA_B755,
    // DisplayID, CharComponentTextureLayoutID
    columns: &[4, 5],
};
const OPTION: Source = Source {
    name: "ChrCustomizationOption",
    fdid: 3_384_247,
    layout: 0xDCC2_A86E,
    // ChrModelID, OrderIndex
    columns: &[4, 5],
};
const CHOICE: Source = Source {
    name: "ChrCustomizationChoice",
    fdid: 3_450_554,
    layout: 0x9559_C358,
    // ChrCustomizationOptionID, OrderIndex
    columns: &[2, 5],
};
const ELEMENT: Source = Source {
    name: "ChrCustomizationElement",
    fdid: 3_512_765,
    layout: 0x6483_C37E,
    // Choice, related choice, ChrCustomizationGeosetID, ChrCustomizationMaterialID
    columns: &[0, 1, 2, 4],
};
const MATERIAL: Source = Source {
    name: "ChrCustomizationMaterial",
    fdid: 3_459_652,
    layout: 0xBE97_67E9,
    // ChrModelTextureTargetID, MaterialResourcesID
    columns: &[0, 1],
};
const GEOSET: Source = Source {
    name: "ChrCustomizationGeoset",
    fdid: 3_456_171,
    layout: 0xBF55_F2FB,
    // GeosetType, GeosetID
    columns: &[0, 1],
};
const TEXTURE_FILE: Source = Source {
    name: "TextureFileData",
    fdid: 982_459,
    layout: 0xBD7C_74C2,
    // UsageType; the row id is the file and the parent the resource.
    columns: &[1],
};
const LAYER: Source = Source {
    name: "ChrModelTextureLayer",
    fdid: 3_548_976,
    layout: 0xD058_3FB4,
    // TextureType, Layer, BlendMode, TextureSectionTypeBitMask, ChrModelTextureTargetID
    columns: &[0, 1, 3, 4, 7],
};
const SHEET: Source = Source {
    name: "ChrModelMaterial",
    fdid: 3_566_562,
    layout: 0x2246_9480,
    // TextureType, Width, Height
    columns: &[2, 3, 4],
};
const SECTION: Source = Source {
    name: "CharComponentTextureSections",
    fdid: 1_360_263,
    layout: 0x2173_BA71,
    // SectionType, X, Y, Width, Height
    columns: &[1, 2, 3, 4, 5],
};
const MODEL_DATA: Source = Source {
    name: "CreatureModelData",
    fdid: super::creature_skins::MODEL_DATA_FDID,
    layout: 0x0F54_49F8,
    // FileDataID
    columns: &[2],
};
const DISPLAY_INFO: Source = Source {
    name: "CreatureDisplayInfo",
    fdid: super::creature_skins::DISPLAY_INFO_FDID,
    layout: 0x7275_F5F6,
    // ModelID
    columns: &[1],
};

impl Source {
    /// This table's rows, as `(id, parent, the columns asked for)`.
    fn rows(&self, fetch: &dyn Fn(u32) -> Option<Vec<u8>>) -> Result<Vec<Row>, String> {
        let name = self.name;
        let bytes = fetch(self.fdid).ok_or_else(|| format!("{name} could not be read"))?;
        let table =
            super::db2::Table::parse(&bytes, self.columns).map_err(|e| format!("{name}: {e}"))?;
        if table.layout_hash != self.layout {
            return Err(format!(
                "{name} has a layout this version does not know ({:08X})",
                table.layout_hash
            ));
        }
        Ok(table
            .rows
            .into_iter()
            .map(|r| Row {
                id: r.id,
                parent: r.parent,
                values: r
                    .fields
                    .iter()
                    .map(|f| f.first().copied().unwrap_or(0) as u32)
                    .collect(),
            })
            .collect())
    }
}

struct Row {
    id: u32,
    parent: u32,
    values: Vec<u32>,
}

impl Data {
    /// Read the character tables.  `fetch` opens a file of the archive by
    /// FileDataID.  The error names the table that could not be used.
    pub fn load(fetch: &dyn Fn(u32) -> Option<Vec<u8>>) -> Result<Self, String> {
        let ordered = |source: &Source| -> Result<Vec<Ordered>, String> {
            Ok(source
                .rows(fetch)?
                .iter()
                .map(|r| Ordered {
                    id: r.id,
                    owner: r.values[0],
                    order: r.values[1],
                })
                .collect())
        };
        let pairs = |source: &Source| -> Result<HashMap<u32, (u32, u32)>, String> {
            Ok(source
                .rows(fetch)?
                .iter()
                .map(|r| (r.id, (r.values[0], r.values[1])))
                .collect())
        };

        let model_files: HashMap<u32, u32> = MODEL_DATA
            .rows(fetch)?
            .iter()
            .map(|r| (r.id, r.values[0]))
            .collect();
        let display_files = DISPLAY_INFO
            .rows(fetch)?
            .iter()
            .filter_map(|r| Some((r.id, *model_files.get(&r.values[0])?)))
            .collect();
        // A resource can have several files; the plain one has usage 0.
        let mut texture_files = HashMap::new();
        let mut files = TEXTURE_FILE.rows(fetch)?;
        files.sort_by_key(|r| (r.values[0], r.id));
        for r in &files {
            texture_files.entry(r.parent).or_insert(r.id);
        }

        Ok(Self {
            models: CHR_MODEL
                .rows(fetch)?
                .iter()
                .map(|r| Model {
                    id: r.id,
                    display: r.values[0],
                    layout: r.values[1],
                })
                .collect(),
            options: ordered(&OPTION)?,
            choices: ordered(&CHOICE)?,
            elements: ELEMENT
                .rows(fetch)?
                .iter()
                .map(|r| Element {
                    choice: r.values[0],
                    related: r.values[1],
                    geoset: r.values[2],
                    material: r.values[3],
                })
                .collect(),
            materials: pairs(&MATERIAL)?,
            geosets: pairs(&GEOSET)?,
            texture_files,
            layers: LAYER
                .rows(fetch)?
                .iter()
                .map(|r| Layer {
                    layout: r.parent,
                    texture_type: r.values[0],
                    order: r.values[1],
                    blend: r.values[2],
                    sections: r.values[3],
                    target: r.values[4],
                })
                .collect(),
            sheets: SHEET
                .rows(fetch)?
                .iter()
                .map(|r| Sheet {
                    layout: r.parent,
                    texture_type: r.values[0],
                    width: r.values[1],
                    height: r.values[2],
                })
                .collect(),
            sections: SECTION
                .rows(fetch)?
                .iter()
                .map(|r| Section {
                    layout: r.parent,
                    kind: r.values[0],
                    rect: [r.values[1], r.values[2], r.values[3], r.values[4]],
                })
                .collect(),
            display_files,
        })
    }
}

/// The tables of one open archive, read the first time a model asks.  An
/// error is remembered too, so a game without them is asked once.
pub type Cache = std::cell::OnceCell<Result<Data, String>>;

/// The default look of the character whose body is `model_file`, reading
/// the tables into `cache` on first use.  `Ok(None)` for a file that is no
/// character body; `Err` says which table could not be used.
pub fn lookup(
    cache: &Cache,
    handler: Option<&rustydemon_lib::CascHandler>,
    model_file: Option<u32>,
) -> Result<Option<Look>, String> {
    let (Some(handler), Some(model_file)) = (handler, model_file) else {
        return Ok(None);
    };
    let data = cache.get_or_init(|| {
        // Rows for unreleased content are encrypted; take the rest.
        Data::load(&|fdid| handler.open_file_by_fdid_partial(fdid).ok().map(|f| f.data))
    });
    match data {
        Ok(data) => Ok(data.look(model_file)),
        Err(e) => Err(e.clone()),
    }
}

/// A decoded texture: RGBA bytes, width, height.
pub type Image = (Vec<u8>, u32, u32);

/// Paint a sheet.  `fetch` decodes a texture file; a layer whose file cannot
/// be had is left out.  `None` when nothing could be painted at all.
pub fn compose(plan: &SheetPlan, fetch: &dyn Fn(u32) -> Option<Image>) -> Option<Image> {
    let (w, h) = (plan.width as usize, plan.height as usize);
    let mut sheet = vec![0u8; w.checked_mul(h)?.checked_mul(4)?];
    let mut painted = false;
    for paint in &plan.paints {
        let Some((pixels, pw, ph)) = fetch(paint.file) else {
            continue;
        };
        let (pw, ph) = (pw as usize, ph as usize);
        if pw == 0 || ph == 0 || pixels.len() < pw * ph * 4 {
            continue;
        }
        let [x0, y0, rw, rh] = paint.rect.map(|v| v as usize);
        for y in 0..rh.min(h.saturating_sub(y0)) {
            for x in 0..rw.min(w.saturating_sub(x0)) {
                // Stretch the texture over its rectangle.
                let s = ((y * ph / rh) * pw + x * pw / rw) * 4;
                let d = ((y0 + y) * w + x0 + x) * 4;
                let (src, dst) = (&pixels[s..s + 4], &mut sheet[d..d + 4]);
                mix(dst, src, paint.blend);
            }
        }
        painted = true;
    }
    painted.then_some((sheet, plan.width, plan.height))
}

/// Mix one texel into the sheet.  Modes: 1 replaces; 4 multiplies; 6 is
/// overlay; 7 is screen; the rest lay the texel over what is there by its
/// alpha.
fn mix(dst: &mut [u8], src: &[u8], blend: u32) {
    if blend == 1 {
        dst.copy_from_slice(src);
        return;
    }
    let alpha = u32::from(src[3]);
    for c in 0..3 {
        let (d, s) = (u32::from(dst[c]), u32::from(src[c]));
        let mixed = match blend {
            4 => d * s / 255,
            6 if d < 128 => 2 * d * s / 255,
            6 => 255 - 2 * (255 - d) * (255 - s) / 255,
            7 => 255 - (255 - d) * (255 - s) / 255,
            _ => s,
        };
        dst[c] = ((mixed * alpha + d * (255 - alpha)) / 255) as u8;
    }
    dst[3] = dst[3].max(src[3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY_FILE: u32 = 9000;
    const SKIN: u32 = 1;
    const FACE: u32 = 5;
    const HAIR: u32 = 10;

    /// One body (model 7, file 9000) with three options:
    ///
    /// * skin colour: pale (choice 10) or dark (11)
    /// * face: one face (20), with a texture per skin colour
    /// * hair style: long (30) or short (31), each a model part
    fn data() -> Data {
        let order = |id, owner, order| Ordered { id, owner, order };
        let element = |choice, related, geoset, material| Element {
            choice,
            related,
            geoset,
            material,
        };
        let layer = |texture_type, order, blend, sections, target| Layer {
            layout: 3,
            texture_type,
            order,
            blend,
            sections,
            target,
        };
        Data {
            models: vec![Model {
                id: 7,
                display: 70,
                layout: 3,
            }],
            options: vec![
                order(1, 7, 0),
                order(2, 7, 1),
                order(3, 7, 2),
                order(4, 8, 0),
                order(5, 7, 3),
            ],
            choices: vec![
                // Listed out of order: position decides, not row order.
                order(11, 1, 1),
                order(10, 1, 0),
                order(20, 2, 0),
                order(31, 3, 1),
                order(30, 3, 0),
                // Another model's option.
                order(40, 4, 0),
                // Necklace: none (no effects at all), or a pendant.
                order(50, 5, 0),
                order(51, 5, 1),
            ],
            elements: vec![
                element(10, 0, 0, 100),
                element(11, 0, 0, 101),
                element(20, 11, 0, 111),
                element(20, 10, 0, 110),
                element(30, 0, 500, 0),
                element(31, 0, 501, 0),
                element(30, 0, 0, 120),
                element(40, 0, 502, 130),
                element(51, 0, 503, 0),
            ],
            materials: HashMap::from([
                (100, (SKIN, 1000)),
                (101, (SKIN, 1001)),
                (110, (FACE, 1010)),
                (111, (FACE, 1011)),
                (120, (HAIR, 1020)),
                (130, (SKIN, 1030)),
            ]),
            geosets: HashMap::from([(500, (0, 2)), (501, (0, 3)), (502, (7, 2)), (503, (33, 2))]),
            texture_files: HashMap::from([
                (1000, 50),
                (1001, 51),
                (1010, 60),
                (1011, 61),
                (1020, 80),
                (1030, 99),
            ]),
            layers: vec![
                // Face before skin in the table: order decides.
                layer(1, 3, 15, 1 << 10, FACE),
                layer(1, 0, 1, WHOLE_SHEET, SKIN),
                layer(6, 1, 1, WHOLE_SHEET, HAIR),
                // Nothing chosen supplies this kind of texture.
                layer(1, 9, 15, 1 << 10, 77),
                Layer {
                    layout: 4,
                    ..layer(1, 0, 1, WHOLE_SHEET, SKIN)
                },
            ],
            sheets: vec![
                Sheet {
                    layout: 3,
                    texture_type: 1,
                    width: 8,
                    height: 4,
                },
                Sheet {
                    layout: 3,
                    texture_type: 6,
                    width: 2,
                    height: 2,
                },
                // A sheet nothing paints on.
                Sheet {
                    layout: 3,
                    texture_type: 19,
                    width: 2,
                    height: 2,
                },
            ],
            sections: vec![
                Section {
                    layout: 3,
                    kind: 10,
                    rect: [4, 0, 4, 2],
                },
                Section {
                    layout: 4,
                    kind: 10,
                    rect: [0, 0, 1, 1],
                },
            ],
            display_files: HashMap::from([(70, BODY_FILE)]),
        }
    }

    #[test]
    fn a_table_that_cannot_be_read_is_named() {
        let err = Data::load(&|_| None).unwrap_err();
        assert!(err.contains("CreatureModelData"), "{err}");
    }

    #[test]
    fn a_table_with_another_layout_is_refused_by_name_and_hash() {
        use super::super::db2::tests::{file, HAS_ID_LIST};
        // A valid table, but not one of this build's.
        let other = file(HAS_ID_LIST, &[]);
        let err = Data::load(&|_| Some(other.clone())).unwrap_err();
        assert!(err.contains("CreatureModelData"), "{err}");
        assert!(err.contains("ABCD0123"), "{err}");
    }

    #[test]
    fn a_file_no_character_uses_has_no_look() {
        assert_eq!(data().look(1234), None);
    }

    #[test]
    fn the_first_choice_of_each_option_is_taken() {
        let look = data().look(BODY_FILE).unwrap();
        // Long hair (part 0/2), not short, and not the other model's ears.
        assert!(look.geosets.contains(&(0, 2)), "{:?}", look.geosets);
        assert!(!look.geosets.iter().any(|g| g.0 == 7), "{:?}", look.geosets);
    }

    /// A necklace is its own group of parts.  Choosing "none" has no row
    /// saying so; the group must still be emptied, or the model's first
    /// necklace would show.
    #[test]
    fn a_group_an_option_controls_is_emptied_when_the_choice_picks_nothing() {
        let look = data().look(BODY_FILE).unwrap();
        assert_eq!(look.geosets, [(0, 2), (33, 0)]);
    }

    #[test]
    fn the_body_sheet_is_painted_skin_first_then_face_in_its_rectangle() {
        let look = data().look(BODY_FILE).unwrap();
        let body = look.sheets.iter().find(|s| s.texture_type == 1).unwrap();
        assert_eq!((body.width, body.height), (8, 4));
        assert_eq!(
            body.paints,
            [
                Paint {
                    file: 50,
                    rect: [0, 0, 8, 4],
                    blend: 1
                },
                // The face made for pale skin, not the one for dark.
                Paint {
                    file: 60,
                    rect: [4, 0, 4, 2],
                    blend: 15
                },
            ]
        );
    }

    #[test]
    fn each_kind_of_texture_gets_its_own_sheet_and_empty_ones_are_left_out() {
        let look = data().look(BODY_FILE).unwrap();
        let kinds: Vec<u32> = look.sheets.iter().map(|s| s.texture_type).collect();
        assert_eq!(kinds, [1, 6]);
        assert_eq!(look.sheets[1].paints[0].file, 80);
    }

    #[test]
    fn a_layer_whose_rectangle_the_layout_lacks_is_left_out() {
        let mut d = data();
        d.sections.clear();
        let look = d.look(BODY_FILE).unwrap();
        assert_eq!(look.sheets[0].paints.len(), 1, "skin only");
    }

    fn solid(rgba: [u8; 4], w: u32, h: u32) -> Image {
        (rgba.repeat((w * h) as usize), w, h)
    }

    fn texel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.1 + x) * 4) as usize;
        image.0[at..at + 4].try_into().unwrap()
    }

    fn plan(paints: Vec<Paint>) -> SheetPlan {
        SheetPlan {
            texture_type: 1,
            width: 4,
            height: 2,
            paints,
        }
    }

    #[test]
    fn a_layer_is_painted_into_its_rectangle_only() {
        let whole = Paint {
            file: 1,
            rect: [0, 0, 4, 2],
            blend: 1,
        };
        let corner = Paint {
            file: 2,
            rect: [2, 0, 2, 1],
            blend: 15,
        };
        let fetch = |file| match file {
            1 => Some(solid([10, 20, 30, 255], 4, 2)),
            _ => Some(solid([200, 0, 0, 255], 1, 1)),
        };
        let sheet = compose(&plan(vec![whole, corner]), &fetch).unwrap();
        assert_eq!((sheet.1, sheet.2), (4, 2));
        assert_eq!(texel(&sheet, 0, 0), [10, 20, 30, 255]);
        assert_eq!(texel(&sheet, 2, 0), [200, 0, 0, 255]);
        assert_eq!(texel(&sheet, 3, 0), [200, 0, 0, 255]);
        assert_eq!(
            texel(&sheet, 2, 1),
            [10, 20, 30, 255],
            "below the rectangle"
        );
    }

    #[test]
    fn a_texture_is_stretched_over_its_rectangle() {
        // Left half red, right half blue, two texels wide.
        let two = (vec![255, 0, 0, 255, 0, 0, 255, 255], 2, 1);
        let paint = Paint {
            file: 1,
            rect: [0, 0, 4, 2],
            blend: 1,
        };
        let sheet = compose(&plan(vec![paint]), &|_| Some(two.clone())).unwrap();
        assert_eq!(texel(&sheet, 1, 1), [255, 0, 0, 255]);
        assert_eq!(texel(&sheet, 2, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn a_see_through_layer_lets_the_skin_show() {
        let paints = vec![
            Paint {
                file: 1,
                rect: [0, 0, 4, 2],
                blend: 1,
            },
            Paint {
                file: 2,
                rect: [0, 0, 4, 2],
                blend: 15,
            },
        ];
        let fetch = |file| match file {
            1 => Some(solid([100, 100, 100, 255], 1, 1)),
            _ => Some(solid([255, 255, 255, 0], 1, 1)),
        };
        let sheet = compose(&plan(paints), &fetch).unwrap();
        assert_eq!(texel(&sheet, 0, 0), [100, 100, 100, 255]);
    }

    #[test]
    fn a_multiply_layer_darkens() {
        let paints = vec![
            Paint {
                file: 1,
                rect: [0, 0, 4, 2],
                blend: 1,
            },
            Paint {
                file: 2,
                rect: [0, 0, 4, 2],
                blend: 4,
            },
        ];
        let fetch = |file| match file {
            1 => Some(solid([200, 100, 50, 255], 1, 1)),
            _ => Some(solid([127, 255, 0, 255], 1, 1)),
        };
        let sheet = compose(&plan(paints), &fetch).unwrap();
        assert_eq!(texel(&sheet, 0, 0), [99, 100, 0, 255]);
    }

    #[test]
    fn a_missing_texture_is_skipped_and_nothing_at_all_is_none() {
        let paints = vec![
            Paint {
                file: 1,
                rect: [0, 0, 4, 2],
                blend: 1,
            },
            Paint {
                file: 2,
                rect: [0, 0, 4, 2],
                blend: 15,
            },
        ];
        let only_skin = |file| (file == 1).then(|| solid([9, 9, 9, 255], 1, 1));
        let sheet = compose(&plan(paints.clone()), &only_skin).unwrap();
        assert_eq!(texel(&sheet, 3, 1), [9, 9, 9, 255]);
        assert_eq!(compose(&plan(paints), &|_| None), None);
    }

    #[test]
    fn a_rectangle_hanging_off_the_sheet_and_a_short_texture_do_not_panic() {
        let off = Paint {
            file: 1,
            rect: [3, 1, 40, 40],
            blend: 1,
        };
        let sheet = compose(&plan(vec![off]), &|_| Some(solid([1, 2, 3, 255], 1, 1))).unwrap();
        assert_eq!(texel(&sheet, 3, 1), [1, 2, 3, 255]);
        assert_eq!(texel(&sheet, 2, 1), [0, 0, 0, 0]);
        // Claims 4x4 but holds one texel.
        let short = (vec![1, 2, 3, 255], 4, 4);
        assert_eq!(compose(&plan(vec![off]), &|_| Some(short.clone())), None);
    }
}
