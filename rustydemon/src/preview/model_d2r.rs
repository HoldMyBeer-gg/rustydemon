//! D2R `.model` preview plugin.
//!
//! Uses the pure-Rust [`rustydemon_gr2`] reader to parse Granny3D
//! files, extract mesh geometry into the 3D viewport, and resolve
//! sibling `.texture` files as materials.  Shows a structural summary
//! (source file, textures, mesh / bone / animation counts, element
//! tree) alongside the rendered preview.

use std::sync::Arc;

use super::{Mesh3dCpu, MeshBatch, MeshMaterial, PreviewOutput, PreviewPlugin};
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
                    material_id: m.material_index.unwrap_or(mesh_i as u32),
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

            // ── Resolve sibling .texture files as materials ──────────
            // Texture filenames from the Granny tree (in order).  Each
            // mesh's material_index typically maps 1:1 to this array.
            let tex_names = gf.texture_filenames();
            let mut materials: Vec<MeshMaterial> = Vec::with_capacity(tex_names.len());
            let mut tex_loaded: u32 = 0;
            let mut tex_failed: u32 = 0;
            for tex_name in &tex_names {
                // D2R texture paths look like
                //   "data:data/hd/env/act1/outdoors/…/foo.texture"
                // The sibling fetcher expects the virtual path without
                // the "data:" prefix, or just the bare filename. Try
                // progressively shorter forms.
                let candidates = texture_lookup_candidates(tex_name);
                let bytes = candidates.iter().find_map(|c| (fetch.by_name)(c));
                let decoded = bytes
                    .as_deref()
                    .and_then(|b| super::texture::decode_mip0(b, tex_name));
                match decoded {
                    Some((rgba, w, h, _fmt)) => {
                        tex_loaded += 1;
                        materials.push(MeshMaterial {
                            rgba: Some(rgba),
                            width: w,
                            height: h,
                        });
                    }
                    None => {
                        tex_failed += 1;
                        materials.push(MeshMaterial {
                            rgba: None,
                            width: 1,
                            height: 1,
                        });
                    }
                }
            }

            if !tex_names.is_empty() {
                text.push_str(&format!(
                    "\nTexture materials: {tex_loaded}/{} loaded{}",
                    tex_names.len(),
                    if tex_failed > 0 {
                        format!(" ({tex_failed} fallback)")
                    } else {
                        String::new()
                    },
                ));
            }

            out.mesh3d = Some(Arc::new(Mesh3dCpu {
                positions,
                uvs,
                indices,
                bbox_min,
                bbox_max,
                batches,
                materials,
            }));
        }

        // Top-level element tree — one level deep.  Users can see the
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
/// filename — in that order.
fn texture_lookup_candidates(raw: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(3);
    // Full path as-is.
    out.push(raw.to_string());
    // Strip `data:` URI prefix.
    if let Some(stripped) = raw.strip_prefix("data:") {
        out.push(stripped.to_string());
    }
    // Bare filename.
    if let Some(slash) = raw.rfind('/') {
        let basename = &raw[slash + 1..];
        if !basename.is_empty() && !out.iter().any(|c| c == basename) {
            out.push(basename.to_string());
        }
    }
    out
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
