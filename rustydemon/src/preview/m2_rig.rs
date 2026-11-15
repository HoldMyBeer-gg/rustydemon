//! An M2 model in the shared glTF writer's terms: the skeleton from the
//! bone table, one part per render batch, four bone weights per vertex.
//!
//! The parts are the batches the viewer draws, so the export shows what
//! the preview showed: the same skin, the same texture per batch.  Bones
//! rest at their pivot with no rotation, placed relative to the parent's
//! pivot, which is how the game composes them before any animation.
//! Animation tracks are not exported yet.

use std::collections::HashMap;

use super::gltf;
use super::m2_lite::M2Lite;
use super::MeshBatch;

/// Material name of texture slot `i`, shared by the parts that use the
/// slot and the texture list.
pub fn texture_name(slot: usize) -> String {
    format!("texture_{slot}")
}

/// The model for the writer.  `indices` and `batches` are the resolved
/// triangle list and draw batches the viewer built from the skin;
/// `textures` are PNGs named by [`texture_name`].
pub fn model(
    m2: &M2Lite,
    indices: &[u32],
    batches: &[MeshBatch],
    textures: Vec<(String, Vec<u8>)>,
) -> gltf::Model {
    let bones: Vec<gltf::Bone> = m2
        .bones
        .iter()
        .map(|b| {
            let parent = usize::try_from(b.parent)
                .ok()
                .filter(|&p| p < m2.bones.len());
            let origin = parent.map(|p| m2.bones[p].pivot).unwrap_or_default();
            gltf::Bone {
                name: String::new(),
                parent,
                rotation: [0.0, 0.0, 0.0, 1.0],
                translation: [0, 1, 2].map(|k| b.pivot[k] - origin[k]),
                scale: [1.0; 3],
            }
        })
        .collect();

    let mut parts = Vec::new();
    for (n, batch) in batches.iter().enumerate() {
        let start = (batch.start_index as usize).min(indices.len());
        let end = start
            .saturating_add(batch.index_count as usize)
            .min(indices.len());
        if start == end {
            continue;
        }
        let mut part = gltf::Part {
            name: format!("part_{n}"),
            material: Some(texture_name(batch.material_id as usize)),
            ..Default::default()
        };
        // Each part carries only the vertices its triangles use, numbered
        // in order of first use.
        let mut local: HashMap<u32, u32> = HashMap::new();
        for &i in &indices[start..end] {
            let v = *local.entry(i).or_insert_with(|| {
                let vi = i as usize;
                part.positions
                    .push(m2.positions.get(vi).copied().unwrap_or_default());
                part.uvs.push(m2.uvs.get(vi).copied().unwrap_or_default());
                if !bones.is_empty() {
                    part.skin.push(influences(m2, vi, bones.len()));
                }
                part.positions.len() as u32 - 1
            });
            part.indices.push(v);
        }
        parts.push(part);
    }

    gltf::Model {
        bones,
        parts,
        textures,
        animations: Vec::new(),
        z_up: true,
    }
}

/// The four (bone, weight) pairs of vertex `vi`, weights scaled from
/// 0..=255 to 0..=1.  A slot with no weight, or one naming a bone the
/// skeleton does not have, is left empty.
fn influences(m2: &M2Lite, vi: usize, bone_count: usize) -> [(u16, f32); 4] {
    let weights = m2.bone_weights.get(vi).copied().unwrap_or_default();
    let bones = m2.bone_indices.get(vi).copied().unwrap_or_default();
    let mut out = [(0u16, 0.0f32); 4];
    for k in 0..4 {
        if weights[k] > 0 && usize::from(bones[k]) < bone_count {
            out[k] = (u16::from(bones[k]), f32::from(weights[k]) / 255.0);
        }
    }
    out
}
