//! Which textures a creature model wears.
//!
//! A creature's `.m2` leaves its main texture slots empty ("monster skin
//! 1/2/3"): the same model is a red basilisk in one zone and a jade one in
//! another.  The game picks the skin from two database tables:
//!
//! * `CreatureModelData`: model id -> the `.m2` file
//! * `CreatureDisplayInfo`: display id -> model id + up to three skin
//!   textures
//!
//! The preview shows the skin of the model's lowest-numbered display.

use std::collections::HashMap;

use super::db2::Table;

/// FileDataIDs of the two tables.
pub const MODEL_DATA_FDID: u32 = 1_365_368;
pub const DISPLAY_INFO_FDID: u32 = 1_108_759;

/// Where the columns are.  Tables carry no column names, so each known
/// layout is listed here by its hash; an unknown layout is refused rather
/// than guessed at.
#[derive(Clone, Copy)]
struct Columns {
    model_data_layout: u32,
    /// `CreatureModelData.FileDataID`
    model_file: usize,
    display_info_layout: u32,
    /// `CreatureDisplayInfo.ModelID`
    display_model: usize,
    /// `CreatureDisplayInfo.TextureVariationFileDataID[]`
    display_textures: usize,
}

const KNOWN: &[Columns] = &[
    // WoW Forever (wowf), build 1.60.1.70399.
    Columns {
        model_data_layout: 0x0F54_49F8,
        model_file: 2,
        display_info_layout: 0x7275_F5F6,
        display_model: 1,
        display_textures: 27,
    },
];

/// Model file -> its monster skin textures (0 for none).
#[derive(Debug, Default)]
pub struct CreatureSkins {
    by_model_file: HashMap<u32, [u32; 3]>,
}

impl CreatureSkins {
    /// Build the lookup from the raw bytes of the two tables.
    pub fn build(model_data: &[u8], display_info: &[u8]) -> Result<Self, String> {
        let models = Table::parse(model_data, &[])?.layout_hash;
        let displays = Table::parse(display_info, &[])?.layout_hash;
        let columns = KNOWN
            .iter()
            .find(|c| c.model_data_layout == models && c.display_info_layout == displays)
            .ok_or_else(|| {
                format!(
                    "this game build's creature tables have a layout this version \
                     does not know (CreatureModelData {models:08X}, \
                     CreatureDisplayInfo {displays:08X})"
                )
            })?;
        Self::build_with(model_data, display_info, *columns)
    }

    fn build_with(model_data: &[u8], display_info: &[u8], c: Columns) -> Result<Self, String> {
        let first = |row: &super::db2::Row| row.fields[0].first().copied().unwrap_or(0) as u32;
        let file_of_model: HashMap<u32, u32> = Table::parse(model_data, &[c.model_file])?
            .rows
            .iter()
            .map(|row| (row.id, first(row)))
            .collect();

        let mut displays = Table::parse(display_info, &[c.display_model, c.display_textures])?.rows;
        displays.sort_by_key(|row| row.id);

        let mut by_model_file = HashMap::new();
        for row in &displays {
            let mut skins = [0u32; 3];
            for (slot, &texture) in skins.iter_mut().zip(&row.fields[1]) {
                *slot = texture as u32;
            }
            let Some(&file) = file_of_model.get(&first(row)) else {
                continue;
            };
            if file != 0 && skins != [0; 3] {
                // Sorted by display id, so the first one seen is the lowest.
                by_model_file.entry(file).or_insert(skins);
            }
        }
        Ok(Self { by_model_file })
    }

    /// Skins for the model whose `.m2` is FileDataID `model_file`.
    pub fn for_model(&self, model_file: u32) -> Option<[u32; 3]> {
        self.by_model_file.get(&model_file).copied()
    }
}

/// The lookup for one open archive, built the first time it is needed.
/// `None` inside means the tables could not be read; that is remembered too,
/// so a game without them is not asked again on every click.
pub type Cache = std::cell::OnceCell<Option<CreatureSkins>>;

/// Skins for `model_file` in `handler`'s archive, loading the tables into
/// `cache` on first use.
pub fn lookup(
    cache: &Cache,
    handler: Option<&rustydemon_lib::CascHandler>,
    model_file: Option<u32>,
) -> Option<[u32; 3]> {
    let (handler, model_file) = (handler?, model_file?);
    cache
        .get_or_init(|| {
            // Rows for unreleased content are encrypted; take the rest.
            let table = |fdid| handler.open_file_by_fdid_partial(fdid).ok().map(|f| f.data);
            CreatureSkins::build(&table(MODEL_DATA_FDID)?, &table(DISPLAY_INFO_FDID)?).ok()
        })
        .as_ref()?
        .for_model(model_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::db2::tests::{file, record, Section, HAS_ID_LIST};

    /// The test table's columns stand in for the real ones: column 0 is
    /// the model's file, column 1 the display's model, column 3 its skins
    /// (`[1, 2]`, `[3, 4]` or `[0, 0]` for pallet index 0, 1, 2).
    const TEST: Columns = Columns {
        model_data_layout: 0xABCD_0123,
        model_file: 0,
        display_info_layout: 0xABCD_0123,
        display_model: 1,
        display_textures: 3,
    };

    fn table(rows: &[(u32, Vec<u8>)]) -> Vec<u8> {
        file(
            HAS_ID_LIST,
            &[Section {
                key: 0,
                records: rows.iter().map(|(_, r)| r.clone()).collect(),
                ids: rows.iter().map(|(id, _)| *id).collect(),
                copies: vec![],
            }],
        )
    }

    /// Model 10 is file 5000, model 11 is file 6000.
    fn models() -> Vec<u8> {
        table(&[
            (10, record(5000, 0, 0, 0, 0)),
            (11, record(6000, 0, 0, 0, 0)),
        ])
    }

    fn display(model: u64, skins: u64) -> Vec<u8> {
        record(0, model, 0, skins, 0)
    }

    #[test]
    fn finds_a_models_skins_through_its_display() {
        let displays = table(&[(7, display(10, 1))]);
        let s = CreatureSkins::build_with(&models(), &displays, TEST).unwrap();
        assert_eq!(s.for_model(5000), Some([3, 4, 0]));
        assert_eq!(s.for_model(6000), None, "no display uses model 11");
        assert_eq!(s.for_model(1), None);
    }

    #[test]
    fn the_lowest_numbered_display_with_a_skin_wins() {
        let displays = table(&[
            (9, display(10, 1)),
            (5, display(10, 0)),
            (2, display(10, 2)), // no textures at all: not a skin
        ]);
        let s = CreatureSkins::build_with(&models(), &displays, TEST).unwrap();
        assert_eq!(s.for_model(5000), Some([1, 2, 0]));
    }

    #[test]
    fn a_display_of_an_unknown_model_is_ignored() {
        let displays = table(&[(7, display(99, 1))]);
        let s = CreatureSkins::build_with(&models(), &displays, TEST).unwrap();
        assert_eq!(s.for_model(5000), None);
    }

    #[test]
    fn tables_with_an_unknown_layout_are_refused_by_hash() {
        let err = CreatureSkins::build(&models(), &models()).unwrap_err();
        assert!(err.to_lowercase().contains("abcd0123"), "{err}");
    }

    #[test]
    fn a_damaged_table_is_an_error() {
        assert!(CreatureSkins::build_with(&models()[..120], &models(), TEST).is_err());
    }
}
