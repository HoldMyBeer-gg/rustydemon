//! D2R `.model` preview plugin.
//!
//! Uses the pure-Rust [`rustydemon_gr2`] reader to parse Granny3D
//! files, extract mesh geometry into the 3D viewport, and resolve
//! sibling `.texture` files as materials.  Shows a structural summary
//! (source file, textures, mesh / bone / animation counts, element
//! tree) alongside the rendered preview.

use std::sync::Arc;

use super::{ExportAction, Mesh3dCpu, MeshBatch, MeshMaterial, PreviewOutput, PreviewPlugin};
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
        fetch: &super::SiblingFetcher<'_>,
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

        // Source file + art tool info: pulled straight out of the
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

        // Texture names: pulled from the Textures array's inner
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

        // Geometry: extract every mesh and feed the first one to the
        // 3D viewport.  Multi-mesh D2R models merge into a single
        // Mesh3dCpu with one batch per source mesh, so the whole thing
        // renders at once.
        let meshes = gf.meshes();
        if !meshes.is_empty() {
            text.push_str("\nDecoded meshes:\n");
            for m in &meshes {
                text.push_str(&format!(
                    "  '{}'  {} verts / {} tris\n",
                    m.name,
                    m.positions.len(),
                    m.indices.len() / 3
                ));
            }

            // One material per distinct colour texture; meshes
            // without one share an untextured slot.
            let albedo_names: Vec<Option<String>> =
                meshes.iter().map(|m| m.albedo_texture.clone()).collect();
            let (slot_textures, slot_ids) = material_slots(&albedo_names);

            // Flatten all meshes into one combined buffer with a
            // per-mesh MeshBatch so the renderer can colour-code them.
            let mut positions: Vec<[f32; 3]> = Vec::new();
            let mut uvs: Vec<[f32; 2]> = Vec::new();
            let mut indices: Vec<u32> = Vec::new();
            let mut batches: Vec<MeshBatch> = Vec::new();
            let mut bbox_min = [f32::INFINITY; 3];
            let mut bbox_max = [f32::NEG_INFINITY; 3];
            for (mesh_i, m) in meshes.iter().enumerate() {
                let vbase = positions.len() as u32;
                let ibase = indices.len() as u32;
                positions.extend_from_slice(&m.positions);
                uvs.extend_from_slice(&m.uvs);
                for i in &m.indices {
                    indices.push(vbase + i);
                }
                batches.push(MeshBatch {
                    start_index: ibase,
                    index_count: m.indices.len() as u32,
                    material_id: slot_ids[mesh_i],
                    blend: Default::default(),
                });
                for axis in 0..3 {
                    if m.bbox_min[axis] < bbox_min[axis] {
                        bbox_min[axis] = m.bbox_min[axis];
                    }
                    if m.bbox_max[axis] > bbox_max[axis] {
                        bbox_max[axis] = m.bbox_max[axis];
                    }
                }
            }
            if bbox_min[0] == f32::INFINITY {
                bbox_min = [0.0; 3];
                bbox_max = [0.0; 3];
            }

            // ── Resolve each slot's .texture file ────────────────────
            let mut materials: Vec<MeshMaterial> = Vec::with_capacity(slot_textures.len());
            let mut tex_wanted: u32 = 0;
            let mut tex_loaded: u32 = 0;
            for slot in &slot_textures {
                let decoded = slot.as_deref().and_then(|tex_name| {
                    tex_wanted += 1;
                    let bytes = texture_lookup_candidates(tex_name)
                        .iter()
                        .find_map(|c| (fetch.by_name)(c))?;
                    super::texture::decode_mip0(&bytes, tex_name)
                });
                materials.push(match decoded {
                    Some((rgba, w, h, _fmt)) => {
                        tex_loaded += 1;
                        MeshMaterial {
                            rgba: Some(rgba),
                            width: w,
                            height: h,
                        }
                    }
                    None => MeshMaterial {
                        rgba: None,
                        width: 1,
                        height: 1,
                    },
                });
            }

            if tex_wanted > 0 {
                text.push_str(&format!(
                    "\nColour textures: {tex_loaded}/{tex_wanted} loaded\n"
                ));
                for (m, name) in meshes.iter().zip(&albedo_names) {
                    text.push_str(&format!(
                        "  {}: {}\n",
                        m.name,
                        name.as_deref().unwrap_or("none")
                    ));
                }
            }

            let mesh = Arc::new(Mesh3dCpu {
                positions,
                uvs,
                indices,
                bbox_min,
                bbox_max,
                batches,
                materials,
            });

            let mesh_for_obj = Arc::clone(&mesh);
            out.extra_exports.push(ExportAction {
                label: "Export As OBJ",
                default_extension: "obj",
                filter_name: "Wavefront OBJ",
                build: Arc::new(move |_raw, _path| Ok(super::encode_obj(&mesh_for_obj))),
            });

            if !mesh.materials.is_empty() {
                let mesh_for_mtl = Arc::clone(&mesh);
                out.extra_exports.push(ExportAction {
                    label: "Export As OBJ + MTL",
                    default_extension: "obj",
                    filter_name: "Wavefront OBJ",
                    build: Arc::new(move |_raw, path| {
                        super::encode_obj_with_materials(&mesh_for_mtl, path)
                    }),
                });
            }

            out.mesh3d = Some(mesh);
        }

        // Top-level element tree: one level deep.  Users can see the
        // skeleton name, mesh name, etc. without us having to interpret
        // the geometry.
        text.push_str("\nTop-level tree:\n");
        for e in &gf.root_elements {
            text.push_str(&format!("  - {} :: {}\n", e.name, kind_label(&e.value)));
        }

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

/// Build lookup candidates for a Granny texture path.
///
/// D2R stores paths like `"data:data/hd/env/…/foo.texture"`.  The
/// CASC virtual tree might index them with or without the `data:`
/// prefix, so we try the full path, without prefix, and just the
/// filename, in that order.
fn texture_lookup_candidates(raw: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(4);
    // Full path as-is.
    out.push(raw.to_string());
    // Strip `data:` URI prefix.
    let bare = raw.strip_prefix("data:").unwrap_or(raw);
    if bare != raw {
        out.push(bare.to_string());
    }
    // Models name `data/hd/...`; the archive files it one `data`
    // folder deeper.
    out.push(format!("data/{bare}"));
    // Bare filename.
    if let Some(slash) = raw.rfind('/') {
        let basename = &raw[slash + 1..];
        if !basename.is_empty() && !out.iter().any(|c| c == basename) {
            out.push(basename.to_string());
        }
    }
    out
}

/// Give each distinct texture name one material slot, and all the
/// meshes without a texture one shared slot.  Returns the slots and,
/// per mesh, the slot it draws with.
fn material_slots(names: &[Option<String>]) -> (Vec<Option<String>>, Vec<u32>) {
    let mut slots: Vec<Option<String>> = Vec::new();
    let ids = names
        .iter()
        .map(|name| {
            let i = slots.iter().position(|s| s == name).unwrap_or_else(|| {
                slots.push(name.clone());
                slots.len() - 1
            });
            i as u32
        })
        .collect();
    (slots, ids)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The model names `data/hd/...`; the archive files it under
    /// `data/data/hd/...`.
    #[test]
    fn a_texture_is_also_looked_up_under_the_archive_data_folder() {
        let c = texture_lookup_candidates(
            "data/hd/character/npc/cain1/textures/cain1_body_alb.texture",
        );
        assert!(c.contains(
            &"data/data/hd/character/npc/cain1/textures/cain1_body_alb.texture".to_string()
        ));
    }

    #[test]
    fn meshes_sharing_a_texture_share_a_material() {
        let names = [
            Some("head_alb.texture".to_string()),
            None,
            Some("hair_alb.texture".to_string()),
            Some("head_alb.texture".to_string()),
        ];
        let (textures, ids) = material_slots(&names);
        assert_eq!(
            textures,
            vec![
                Some("head_alb.texture".to_string()),
                None,
                Some("hair_alb.texture".to_string())
            ]
        );
        assert_eq!(ids, vec![0, 1, 2, 0]);
    }
}
