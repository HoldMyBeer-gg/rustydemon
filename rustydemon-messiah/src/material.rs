//! Messiah materials: a `.MESSIAH` object holding one parameter block per
//! sub-material, each a run of `key=value` lines such as
//! `tBaseMap=<texture guid>`. Sub-material n paints submesh n.

/// What the previews need from a material.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Material {
    pub name: Option<String>,
    /// Base colour texture GUID of each sub-material, `None` when unset.
    pub base_maps: Vec<Option<String>>,
}

const EMPTY_GUID: &str = "00000000-0000-0000-0000-000000000000";

impl Material {
    pub fn parse(data: &[u8]) -> Self {
        let mut m = Material::default();
        let text_byte = |b: u8| b == b'\n' || (0x20..0x7f).contains(&b);
        let mut i = 0;
        while i < data.len() {
            if !text_byte(data[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < data.len() && text_byte(data[i]) {
                i += 1;
            }
            let run = String::from_utf8_lossy(&data[start..i]);
            if run.contains('=') && run.contains('\n') {
                let base = run
                    .lines()
                    .find_map(|l| l.strip_prefix("tBaseMap="))
                    .map(str::trim)
                    .filter(|g| *g != EMPTY_GUID && !g.is_empty())
                    .map(str::to_owned);
                m.base_maps.push(base);
            } else if m.name.is_none() && run.ends_with("_mat") {
                m.name = Some(run.into_owned());
            }
        }
        m
    }
}
