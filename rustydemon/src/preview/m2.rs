//! M2 model preview plugin (wow-alchemy-m2 backend).
//!
//! v0: standalone M2 viewer. Click an `.m2` file, see the static bind
//! pose with per-submesh hash colours. No animation, no doodad
//! placement, no texture lookup yet — those layer on later.
//!
//! Why `wow-alchemy-m2` instead of `wow-m2`: the older `wow-m2` 0.6.4
//! crate ships a stub MD21 parser that returns an empty model and
//! hardcodes pre-TWW chunk sizes (LDV1 etc.), so retail TWW M2s fail
//! either silently or with a chunk-size mismatch error. The
//! `wow-alchemy-m2` fork properly unwraps the MD21 wrapper, parses the
//! inner MD20 from a chunk-relative cursor, and tolerates unknown
//! chunks by stashing them as opaque blobs.

use std::io::Cursor;
use std::sync::Arc;

use wow_alchemy_data::types::{VWowStructR, WowStructR};
use wow_alchemy_m2::skin::SkinVersion;
use wow_alchemy_m2::{M2Model, Skin};

use super::{Mesh3dCpu, MeshBatch, PreviewOutput, PreviewPlugin};

pub struct M2Preview;

fn looks_like_m2(data: &[u8]) -> bool {
    data.len() >= 4 && (&data[..4] == b"MD20" || &data[..4] == b"MD21")
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

        // Locate the SFID chunk (skin file FDIDs) by walking m2.chunks.
        // wow-alchemy-m2 stores them in a typed enum variant.
        let skin_fdids: Vec<u32> = m2
            .chunks
            .iter()
            .find_map(|c| match c {
                wow_alchemy_m2::model::M2Chunk::SFID(skins) => Some(skins.file_ids.clone()),
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
        // Skipping this indirection connects wrong vertices → missing faces.
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

        // One batch per skin submesh. SkinSubmesh.triangle_start /
        // triangle_count are in *index* units (each = one entry in the
        // triangles buffer), so they slot directly into our index buffer.
        let batches: Vec<MeshBatch> = if skin.submeshes.is_empty() {
            vec![MeshBatch {
                start_index: 0,
                index_count: indices.len() as u32,
                material_id: 0,
            }]
        } else {
            skin.submeshes
                .iter()
                .map(|s| MeshBatch {
                    start_index: s.triangle_start as u32,
                    index_count: s.triangle_count as u32,
                    material_id: s.id as u32,
                })
                .collect()
        };

        let (mn, mx) = compute_bbox(&positions);
        out.mesh3d = Some(Arc::new(Mesh3dCpu {
            positions,
            uvs,
            indices,
            bbox_min: mn,
            bbox_max: mx,
            batches,
            // No texture wiring yet — renderer falls back to per-batch
            // hash colours from material_id.
            materials: Vec::new(),
        }));

        text.push_str(&format!(
            "\n\nLoaded SKIN with {} submesh(es)",
            skin.submeshes.len()
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
