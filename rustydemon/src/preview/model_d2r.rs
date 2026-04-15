//! D2R `.model` preview plugin.
//!
//! Uses the pure-Rust [`rustydemon_gr2`] reader to parse the whole
//! Granny3D file and show a structural summary — source file,
//! textures, mesh / bone / animation counts, top-level element tree.
//! The same parser is what a future 3D viewport for Granny meshes
//! will build on, so this plugin also serves as the integration
//! point where the reader first hits real end-user data.

use super::{PreviewOutput, PreviewPlugin};
use rustydemon_gr2::{has_granny_magic, Element, ElementValue, GrannyFile};

pub struct ModelD2rPreview;

impl PreviewPlugin for ModelD2rPreview {
    fn name(&self) -> &str {
        ".model (D2R / Granny3D)"
    }

    fn can_preview(&self, _filename: &str, data: &[u8]) -> bool {
        has_granny_magic(data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let gf = match GrannyFile::from_bytes(data) {
            Ok(gf) => gf,
            Err(e) => {
                out.text = Some(format!(
                    "Granny3D file recognised (magic matched), but parsing failed:\n  {e}\n\n\
                     Bitknit2 decompression is implemented in-tree via rustydemon-gr2.\n\
                     Please report any file this fails on."
                ));
                return out;
            }
        };

        let summary = gf.summary();
        let mut text = String::new();
        text.push_str("D2R .model  (Granny3D, pure-Rust reader)\n\n");

        // Header & section counts.
        text.push_str(&format!(
            "Format            {} ({}-bit {:?})\n",
            gf.header.format,
            if gf.header.bits_64 { 64 } else { 32 },
            gf.header.endian
        ));
        text.push_str(&format!(
            "File size         {} bytes\n",
            gf.file_info.total_size
        ));
        text.push_str(&format!("CRC-32            0x{:08X}\n", gf.file_info.crc32));
        text.push_str(&format!("Tag               0x{:08X}\n", gf.file_info.tag));
        text.push_str(&format!("Sections          {}\n\n", summary.section_count));

        // High-level content summary.
        text.push_str("Contents\n");
        text.push_str(&format!(
            "  Models          {}\n  Meshes          {}\n  Skeletons       {}\n  \
             Animations      {}\n  Textures        {}\n\n",
            summary.models, summary.meshes, summary.skeletons, summary.animations, summary.textures
        ));

        // Source file + art tool info — pulled straight out of the
        // parsed tree.  Granny puts these at the top level.
        if let Some(name) = find_string(&gf.root_elements, "FromFileName") {
            text.push_str(&format!("Source file       {name}\n"));
        }
        if let Some(art_tool) = gf.find("ArtToolInfo") {
            if let ElementValue::Reference(children) = &art_tool.value {
                if let Some(name) = find_string(children, "FromArtToolName") {
                    text.push_str(&format!("Authored in       {name}\n"));
                }
                if let Some(units) = find_f32(children, "UnitsPerMeter") {
                    text.push_str(&format!("Units             {units:.4} per metre\n"));
                }
            }
        }
        text.push('\n');

        // Texture names — pulled from the Textures array's inner
        // FromFileName fields.  Shows users which .texture files they'd
        // need to extract alongside this model.
        if let Some(textures) = gf.find("Textures") {
            if let ElementValue::ArrayOfReferences(groups) = &textures.value {
                if !groups.is_empty() {
                    text.push_str(&format!("Textures ({}):\n", groups.len()));
                    for g in groups {
                        if let Some(name) = find_string(g, "FromFileName") {
                            text.push_str(&format!("  {name}\n"));
                        }
                    }
                    text.push('\n');
                }
            }
        }

        // Top-level element tree — one level deep.  Users can see the
        // skeleton name, mesh name, etc. without us having to interpret
        // the geometry.
        text.push_str("Top-level tree:\n");
        for e in &gf.root_elements {
            text.push_str(&format!("  - {} :: {}\n", e.name, kind_label(&e.value)));
        }

        text.push_str(
            "\nGeometry extraction for the 3D viewport is the next step — \
             the parser already exposes the mesh trees, we just need a\n\
             plugin that walks VertexData + TriTopology and fills a Mesh3dCpu.\n",
        );

        out.text = Some(text);
        out
    }
}

fn find_string(elements: &[Element], name: &str) -> Option<String> {
    elements.iter().find_map(|e| {
        if e.name == name {
            if let ElementValue::String(s) = &e.value {
                return Some(s.clone());
            }
        }
        None
    })
}

fn find_f32(elements: &[Element], name: &str) -> Option<f32> {
    elements.iter().find_map(|e| {
        if e.name == name {
            if let ElementValue::F32(v) = &e.value {
                return Some(*v);
            }
        }
        None
    })
}

fn kind_label(v: &ElementValue) -> String {
    match v {
        ElementValue::Reference(c) => format!("Reference ({} children)", c.len()),
        ElementValue::ReferenceArray(g) => format!("ReferenceArray ({} entries)", g.len()),
        ElementValue::ArrayOfReferences(g) => format!("ArrayOfReferences ({} entries)", g.len()),
        ElementValue::String(s) => format!("String {s:?}"),
        ElementValue::Transform(_) => "Transform".into(),
        ElementValue::F32(v) => format!("f32 {v}"),
        ElementValue::I32(v) => format!("i32 {v}"),
        ElementValue::Opaque(id) => format!("Opaque(type={id})"),
        ElementValue::Array(v) => format!("Array ({} entries)", v.len()),
    }
}
