//! Top-level Granny file object and high-level accessors.

use crate::element::{parse_elements, Element, ElementValue};
use crate::error::Result;
use crate::file_info::{parse_file_info, FileInfo};
use crate::header::{parse_header, Header};
use crate::section::{load_section, parse_section_info, Section};

/// A fully-loaded Granny3D file: header, file info, decompressed
/// sections, parsed element tree.
pub struct GrannyFile {
    pub header: Header,
    pub file_info: FileInfo,
    pub sections: Vec<Section>,
    pub root_elements: Vec<Element>,
}

impl GrannyFile {
    /// Parse an in-memory Granny file start-to-end.
    ///
    /// - Decompresses every sector (Bitknit2 or None — returns an
    ///   [`UnsupportedCompression`](crate::GrannyError::UnsupportedCompression)
    ///   error for sectors using Oodle/Bitknit1, which D2R doesn't use).
    /// - Applies no in-place relocations; pointer fixups are kept as a
    ///   side table and resolved lazily by the element walker.
    /// - Walks the type/data tree from `root_ref` and collects a
    ///   [`Vec<Element>`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (header, mut cursor) = parse_header(bytes)?;
        let (file_info, next) = parse_file_info(bytes, cursor, header.endian)?;
        cursor = next;

        let mut section_infos = Vec::with_capacity(file_info.sector_count as usize);
        for _ in 0..file_info.sector_count {
            let (info, next) = parse_section_info(bytes, cursor, header.endian)?;
            section_infos.push(info);
            cursor = next;
        }
        let _ = cursor;

        let mut sections = Vec::with_capacity(section_infos.len());
        for info in section_infos {
            sections.push(load_section(bytes, info, header.endian)?);
        }

        let root_elements = parse_elements(
            &sections,
            header.endian,
            header.bits_64,
            file_info.root_ref.sector,
            file_info.type_ref.sector,
            file_info.root_ref.position,
            file_info.type_ref.position,
        )?;

        Ok(GrannyFile {
            header,
            file_info,
            sections,
            root_elements,
        })
    }

    /// Find the first top-level element with the given name.  Useful
    /// for picking known fields like `"Models"` or `"Meshes"` out of
    /// the root tree without writing dotted-path lookups.
    pub fn find(&self, name: &str) -> Option<&Element> {
        self.root_elements.iter().find(|e| e.name == name)
    }

    /// Texture filenames from the top-level `Textures` array, in order.
    /// Each entry is the `FromFileName` string of the corresponding
    /// texture element.  Returns an empty vec if the file has no
    /// `Textures` array.
    pub fn texture_filenames(&self) -> Vec<String> {
        let Some(textures) = self.find("Textures") else {
            return Vec::new();
        };
        let groups = match &textures.value {
            ElementValue::ArrayOfReferences(g) | ElementValue::ReferenceArray(g) => g,
            _ => return Vec::new(),
        };
        groups
            .iter()
            .filter_map(|g| {
                g.iter().find_map(|e| {
                    if e.name == "FromFileName" {
                        if let ElementValue::String(s) = &e.value {
                            return Some(s.clone());
                        }
                    }
                    None
                })
            })
            .collect()
    }

    /// Shallow structural summary used by the preview plugin.
    pub fn summary(&self) -> GrannySummary {
        let mut meshes = 0usize;
        let mut models = 0usize;
        let mut animations = 0usize;
        let mut textures = 0usize;
        let mut skeletons = 0usize;
        count_by_name(&self.root_elements, &mut |name, val| {
            let target = match name {
                "Meshes" => Some(&mut meshes),
                "Models" => Some(&mut models),
                "Animations" => Some(&mut animations),
                "Textures" => Some(&mut textures),
                "Skeletons" => Some(&mut skeletons),
                _ => None,
            };
            if let Some(target) = target {
                match val {
                    ElementValue::ReferenceArray(v) | ElementValue::ArrayOfReferences(v) => {
                        *target += v.len();
                    }
                    ElementValue::Array(v) => *target += v.len(),
                    _ => {}
                }
            }
        });
        GrannySummary {
            section_count: self.sections.len(),
            compressed_size: self.file_info.total_size as usize,
            decompressed_size: self
                .sections
                .iter()
                .map(|s| s.info.decompressed_length as usize)
                .sum(),
            models,
            meshes,
            animations,
            textures,
            skeletons,
        }
    }
}

/// Compact structural summary — what the preview panel shows users
/// before any geometry rendering happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrannySummary {
    pub section_count: usize,
    pub compressed_size: usize,
    pub decompressed_size: usize,
    pub models: usize,
    pub meshes: usize,
    pub animations: usize,
    pub textures: usize,
    pub skeletons: usize,
}

fn count_by_name<F: FnMut(&str, &ElementValue)>(elements: &[Element], f: &mut F) {
    for e in elements {
        f(&e.name, &e.value);
        match &e.value {
            ElementValue::Reference(children) => count_by_name(children, f),
            ElementValue::ReferenceArray(groups) | ElementValue::ArrayOfReferences(groups) => {
                for g in groups {
                    count_by_name(g, f);
                }
            }
            _ => {}
        }
    }
}
