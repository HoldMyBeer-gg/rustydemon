//! Diablo Immortal mesh preview (Messiah engine `.MESSIAH` mesh objects).
//!
//! Geometry goes to the 3D viewport, one batch per submesh. Materials and
//! textures are not resolved yet, so batches get the viewport's flat colours.

use std::sync::Arc;

use rustydemon_messiah::{looks_like_mesh, Mesh, MessiahError};

use super::{ExportAction, Mesh3dCpu, MeshBatch, PreviewOutput, PreviewPlugin, SiblingFetcher};

pub struct MessiahMeshPreview;

impl PreviewPlugin for MessiahMeshPreview {
    fn name(&self) -> &str {
        "Messiah mesh (Diablo Immortal)"
    }

    fn can_preview(&self, _filename: &str, data: &[u8]) -> bool {
        looks_like_mesh(data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _fetch: &SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let mesh = match Mesh::parse(data) {
            Ok(m) => m,
            Err(MessiahError::Compressed) => {
                out.text = Some(
                    "Messiah mesh\n\nThis mesh's data is compressed with a codec that is \
                     not decoded yet, so it cannot be drawn."
                        .into(),
                );
                return out;
            }
            Err(e) => {
                out.text = Some(format!("Messiah mesh\n\nCould not read it: {e}"));
                return out;
            }
        };

        let mut text = String::from("Messiah mesh\n\n");
        text.push_str(&format!(
            "Vertices          {}\nTriangles         {}\nSubmeshes         {}\nVertex streams    {}\n",
            mesh.positions.len(),
            mesh.indices.len() / 3,
            mesh.submeshes.len(),
            mesh.formats.join(", "),
        ));
        let [x0, y0, z0] = mesh.bbox_min;
        let [x1, y1, z1] = mesh.bbox_max;
        text.push_str(&format!(
            "Bounds            ({x0:.2}, {y0:.2}, {z0:.2}) to ({x1:.2}, {y1:.2}, {z1:.2})\n"
        ));
        out.text = Some(text);

        let batches = mesh
            .submeshes
            .iter()
            .enumerate()
            .map(|(i, s)| MeshBatch {
                start_index: s.start_index,
                index_count: s.index_count,
                material_id: i as u32,
                blend: Default::default(),
            })
            .collect();
        let cpu = Arc::new(Mesh3dCpu {
            positions: mesh.positions,
            uvs: mesh.uvs,
            indices: mesh.indices,
            bbox_min: mesh.bbox_min,
            bbox_max: mesh.bbox_max,
            batches,
            materials: Vec::new(),
        });
        let for_obj = Arc::clone(&cpu);
        out.extra_exports.push(ExportAction {
            label: "Export As OBJ",
            default_extension: "obj",
            filter_name: "Wavefront OBJ",
            build: Arc::new(move |_raw, _path| Ok(super::encode_obj(&for_obj))),
        });
        out.mesh3d = Some(cpu);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One triangle in the interleaved layout static props use.
    fn triangle(flag: u16) -> Vec<u8> {
        let mut b = b".MESSIAH".to_vec();
        for v in [8u32, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&flag.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        for f in ["P3F_N4B_T2F", "None", "None", "None"] {
            b.extend_from_slice(&(f.len() as u16).to_le_bytes());
            b.extend_from_slice(f.as_bytes());
        }
        let floats = [0.0f32, 0.0, 0.0, 1.0, 1.0, 0.0, 0.5, 0.5, 0.0, 0.8];
        b.extend(floats.iter().flat_map(|f| f.to_le_bytes()));
        b.extend([0u32, 3, 0, 3].iter().flat_map(|v| v.to_le_bytes()));
        b.extend([0u16, 1, 2].iter().flat_map(|v| v.to_le_bytes()));
        for p in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            b.extend(p.iter().flat_map(|f| f.to_le_bytes()));
            b.extend([0x80, 0x80, 0xff, 0]);
            b.extend([0.0f32, 0.0].iter().flat_map(|f| f.to_le_bytes()));
        }
        b.extend([0u8; 52]);
        b
    }

    fn preview(data: &[u8]) -> PreviewOutput {
        let by_name = |_: &str| None;
        let none = |_: u32| None;
        let no_info = |_: &str| None;
        let no_skins = || None;
        let siblings = SiblingFetcher {
            by_name: &by_name,
            by_fdid: &none,
            texture_info: &no_info,
            monster_skins: &no_skins,
        };
        MessiahMeshPreview.build(
            "Package/ab/abcd",
            data,
            &egui::Context::default(),
            &siblings,
        )
    }

    #[test]
    fn a_mesh_reaches_the_viewport() {
        let data = triangle(0);
        assert!(MessiahMeshPreview.can_preview("Package/ab/abcd", &data));
        let out = preview(&data);
        let mesh = out.mesh3d.expect("a mesh");
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.batches.len(), 1);
        assert!(out.text.unwrap().contains("Triangles         1"));
    }

    #[test]
    fn a_compressed_mesh_says_so_instead_of_drawing() {
        let out = preview(&triangle(2));
        assert!(out.mesh3d.is_none());
        assert!(out.text.unwrap().contains("compressed"));
    }
}
