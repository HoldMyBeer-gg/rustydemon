//! ADT terrain preview plugin.
//!
//! Parses a WoW ADT root file via `wow-adt`, walks all 256 `MCNK` chunks
//! and emits a single [`Mesh3dCpu`] that the existing 3D viewport can
//! render.  Texture splatting and adjacent-tile streaming are future
//! commits; this pass just draws the heightmap so the whole tile's
//! geometry is visible end to end.
//!
//! ## Mesh shape per MCNK
//!
//! Each MCNK is a 33.333-yard square of terrain stored as 145 vertices:
//!
//! - 81 **outer** vertices on a 9×9 grid (corners of 8×8 quads)
//! - 64 **inner** vertices on an 8×8 grid, offset by half a step — one
//!   per quad, at the quad's center
//!
//! Triangulation per quad is a 4-triangle fan around the inner vertex:
//!
//! ```text
//!  outer(x,y) --------- outer(x+1,y)
//!       |\            /|
//!       | \          / |
//!       |  inner(x,y)  |
//!       | /          \ |
//!       |/            \|
//!  outer(x,y+1) ------ outer(x+1,y+1)
//! ```
//!
//! 64 quads × 4 triangles = 256 triangles per MCNK, 65,536 per ADT tile.

use std::io::Cursor;
use std::sync::Arc;

use super::{Mesh3dCpu, MeshBatch, PreviewOutput, PreviewPlugin};

pub struct AdtPreview;

/// 1/16 of a WoW ADT tile, in game yards.  Matches `CHUNKSIZE` in every
/// WoW client since vanilla.  An ADT tile is 16×16 MCNKs = 533.333 yards.
const MCNK_SIZE: f32 = 33.333_332;
/// Distance between adjacent outer-grid vertices inside a single MCNK.
const OUTER_STEP: f32 = MCNK_SIZE / 8.0;
/// Vertices on one axis of the outer grid.
const OUTER_DIM: usize = 9;
/// Vertices on one axis of the inner grid.
const INNER_DIM: usize = 8;
/// Total vertices contributed by one MCNK to the mesh.
const VERTS_PER_MCNK: usize = OUTER_DIM * OUTER_DIM + INNER_DIM * INNER_DIM; // 145

impl PreviewPlugin for AdtPreview {
    fn name(&self) -> &str {
        ".adt (WoW terrain)"
    }

    fn can_preview(&self, filename: &str, _data: &[u8]) -> bool {
        let lower = filename.to_ascii_lowercase();
        // Accept the root `.adt` only.  Retail WoW splits terrain across
        // `foo.adt` (heights, the thing we want), `foo_tex0.adt`
        // (texture layers), `foo_obj0.adt` (doodad placements) etc.
        // The split ones end in `_tex0.adt` / `_obj0.adt` / `_lod.adt`
        // and we skip them — they don't carry MCNK height data.
        lower.ends_with(".adt")
            && !lower.ends_with("_tex0.adt")
            && !lower.ends_with("_tex1.adt")
            && !lower.ends_with("_obj0.adt")
            && !lower.ends_with("_obj1.adt")
            && !lower.ends_with("_lod.adt")
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();

        let parsed = match wow_adt::parse_adt(&mut Cursor::new(data)) {
            Ok(p) => p,
            Err(e) => {
                out.text = Some(format!("ADT parse failed: {e}"));
                return out;
            }
        };

        // Only the root ADT carries terrain heights.  Split-format Tex0 /
        // Obj0 / Lod files are structurally valid ADTs in their own right
        // but contain no `MCNK` heightmap data — show a text note instead
        // of silently rendering an empty mesh.
        let mut root = match parsed {
            wow_adt::ParsedAdt::Root(r) => r,
            other => {
                out.text = Some(format!(
                    ".adt parsed as {:?}, but terrain heights live in the root \
                     .adt — open the main file (e.g. `world/maps/azeroth/azeroth_32_48.adt`) \
                     rather than a split `_tex0` / `_obj0` / `_lod` sibling.",
                    other.version()
                ));
                return out;
            }
        };

        let chunks = root.mcnk_chunks_mut();
        let chunk_count = chunks.len();
        if chunk_count == 0 {
            out.text =
                Some(".adt has no MCNK terrain chunks — this is probably a stub file.".into());
            return out;
        }

        // ── Build the mesh ────────────────────────────────────────────
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(chunk_count * VERTS_PER_MCNK);
        let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(chunk_count * VERTS_PER_MCNK);
        let mut indices: Vec<u32> = Vec::with_capacity(chunk_count * 256 * 3); // 256 tris per chunk

        let mut bbox_min = [f32::INFINITY; 3];
        let mut bbox_max = [f32::NEG_INFINITY; 3];

        for mcnk in chunks.iter() {
            let Some(heights) = mcnk.heights.as_ref() else {
                continue;
            };

            // `header.position` is stored as `[Z, X, Y]` in the file per the
            // wow-adt source comments — Z is vertical.  Fetch via explicit
            // indices rather than hoping a helper exists with the right name.
            let raw_pos = mcnk.header.position;
            let base_x = raw_pos[1];
            let base_y = raw_pos[2];
            let base_z = raw_pos[0];

            let vert_offset = positions.len() as u32;

            // Outer 9×9 grid (vertex indices 0..81 within this chunk).
            for iy in 0..OUTER_DIM {
                for ix in 0..OUTER_DIM {
                    let rel_h = heights.get_outer_height(ix, iy).unwrap_or(0.0);
                    let x = base_x + ix as f32 * OUTER_STEP;
                    let y = base_y + iy as f32 * OUTER_STEP;
                    let z = base_z + rel_h;
                    positions.push([x, y, z]);
                    uvs.push([0.0, 0.0]);
                    bbox_grow(&mut bbox_min, &mut bbox_max, [x, y, z]);
                }
            }

            // Inner 8×8 grid (vertex indices 81..145 within this chunk),
            // offset by half a step so each inner vertex sits at the
            // centre of an outer quad.
            for iy in 0..INNER_DIM {
                for ix in 0..INNER_DIM {
                    let rel_h = heights.get_inner_height(ix, iy).unwrap_or(0.0);
                    let x = base_x + (ix as f32 + 0.5) * OUTER_STEP;
                    let y = base_y + (iy as f32 + 0.5) * OUTER_STEP;
                    let z = base_z + rel_h;
                    positions.push([x, y, z]);
                    uvs.push([0.0, 0.0]);
                    bbox_grow(&mut bbox_min, &mut bbox_max, [x, y, z]);
                }
            }

            // Helper closures to turn (ix, iy) grid coordinates into the
            // global vertex index we just pushed.
            let outer =
                |ix: usize, iy: usize| -> u32 { vert_offset + (iy * OUTER_DIM + ix) as u32 };
            let inner = |ix: usize, iy: usize| -> u32 {
                vert_offset + (OUTER_DIM * OUTER_DIM + iy * INNER_DIM + ix) as u32
            };

            // 4-triangle fan per quad around the inner vertex.  Winding
            // order is consistent so back-face culling picks the right
            // side when the renderer adds it later.
            for qy in 0..INNER_DIM {
                for qx in 0..INNER_DIM {
                    let tl = outer(qx, qy);
                    let tr = outer(qx + 1, qy);
                    let br = outer(qx + 1, qy + 1);
                    let bl = outer(qx, qy + 1);
                    let c = inner(qx, qy);

                    indices.extend_from_slice(&[tl, tr, c]);
                    indices.extend_from_slice(&[tr, br, c]);
                    indices.extend_from_slice(&[br, bl, c]);
                    indices.extend_from_slice(&[bl, tl, c]);
                }
            }
        }

        if positions.is_empty() {
            out.text = Some(".adt terrain has no MCVT height data — cannot build a mesh.".into());
            return out;
        }

        let index_count = indices.len() as u32;
        let batches = vec![MeshBatch {
            start_index: 0,
            index_count,
            material_id: u32::MAX, // fallback colour
        }];

        let mesh = Mesh3dCpu {
            positions,
            uvs,
            indices,
            bbox_min,
            bbox_max,
            batches,
            materials: Vec::new(),
        };

        let stem = std::path::Path::new(filename)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(filename);
        out.text = Some(format!(
            "ADT terrain: {stem}\n\
             {chunk_count} MCNK chunks\n\
             {} vertices, {} triangles\n\
             bbox: ({:.0}, {:.0}, {:.0}) → ({:.0}, {:.0}, {:.0})",
            mesh.positions.len(),
            mesh.indices.len() / 3,
            bbox_min[0],
            bbox_min[1],
            bbox_min[2],
            bbox_max[0],
            bbox_max[1],
            bbox_max[2],
        ));
        out.mesh3d = Some(Arc::new(mesh));
        out
    }
}

fn bbox_grow(min: &mut [f32; 3], max: &mut [f32; 3], p: [f32; 3]) {
    for i in 0..3 {
        if p[i] < min[i] {
            min[i] = p[i];
        }
        if p[i] > max[i] {
            max[i] = p[i];
        }
    }
}
