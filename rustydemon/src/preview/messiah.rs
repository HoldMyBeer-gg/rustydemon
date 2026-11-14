//! Diablo Immortal mesh preview (Messiah engine `.MESSIAH` mesh objects).
//!
//! Geometry goes to the 3D viewport, one batch per submesh. When the
//! install's resource registry links the mesh to a model, each submesh is
//! painted with its sub-material's base texture. Textures (`.6` files)
//! preview on their own as well.

use std::sync::Arc;

use rustydemon_messiah::{
    looks_like_mesh,
    material::Material,
    repository::{package_path, Repository},
    texture::{looks_like_texture, Texture, FORMAT_BC7},
    Mesh, MessiahError,
};

use super::{
    ExportAction, Mesh3dCpu, MeshBatch, MeshMaterial, PreviewOutput, PreviewPlugin, SiblingFetcher,
};

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
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        fetch: &SiblingFetcher<'_>,
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
        let materials = (fetch.messiah_repo)()
            .map(|repo| submesh_materials(&repo, filename, mesh.submeshes.len(), fetch))
            .unwrap_or_default();
        let painted = materials.iter().filter(|m| m.rgba.is_some()).count();
        if !materials.is_empty() {
            text.push_str(&format!(
                "Textures          {painted} of {} submeshes
",
                mesh.submeshes.len()
            ));
        }
        out.text = Some(text);

        let batches = mesh
            .submeshes
            .iter()
            .enumerate()
            .map(|(i, s)| MeshBatch {
                start_index: s.start_index,
                index_count: s.index_count,
                material_id: if i < materials.len() {
                    i as u32
                } else {
                    u32::MAX
                },
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
            materials,
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

/// Base texture for each submesh, found through the model that uses the
/// mesh. Empty when the registry does not link the mesh to a material.
fn submesh_materials(
    repo: &Repository,
    filename: &str,
    submeshes: usize,
    fetch: &SiblingFetcher<'_>,
) -> Vec<MeshMaterial> {
    let Some(guid) = repo.guid_for_path(filename) else {
        return Vec::new();
    };
    let base_maps: Vec<Option<String>> = repo
        .materials_for_mesh(&guid)
        .iter()
        .filter_map(|m| (fetch.by_name)(&package_path(m)))
        .flat_map(|bytes| Material::parse(&bytes).base_maps)
        .collect();
    if base_maps.is_empty() {
        return Vec::new();
    }
    let mut decoded: std::collections::HashMap<String, Option<(Vec<u8>, u32, u32)>> =
        std::collections::HashMap::new();
    (0..submeshes)
        .map(|i| {
            let pixels = base_maps.get(i).cloned().flatten().and_then(|tex| {
                decoded
                    .entry(tex.clone())
                    .or_insert_with(|| {
                        (fetch.by_name)(&format!("{}.6", package_path(&tex)))
                            .and_then(|b| decode_texture(&b).ok())
                    })
                    .clone()
            });
            match pixels {
                Some((rgba, width, height)) => MeshMaterial {
                    rgba: Some(rgba),
                    width,
                    height,
                },
                None => MeshMaterial {
                    rgba: None,
                    width: 1,
                    height: 1,
                },
            }
        })
        .collect()
}

/// Decode a texture's largest mip to RGBA.
pub fn decode_texture(data: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let tex = Texture::parse(data).map_err(|e| e.to_string())?;
    if tex.format != FORMAT_BC7 {
        return Err(format!(
            "texture format 0x{:02X} is not decoded yet",
            tex.format
        ));
    }
    let mip = tex.largest_mip().ok_or("texture has no mips")?;
    let (w, h) = (mip.width as usize, mip.height as usize);
    let mut px = vec![0u32; w * h];
    texture2ddecoder::decode_bc7(&mip.data, w, h, &mut px)?;
    Ok((
        crate::tex_preview::u32_to_rgba_bytes(&px),
        mip.width,
        mip.height,
    ))
}

pub struct MessiahTexturePreview;

impl PreviewPlugin for MessiahTexturePreview {
    fn name(&self) -> &str {
        "Messiah texture (Diablo Immortal)"
    }

    fn can_preview(&self, _filename: &str, data: &[u8]) -> bool {
        looks_like_texture(data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        _fetch: &SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        match decode_texture(data) {
            Ok((rgba, w, h)) => {
                let image =
                    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                out.texture = Some(ctx.load_texture(
                    "messiah_texture",
                    image,
                    egui::TextureOptions::default(),
                ));
                out.text = Some(format!("Messiah texture {w}×{h}, BC7"));
                out.texture_pixels = Some((rgba, w, h));
                out.extra_exports.push(ExportAction {
                    label: "Export As PNG",
                    default_extension: "png",
                    filter_name: "PNG image",
                    build: Arc::new(|data, _path| {
                        let (rgba, w, h) = decode_texture(data)?;
                        super::encode_png(&rgba, w, h)
                    }),
                });
            }
            Err(e) => {
                out.text = Some(format!(
                    "Messiah texture

Could not decode it: {e}"
                ))
            }
        }
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
        let no_repo = || None;
        let siblings = SiblingFetcher {
            by_name: &by_name,
            by_fdid: &none,
            texture_info: &no_info,
            monster_skins: &no_skins,
            messiah_repo: &no_repo,
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

    /// One 4x4 BC7 texture: a single all-zero block.
    fn texture() -> Vec<u8> {
        let mut b = vec![2, 2, 2, 1, 1, 0x19, 1, 0, 0, 0, 0, 1, 4, 0, 4, 0];
        b.extend([0u8; 16]);
        let mut mip = Vec::new();
        mip.extend_from_slice(&36u32.to_le_bytes());
        for v in [4u16, 4, 0, 16] {
            mip.extend_from_slice(&v.to_le_bytes());
        }
        mip.extend_from_slice(&16u32.to_le_bytes());
        mip.extend_from_slice(b"NNNN");
        mip.extend([0u8; 16]);
        b.extend_from_slice(&((4 + mip.len()) as u32).to_le_bytes());
        b.extend([1, 0x19, 1, 0]);
        b.extend(mip);
        b
    }

    const MESH: &str = "11111111-1111-1111-1111-111111111111";
    const MAT: &str = "22222222-2222-2222-2222-222222222222";
    const MODEL: &str = "33333333-3333-3333-3333-333333333333";
    const TEX: &str = "44444444-4444-4444-4444-444444444444";

    /// A registry where a model uses MESH and MAT, and MAT uses TEX.
    fn repository() -> Repository {
        let guid = |s: &str| rustydemon_messiah::repository::parse_uuid(s).unwrap();
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        let types = b"Texture2D;ParticleSystem;LodModel;Model;Mesh;Material";
        b.extend_from_slice(&(types.len() as u16).to_le_bytes());
        b.extend_from_slice(types);
        let folders = b"@^_^@;Char/crab";
        b.extend_from_slice(&(folders.len() as u16).to_le_bytes());
        b.extend_from_slice(folders);
        let entries: [(&str, &str, u16, Vec<&str>); 4] = [
            (TEX, "crab_d", 0, vec![]),
            (MESH, "Mesh_crab", 4, vec![]),
            (MAT, "crab_mat", 5, vec![TEX]),
            (MODEL, "crab", 3, vec![MAT, MESH]),
        ];
        for (g, name, kind, deps) in entries {
            b.extend_from_slice(&[5, 0, 1, 0, 0]);
            b.extend_from_slice(&guid(g));
            b.extend_from_slice(&(name.len() as u16).to_le_bytes());
            b.extend_from_slice(name.as_bytes());
            for v in [1u16, kind, deps.len() as u16] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            for d in deps {
                b.extend_from_slice(&guid(d));
            }
        }
        Repository::parse(&b).unwrap()
    }

    #[test]
    fn a_texture_previews_as_an_image() {
        let data = texture();
        assert!(MessiahTexturePreview.can_preview("Package/44/x.6", &data));
        let by_name = |_: &str| None;
        let none = |_: u32| None;
        let no_info = |_: &str| None;
        let no_skins = || None;
        let no_repo = || None;
        let siblings = SiblingFetcher {
            by_name: &by_name,
            by_fdid: &none,
            texture_info: &no_info,
            monster_skins: &no_skins,
            messiah_repo: &no_repo,
        };
        let out = MessiahTexturePreview.build("x.6", &data, &egui::Context::default(), &siblings);
        let (rgba, w, h) = out.texture_pixels.expect("pixels");
        assert_eq!((w, h, rgba.len()), (4, 4, 64));
    }

    #[test]
    fn a_registered_mesh_is_painted_with_its_base_texture() {
        let repo = Arc::new(repository());
        let material = format!(
            ".MESSIAH      crab_mat PBR    @ tBaseMap={TEX}
cAlpha=1
"
        );
        let tex = texture();
        let by_name = |path: &str| {
            if path == package_path(MAT) {
                Some(material.clone().into_bytes())
            } else if path == format!("{}.6", package_path(TEX)) {
                Some(tex.clone())
            } else {
                None
            }
        };
        let none = |_: u32| None;
        let no_info = |_: &str| None;
        let no_skins = || None;
        let with_repo = || Some(Arc::clone(&repo));
        let siblings = SiblingFetcher {
            by_name: &by_name,
            by_fdid: &none,
            texture_info: &no_info,
            monster_skins: &no_skins,
            messiah_repo: &with_repo,
        };
        let out = MessiahMeshPreview.build(
            "Char/crab/Mesh_crab.mesh",
            &triangle(0),
            &egui::Context::default(),
            &siblings,
        );
        let mesh = out.mesh3d.expect("a mesh");
        assert_eq!(mesh.materials.len(), 1);
        assert_eq!(mesh.materials[0].width, 4);
        assert!(mesh.materials[0].rgba.is_some());
        assert_eq!(mesh.batches[0].material_id, 0);
    }

    #[test]
    fn a_compressed_mesh_says_so_instead_of_drawing() {
        let out = preview(&triangle(2));
        assert!(out.mesh3d.is_none());
        assert!(out.text.unwrap().contains("compressed"));
    }
}
