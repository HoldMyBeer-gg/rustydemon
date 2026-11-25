//! The game's character bodies, by race and sex.
//!
//! A body is an `.m2` known only by its file number.  The tables say which
//! race and sex each one is:
//!
//! * `ChrRaces`: race -> its name
//! * `ChrRaceXChrModel`: race -> its character models
//! * `ChrModel`: character model -> sex and body file (see
//!   [`character`](super::character))
//!
//! This lists the bodies by name and finds them by what the user has typed
//! so far.

use super::character::{Data, Source};
use super::db2::Table;

/// One character body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// The race's name, as the game shows it.
    pub race: String,
    /// `ChrModel.Sex`: 0 male, 1 female.
    pub sex: u32,
    /// The body's `.m2`.
    pub file: u32,
}

impl Body {
    /// How the body is listed: "Skyborne, female".
    pub fn label(&self) -> String {
        let sex = match self.sex {
            0 => "male",
            1 => "female",
            _ => "other",
        };
        format!("{}, {sex}", self.race)
    }
}

/// Every body some named race uses, by label.
///
/// `races` is `(race, name)`, `race_models` `(race, character model)` and
/// `models` `(character model, sex, body file)`.  Two races that share a
/// name and a body are listed once.
pub fn list(
    races: &[(u32, String)],
    race_models: &[(u32, u32)],
    models: &[(u32, u32, u32)],
) -> Vec<Body> {
    let mut bodies: Vec<Body> = race_models
        .iter()
        .filter_map(|&(race, model)| {
            let name = races.iter().find(|r| r.0 == race)?.1.trim();
            let &(_, sex, file) = models.iter().find(|m| m.0 == model)?;
            (!name.is_empty() && file != 0).then(|| Body {
                race: name.to_owned(),
                sex,
                file,
            })
        })
        .collect();
    bodies.sort_by_key(|b| (b.label(), b.file));
    bodies.dedup();
    bodies
}

/// The bodies to suggest for what has been typed, up to `limit`.  Each
/// word typed must start a word of the label, so "elf f" finds the female
/// elves and "male" does not find "female".  Labels that start with what
/// was typed come first.  Case is ignored.  Nothing typed suggests nothing.
pub fn suggest<'a>(bodies: &'a [Body], typed: &str, limit: usize) -> Vec<&'a Body> {
    let typed = typed.trim().to_lowercase();
    if typed.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = typed.split_whitespace().collect();
    let labelled: Vec<(&Body, String)> = bodies
        .iter()
        .map(|b| (b, b.label().to_lowercase().replace(',', "")))
        .filter(|(_, label)| {
            words
                .iter()
                .all(|w| label.split_whitespace().any(|l| l.starts_with(w)))
        })
        .collect();
    let (first, rest): (Vec<_>, Vec<_>) = labelled
        .iter()
        .partition(|(_, label)| label.starts_with(&typed));
    first
        .into_iter()
        .chain(rest)
        .map(|(body, _)| *body)
        .take(limit)
        .collect()
}

// WoW Forever (wowf), build 1.60.1.
const RACE: Source = Source {
    name: "ChrRaces",
    fdid: 1_305_311,
    layout: 0x4F44_C796,
    // Name_lang
    columns: &[2],
};
const RACE_MODEL: Source = Source {
    name: "ChrRaceXChrModel",
    fdid: 3_490_304,
    layout: 0xA203_BC29,
    // ChrModelID; the parent is the race.
    columns: &[1],
};

/// Read the bodies of the open archive.  `fetch` opens a file by
/// FileDataID; `data` is its character tables.  The error names the table
/// that could not be used.
pub fn load(fetch: &dyn Fn(u32) -> Option<Vec<u8>>, data: &Data) -> Result<Vec<Body>, String> {
    let bytes = fetch(RACE.fdid).ok_or("ChrRaces could not be read")?;
    let table = Table::parse_text(&bytes, RACE.columns, RACE.columns)
        .map_err(|e| format!("ChrRaces: {e}"))?;
    if table.layout_hash != RACE.layout {
        return Err(format!(
            "ChrRaces has a layout this version does not know ({:08X})",
            table.layout_hash
        ));
    }
    let races: Vec<(u32, String)> = table
        .rows
        .iter()
        .filter_map(|r| {
            let name = table.text(*r.fields[0].first()?)?;
            Some((r.id, name.to_owned()))
        })
        .collect();
    let race_models: Vec<(u32, u32)> = RACE_MODEL
        .table(fetch)?
        .rows
        .iter()
        .map(|r| (r.parent, r.fields[0].first().copied().unwrap_or(0) as u32))
        .collect();
    let models: Vec<(u32, u32, u32)> = data
        .models
        .iter()
        .filter_map(|m| Some((m.id, m.sex, *data.display_files.get(&m.display)?)))
        .collect();
    Ok(list(&races, &race_models, &models))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELF: u32 = 10;
    const SKY_ONE: u32 = 95;
    const SKY_TWO: u32 = 96;

    fn bodies() -> Vec<Body> {
        let races = [
            (ELF, "Blood Elf".to_string()),
            (SKY_ONE, "Skyborne".to_string()),
            (SKY_TWO, "Skyborne".to_string()),
            (50, String::new()),
        ];
        // The two Skyborne races share their bodies.
        let race_models = [
            (SKY_ONE, 218),
            (SKY_ONE, 219),
            (SKY_TWO, 218),
            (SKY_TWO, 219),
            (ELF, 20),
            (ELF, 19),
            (50, 19),
            (ELF, 999),
        ];
        let models = [
            (19, 0, 1000),
            (20, 1, 1001),
            (218, 0, 7000),
            (219, 1, 7001),
            // A model with no body file.
            (300, 0, 0),
        ];
        list(&races, &race_models, &models)
    }

    fn labels(found: &[&Body]) -> Vec<String> {
        found.iter().map(|b| b.label()).collect()
    }

    #[test]
    fn bodies_are_listed_by_race_and_sex_once_each() {
        let all = bodies();
        let labels: Vec<String> = all.iter().map(Body::label).collect();
        assert_eq!(
            labels,
            [
                "Blood Elf, female",
                "Blood Elf, male",
                "Skyborne, female",
                "Skyborne, male"
            ]
        );
        assert_eq!(all[2].file, 7001);
    }

    #[test]
    fn typing_the_start_of_a_race_finds_its_bodies() {
        let all = bodies();
        assert_eq!(
            labels(&suggest(&all, "sky", 8)),
            ["Skyborne, female", "Skyborne, male"]
        );
        assert_eq!(
            labels(&suggest(&all, "  SKYBORNE ", 1)),
            ["Skyborne, female"]
        );
        assert!(suggest(&all, "", 8).is_empty());
        assert!(suggest(&all, "orc", 8).is_empty());
    }

    #[test]
    fn a_second_word_narrows_to_the_sex() {
        let all = bodies();
        assert_eq!(labels(&suggest(&all, "sky f", 8)), ["Skyborne, female"]);
        // "male" is not the start of "female".
        assert_eq!(
            labels(&suggest(&all, "male", 8)),
            ["Blood Elf, male", "Skyborne, male"]
        );
    }

    #[test]
    fn a_word_inside_the_name_finds_it_after_names_that_start_with_it() {
        let all = bodies();
        assert_eq!(
            labels(&suggest(&all, "elf", 8)),
            ["Blood Elf, female", "Blood Elf, male"]
        );
    }

    #[test]
    fn a_race_table_that_cannot_be_read_is_named() {
        let err = load(&|_| None, &Data::default()).unwrap_err();
        assert!(err.contains("ChrRaces"), "{err}");
    }
}
