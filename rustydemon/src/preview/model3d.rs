//! 3D model preview plugin.
//!
//! v0: WMO root + group text summary. Renders nothing yet — the goal of
//! this first cut is to confirm `wow-wmo` parses real retail WMO files
//! end-to-end so we know the foundation is sound before we wire up a
//! wgpu paint callback.
//!
//! WMOs are split into a root file (e.g. `Building.wmo`) holding
//! materials + the group list, and N group files (`Building_000.wmo`,
//! `Building_001.wmo`, ...) holding the actual vertex/triangle data.
//! The current PreviewPlugin interface only hands us one file's bytes,
//! so a full render will need a sibling-fetch hook added to the trait.

use std::io::Cursor;
use std::sync::Arc;

use wow_wmo::{parse_wmo, ParsedWmo};

use super::{Mesh3dCpu, MeshBatch, PreviewOutput, PreviewPlugin};

pub struct Model3dPreview;

fn looks_like_wmo(data: &[u8]) -> bool {
    // Both root and group WMOs start with an MVER chunk ("REVM" little-endian).
    data.len() >= 4 && &data[..4] == b"REVM"
}

impl PreviewPlugin for Model3dPreview {
    fn name(&self) -> &str {
        ".wmo (WoW world model)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        if !filename.to_ascii_lowercase().ends_with(".wmo") {
            return false;
        }
        looks_like_wmo(data)
    }

    fn build(&self, filename: &str, data: &[u8], _ctx: &egui::Context) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let mut reader = Cursor::new(data);

        match parse_wmo(&mut reader) {
            Ok(ParsedWmo::Root(root)) => {
                let mn = root.bounding_box_min;
                let mx = root.bounding_box_max;
                out.text = Some(format!(
                    "WMO root  •  {filename}\n\
                     ──────────────────────────\n\
                     version    : {}\n\
                     groups     : {}\n\
                     materials  : {}\n\
                     textures   : {}\n\
                     portals    : {}\n\
                     lights     : {}\n\
                     doodad sets: {}\n\
                     bounding box:\n  min ({:.1}, {:.1}, {:.1})\n  max ({:.1}, {:.1}, {:.1})",
                    root.version,
                    root.n_groups,
                    root.n_materials,
                    root.textures.len(),
                    root.n_portals,
                    root.n_lights,
                    root.n_doodad_sets,
                    mn[0],
                    mn[1],
                    mn[2],
                    mx[0],
                    mx[1],
                    mx[2],
                ));
            }
            Ok(ParsedWmo::Group(group)) => {
                let total_batches = group.trans_batch_count as u32
                    + group.int_batch_count as u32
                    + group.ext_batch_count as u32;
                out.text = Some(format!(
                    "WMO group  •  {filename}\n\
                     ──────────────────────────\n\
                     version  : {}\n\
                     vertices : {}\n\
                     triangles: {}\n\
                     batches  : {} (trans {} / int {} / ext {})",
                    group.version,
                    group.n_vertices,
                    group.n_triangles,
                    total_batches,
                    group.trans_batch_count,
                    group.int_batch_count,
                    group.ext_batch_count,
                ));

                // Build the CPU-side mesh for the 3D viewport.
                let positions: Vec<[f32; 3]> = group
                    .vertex_positions
                    .iter()
                    .map(|v| [v.x, v.y, v.z])
                    .collect();
                let indices: Vec<u32> = group.vertex_indices.iter().map(|&i| i as u32).collect();

                if !positions.is_empty() && !indices.is_empty() {
                    let mut mn = [f32::INFINITY; 3];
                    let mut mx = [f32::NEG_INFINITY; 3];
                    for p in &positions {
                        for i in 0..3 {
                            if p[i] < mn[i] {
                                mn[i] = p[i];
                            }
                            if p[i] > mx[i] {
                                mx[i] = p[i];
                            }
                        }
                    }

                    // Render-batches define material-bounded slices of the
                    // index buffer. Fall back to a single pseudo-batch
                    // covering everything if the group has none.
                    let batches: Vec<MeshBatch> = if group.render_batches.is_empty() {
                        vec![MeshBatch {
                            start_index: 0,
                            index_count: indices.len() as u32,
                            material_id: 0,
                        }]
                    } else {
                        group
                            .render_batches
                            .iter()
                            .map(|b| MeshBatch {
                                start_index: b.start_index,
                                index_count: b.count as u32,
                                material_id: b.material_id as u32,
                            })
                            .collect()
                    };

                    out.mesh3d = Some(Arc::new(Mesh3dCpu {
                        positions,
                        indices,
                        bbox_min: mn,
                        bbox_max: mx,
                        batches,
                    }));
                }
            }
            Err(e) => {
                out.text = Some(format!("WMO header detected but parsing failed:\n  {e}"));
            }
        }

        out
    }
}
