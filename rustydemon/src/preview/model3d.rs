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

use wow_wmo::group_parser::WmoGroup as ParsedGroup;
use wow_wmo::{parse_wmo, ParsedWmo};

use super::{Mesh3dCpu, MeshBatch, PreviewOutput, PreviewPlugin};

struct GroupMeshParts {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    batches: Vec<MeshBatch>,
}

/// Convert a parsed WMO group (the rich `group_parser` flavour returned
/// by `parse_wmo`, not the lighter `wmo_group_types::WmoGroup` re-export)
/// into the renderer's mesh format. Returns `None` if the group has no
/// geometry.
fn group_to_mesh_parts(group: &ParsedGroup) -> Option<GroupMeshParts> {
    let positions: Vec<[f32; 3]> = group
        .vertex_positions
        .iter()
        .map(|v| [v.x, v.y, v.z])
        .collect();
    let indices: Vec<u32> = group.vertex_indices.iter().map(|&i| i as u32).collect();
    if positions.is_empty() || indices.is_empty() {
        return None;
    }
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
    Some(GroupMeshParts {
        positions,
        indices,
        batches,
    })
}

/// Compute the bounding box of a position list. Returns infinities if
/// empty so callers must check before using.
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

/// Derive the path of group file `index` from a root WMO path.
/// `world/wmo/.../foo.wmo` → `world/wmo/.../foo_NNN.wmo`.
fn group_path_for(root_path: &str, index: u32) -> Option<String> {
    let stem = root_path.strip_suffix(".wmo").or_else(|| {
        // Tolerate case variation on the extension itself.
        if root_path.to_ascii_lowercase().ends_with(".wmo") {
            Some(&root_path[..root_path.len() - 4])
        } else {
            None
        }
    })?;
    Some(format!("{stem}_{index:03}.wmo"))
}

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

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        siblings: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
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

                // Walk the sibling group files and accumulate one big
                // combined mesh. Index buffers are concatenated with each
                // group's indices offset by the vertices already loaded.
                //
                // Modern WoW (Legion+) references group files by
                // FileDataID via the GFID chunk; pre-Legion archives use
                // `<root>_NNN.wmo` filenames. Try GFID first and fall
                // back to path-based lookup if the chunk is empty.
                let mut combined_positions: Vec<[f32; 3]> = Vec::new();
                let mut combined_indices: Vec<u32> = Vec::new();
                let mut combined_batches: Vec<MeshBatch> = Vec::new();
                let mut loaded_groups: u32 = 0;
                let mut failed_groups: u32 = 0;

                let use_fdid = !root.group_file_ids.is_empty();
                let group_count = if use_fdid {
                    root.group_file_ids.len() as u32
                } else {
                    root.n_groups
                };

                for i in 0..group_count {
                    let bytes_opt: Option<Vec<u8>> = if use_fdid {
                        let fdid = root.group_file_ids[i as usize];
                        (siblings.by_fdid)(fdid)
                    } else {
                        let Some(group_path) = group_path_for(filename, i) else {
                            break;
                        };
                        (siblings.by_name)(&group_path)
                    };
                    let Some(bytes) = bytes_opt else {
                        failed_groups += 1;
                        continue;
                    };
                    let mut greader = Cursor::new(bytes.as_slice());
                    let parsed = match parse_wmo(&mut greader) {
                        Ok(ParsedWmo::Group(g)) => g,
                        _ => {
                            failed_groups += 1;
                            continue;
                        }
                    };
                    let Some(mut parts) = group_to_mesh_parts(&parsed) else {
                        // Group has no renderable geometry (LOD stub etc).
                        continue;
                    };

                    let vertex_offset = combined_positions.len() as u32;
                    let index_offset = combined_indices.len() as u32;
                    combined_positions.append(&mut parts.positions);
                    combined_indices.extend(parts.indices.iter().map(|&i| i + vertex_offset));
                    combined_batches.extend(parts.batches.iter().map(|b| MeshBatch {
                        start_index: b.start_index + index_offset,
                        index_count: b.index_count,
                        material_id: b.material_id,
                    }));
                    loaded_groups += 1;
                }

                if !combined_positions.is_empty() {
                    let (bmn, bmx) = compute_bbox(&combined_positions);
                    out.mesh3d = Some(Arc::new(Mesh3dCpu {
                        positions: combined_positions,
                        indices: combined_indices,
                        bbox_min: bmn,
                        bbox_max: bmx,
                        batches: combined_batches,
                    }));
                    if let Some(t) = out.text.as_mut() {
                        t.push_str(&format!(
                            "\n\nLoaded {loaded_groups} group file(s)\
                             {} skipped",
                            if failed_groups > 0 {
                                format!(", {failed_groups}")
                            } else {
                                String::new()
                            }
                        ));
                    }
                } else if let Some(t) = out.text.as_mut() {
                    t.push_str(
                        "\n\nNo group files found alongside this root \
                         (siblings missing from the archive).",
                    );
                }
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

                if let Some(parts) = group_to_mesh_parts(&group) {
                    let (mn, mx) = compute_bbox(&parts.positions);
                    out.mesh3d = Some(Arc::new(Mesh3dCpu {
                        positions: parts.positions,
                        indices: parts.indices,
                        bbox_min: mn,
                        bbox_max: mx,
                        batches: parts.batches,
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
