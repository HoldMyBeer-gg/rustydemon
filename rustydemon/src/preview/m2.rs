//! M2 model preview plugin (wow-alchemy-m2 backend).
//!
//! Renders the static bind pose with per-submesh textures loaded from
//! the TXID chunk's FileDataIDs.  The texture lookup chain is:
//!
//! ```text
//! skin.texture_units[i].texture_combo_index
//!   → md20.texture_lookup_table[combo_index]
//!   → texture_fdids[lookup_value]
//!   → BLP file (fetched via SiblingFetcher::by_fdid)
//! ```
//!
//! No animation, no doodad placement — those layer on later.

use std::io::Cursor;
use std::sync::Arc;

use wow_alchemy_data::types::{VWowStructR, WowStructR};
use wow_alchemy_m2::skin::SkinVersion;
use wow_alchemy_m2::{M2Model, Skin};

use super::{ExportAction, Mesh3dCpu, MeshBatch, MeshMaterial, PreviewOutput, PreviewPlugin};

pub struct M2Preview;

fn looks_like_m2(data: &[u8]) -> bool {
    data.len() >= 4 && (&data[..4] == b"MD20" || &data[..4] == b"MD21")
}

/// Decode a BLP byte buffer into RGBA8 + dimensions.
fn decode_blp(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let blp = rustydemon_blp2::BlpFile::from_bytes(bytes.to_vec()).ok()?;
    let (pixels, w, h) = blp.get_pixels(0).ok()?;
    Some((pixels, w, h))
}

impl PreviewPlugin for M2Preview {
    fn name(&self) -> &str {
        ".m2 (WoW model)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        if !filename.to_ascii_lowercase().ends_with(".m2") {
            return false;
        }
        looks_like_m2(data)
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        siblings: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let mut reader = Cursor::new(data);

        let m2 = match M2Model::wow_read(&mut reader) {
            Ok(m) => m,
            Err(e) => {
                out.text = Some(format!("M2 parse failed: {e}"));
                return out;
            }
        };

        let md20 = &m2.md20;
        let is_chunked = &m2.magic == b"MD21";

        // Locate the SFID chunk (skin file FDIDs).
        let skin_fdids: Vec<u32> = m2
            .chunks
            .iter()
            .find_map(|c| match c {
                wow_alchemy_m2::model::M2Chunk::SFID(skins) => Some(skins.file_ids.clone()),
                _ => None,
            })
            .unwrap_or_default();

        // TXID chunk (texture FDIDs).
        let texture_fdids: Vec<u32> = m2
            .chunks
            .iter()
            .find_map(|c| match c {
                wow_alchemy_m2::model::M2Chunk::TXID(ids) => Some(ids.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let mut text = format!(
            "M2 model  •  {filename}\n\
             ──────────────────────────\n\
             format    : {}\n\
             name      : {}\n\
             vertices  : {}\n\
             textures  : {}\n\
             materials : {}\n\
             bones     : {}\n\
             skin files: {}",
            if is_chunked {
                "MD21 (Legion+)"
            } else {
                "MD20 (legacy)"
            },
            md20.name,
            md20.vertices.len(),
            md20.textures.len(),
            md20.materials.len(),
            md20.bones.len(),
            skin_fdids.len(),
        );

        // ── Pull SKIN 0 (highest-quality LOD) via the FDID fetcher ───────────
        let skin_bytes: Option<Vec<u8>> = if let Some(&id) = skin_fdids.first() {
            (siblings.by_fdid)(id)
        } else {
            None
        };

        let Some(skin_bytes) = skin_bytes else {
            text.push_str(
                "\n\nNo external SKIN file available (legacy embedded \
                 skins aren't supported yet — geometry will not render).",
            );
            out.text = Some(text);
            return out;
        };

        let mut skin_reader = Cursor::new(skin_bytes.as_slice());
        let skin = match Skin::wow_read(&mut skin_reader, SkinVersion::V3) {
            Ok(s) => s,
            Err(e) => {
                text.push_str(&format!("\n\nSKIN parse failed: {e}"));
                out.text = Some(text);
                return out;
            }
        };

        // ── Build the CPU-side mesh ──────────────────────────────────────────
        let positions: Vec<[f32; 3]> = md20
            .vertices
            .iter()
            .map(|v| [v.position.x, v.position.y, v.position.z])
            .collect();
        let uvs: Vec<[f32; 2]> = md20
            .vertices
            .iter()
            .map(|v| [v.tex_coords.x, v.tex_coords.y])
            .collect();

        // Skin triangles index into `skin.indices` (the vertex lookup
        // table), which in turn indexes into the MD20 vertex array.
        let indices: Vec<u32> = skin
            .triangles
            .iter()
            .map(|&i| skin.indices.get(i as usize).copied().unwrap_or(0) as u32)
            .collect();

        if positions.is_empty() || indices.is_empty() {
            text.push_str("\n\nModel has no geometry to render.");
            out.text = Some(text);
            return out;
        }

        // ── Fetch and decode textures by FDID ────────────────────────────────
        // Build one MeshMaterial per TXID entry.  The texture_lookup_table
        // and skin texture_units will map submeshes → material slots.
        let mut materials: Vec<MeshMaterial> = Vec::with_capacity(texture_fdids.len());
        let mut tex_loaded: u32 = 0;
        let mut tex_failed: u32 = 0;
        for &fdid in &texture_fdids {
            if fdid == 0 {
                // Slot 0 with FDID 0 = runtime-composited texture (skin,
                // hair, etc).  Can't resolve statically.
                tex_failed += 1;
                materials.push(MeshMaterial {
                    rgba: None,
                    width: 1,
                    height: 1,
                });
                continue;
            }
            let decoded = (siblings.by_fdid)(fdid).as_deref().and_then(decode_blp);
            match decoded {
                Some((rgba, w, h)) => {
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

        // ── Build render batches from skin texture_units ─────────────────────
        // Following Everlook's pattern: iterate texture_units (render
        // batches), not submeshes.  Each texture_unit references a submesh
        // via `skin_section_index` for geometry and resolves its texture
        // via `texture_combo_index` + `texture_count` into the
        // `texture_lookup_table`.  The first lookup entry is the diffuse
        // texture.
        //
        // Multiple texture_units can reference the same submesh (diffuse
        // pass, specular pass, env map pass).  For our single-texture
        // renderer we take `material_layer == 0` as the base/diffuse pass
        // and skip higher layers.
        let batches: Vec<MeshBatch> = if skin.texture_units.is_empty() {
            // Fallback: one batch per submesh (legacy models).
            skin.submeshes
                .iter()
                .enumerate()
                .map(|(i, s)| MeshBatch {
                    start_index: s.triangle_start as u32,
                    index_count: s.triangle_count as u32,
                    material_id: i as u32,
                })
                .collect()
        } else {
            skin.texture_units
                .iter()
                .filter(|tu| tu.material_layer == 0)
                .filter_map(|tu| {
                    let sub = skin.submeshes.get(tu.skin_section_index as usize)?;
                    // Resolve texture: skip combo_index entries in the
                    // lookup table, take texture_count, first is diffuse.
                    let combo_idx = tu.texture_combo_index as usize;
                    let tex_idx = md20
                        .texture_lookup_table
                        .get(combo_idx)
                        .copied()
                        .unwrap_or(-1);
                    let material_id = if tex_idx >= 0 && (tex_idx as usize) < materials.len() {
                        tex_idx as u32
                    } else {
                        tu.skin_section_index as u32
                    };
                    Some(MeshBatch {
                        start_index: sub.triangle_start as u32,
                        index_count: sub.triangle_count as u32,
                        material_id,
                    })
                })
                .collect()
        };

        let (mn, mx) = compute_bbox(&positions);
        let mesh = Arc::new(Mesh3dCpu {
            positions,
            uvs,
            indices,
            bbox_min: mn,
            bbox_max: mx,
            batches,
            materials,
        });

        // OBJ export captures the resolved mesh so the skin indirection
        // and texture mapping are already baked in.
        let mesh_for_obj = Arc::clone(&mesh);
        out.extra_exports.push(ExportAction {
            label: "Export As OBJ",
            default_extension: "obj",
            filter_name: "Wavefront OBJ",
            build: Arc::new(move |_raw, _path| Ok(super::encode_obj(&mesh_for_obj))),
        });

        // OBJ + MTL + texture PNGs — writes sibling files next to the OBJ.
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

        let batch_count = mesh.batches.len();
        out.mesh3d = Some(mesh);

        if !texture_fdids.is_empty() {
            text.push_str(&format!(
                "\n\nTextures: {tex_loaded}/{} loaded{}",
                texture_fdids.len(),
                if tex_failed > 0 {
                    format!(" ({tex_failed} fallback)")
                } else {
                    String::new()
                },
            ));
        }
        text.push_str(&format!(
            "\nLoaded SKIN with {} submesh(es), {} render batch(es)",
            skin.submeshes.len(),
            batch_count
        ));
        out.text = Some(text);
        out
    }
}

fn compute_bbox(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in positions {
        for i in 0..3 {
            if p[i] < mn[i] {
                mn[i] = p[i];
            }
            if p[i] > mx[i] {
                mx[i] = p[i];
            }
        }
    }
    (mn, mx)
}
