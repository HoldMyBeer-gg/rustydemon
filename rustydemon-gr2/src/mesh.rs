//! Geometry extraction from the parsed Granny element tree.
//!
//! Granny stores each mesh as a struct with a `PrimaryVertexData` /
//! `PrimaryTopology` reference pair.  `PrimaryVertexData.Vertices` is
//! a dynamic array of vertex structs, each of which carries its
//! attributes (Position, Normal, Tangent, TextureCoordinates*) as
//! fixed-size float arrays inside the struct.  `PrimaryTopology`
//! carries either an `Indices` (u32) or `Indices16` (u16) array.
//!
//! This module walks the generic tree produced by `GrannyFile::from_bytes`
//! and rebuilds a flat triangle list suitable for feeding into a 3D
//! viewport.  We only extract what we need for preview rendering
//! (position + UV + normal + triangle list); bone weights, tangents,
//! and morph targets are skipped for now.

use crate::element::{Element, ElementValue};
use crate::granny_file::GrannyFile;

/// One mesh, fully decoded into flat GPU-ready buffers.
#[derive(Debug, Clone)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Material index referenced by this mesh's first topology group.
    /// Other groups are merged into the same batch for now.  `None`
    /// means the mesh has no material binding at all.
    pub material_index: Option<u32>,
    /// `FromFileName` of the colour texture of the material this mesh
    /// is bound to, exactly as the file spells it.  `None` when the
    /// bound material has no `AlbedoTexture` map.
    pub albedo_texture: Option<String>,
}

impl GrannyFile {
    /// Extract every mesh's geometry.  Returns an empty vec if the
    /// file has no `Meshes` array or if every mesh has unreadable
    /// vertex/index data.  Unreadable meshes are skipped silently:
    /// the caller keeps the ones that did decode.
    pub fn meshes(&self) -> Vec<Mesh> {
        let meshes_elem = match self.find("Meshes") {
            Some(e) => e,
            None => return Vec::new(),
        };
        let groups = match &meshes_elem.value {
            ElementValue::ArrayOfReferences(groups) | ElementValue::ReferenceArray(groups) => {
                groups
            }
            _ => return Vec::new(),
        };
        groups.iter().filter_map(|g| extract_mesh(g)).collect()
    }
}

fn extract_mesh(mesh_fields: &[Element]) -> Option<Mesh> {
    let name = find_string(mesh_fields, "Name").unwrap_or_else(|| "mesh".to_string());

    let pvd_children = find_reference_children(mesh_fields, "PrimaryVertexData")?;
    let vertices = find_elem(pvd_children, "Vertices")?;
    let vert_groups = match &vertices.value {
        ElementValue::ArrayOfReferences(g) | ElementValue::ReferenceArray(g) => g,
        _ => return None,
    };

    let mut positions = Vec::with_capacity(vert_groups.len());
    let mut normals = Vec::with_capacity(vert_groups.len());
    let mut uvs = Vec::with_capacity(vert_groups.len());
    let mut bbox_min = [f32::INFINITY; 3];
    let mut bbox_max = [f32::NEG_INFINITY; 3];

    for v in vert_groups {
        let p = find_f32_array(v, "Position").unwrap_or([0.0; 4]);
        let pos = [p[0], p[1], p[2]];
        positions.push(pos);
        for axis in 0..3 {
            if pos[axis] < bbox_min[axis] {
                bbox_min[axis] = pos[axis];
            }
            if pos[axis] > bbox_max[axis] {
                bbox_max[axis] = pos[axis];
            }
        }

        let n = find_f32_array(v, "Normal").unwrap_or([0.0, 0.0, 1.0, 0.0]);
        normals.push([n[0], n[1], n[2]]);

        // Granny texture coordinates are 2-component in D2R; we still
        // read as f32_array which writes zeros for the missing slots.
        let uv = find_f32_array(v, "TextureCoordinates0").unwrap_or([0.0; 4]);
        uvs.push([uv[0], uv[1]]);
    }

    if positions.is_empty() {
        return None;
    }
    if bbox_min[0] == f32::INFINITY {
        bbox_min = [0.0; 3];
        bbox_max = [0.0; 3];
    }

    // Topology: prefer 16-bit indices (Indices16), fall back to 32-bit
    // (Indices) if the mesh is large.  D2R's throwing_knife has
    // 3558 entries in Indices16 → 1186 triangles, which matches the
    // TriCount stored in the first Groups entry.
    let topo_children = find_reference_children(mesh_fields, "PrimaryTopology")?;
    let indices = read_indices(topo_children)?;

    // Find the material_index from the first group.
    let material_index = find_elem(topo_children, "Groups")
        .and_then(|groups| match &groups.value {
            ElementValue::ArrayOfReferences(g) | ElementValue::ReferenceArray(g) => g.first(),
            _ => None,
        })
        .and_then(|first_group| find_i32(first_group, "MaterialIndex"))
        .map(|v| v as u32);

    let albedo_texture = albedo_texture(mesh_fields, material_index.unwrap_or(0) as usize);

    Some(Mesh {
        name,
        positions,
        normals,
        uvs,
        indices,
        bbox_min,
        bbox_max,
        material_index,
        albedo_texture,
    })
}

/// Colour texture of the `binding`-th material bound to a mesh.
///
/// A topology group's `MaterialIndex` counts into the mesh's own
/// `MaterialBindings`, not into the file's `Materials` or `Textures`
/// arrays.  The bound material lists its inputs in `Maps`, each a
/// `Usage` name beside a `Map` node whose `Texture` names the file.
fn albedo_texture(mesh_fields: &[Element], binding: usize) -> Option<String> {
    let material = find_reference_children(
        groups_of(mesh_fields, "MaterialBindings")?.get(binding)?,
        "Material",
    )?;
    let albedo = groups_of(material, "Maps")?
        .iter()
        .find(|m| find_string(m, "Usage").as_deref() == Some("AlbedoTexture"))?;
    let file = find_reference_children(albedo, "Map")?;
    find_string(find_reference_children(file, "Texture")?, "FromFileName")
}

/// Entries of a named array-of-struct field.
fn groups_of<'a>(elements: &'a [Element], name: &str) -> Option<&'a [Vec<Element>]> {
    match &find_elem(elements, name)?.value {
        ElementValue::ArrayOfReferences(g) | ElementValue::ReferenceArray(g) => Some(g),
        _ => None,
    }
}

/// Read an indices array from the `PrimaryTopology` struct, handling
/// both `Indices16` (u16 / ReferenceArray of single-field structs) and
/// `Indices` (u32 / same layout with u32 fields).
fn read_indices(topology_fields: &[Element]) -> Option<Vec<u32>> {
    // Try 16-bit first.
    if let Some(ix16) = find_elem(topology_fields, "Indices16") {
        if let Some(v) = read_int_array_flat(ix16) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    if let Some(ix32) = find_elem(topology_fields, "Indices") {
        if let Some(v) = read_int_array_flat(ix32) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Flatten a Granny ReferenceArray whose inner structs are a single
/// integer field into a `Vec<u32>`.
///
/// Granny represents typed integer arrays as "array of struct with one
/// int field"; the walker produces `ArrayOfReferences` with each inner
/// `Vec<Element>` holding one `I32` (or for 16-bit types, an `Opaque(15)`
/// since our walker advances the cursor past int16 without fully
/// decoding it).  For the 16-bit case we can't read values via the
/// generic walker; we'd need to decode from the raw sector data,
/// so for now we return None when we hit that.
fn read_int_array_flat(arr: &Element) -> Option<Vec<u32>> {
    match &arr.value {
        ElementValue::ArrayOfReferences(groups) | ElementValue::ReferenceArray(groups) => {
            let mut out = Vec::with_capacity(groups.len());
            for g in groups {
                for e in g {
                    if let ElementValue::I32(v) = e.value {
                        out.push(v as u32);
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

// ── Tree helpers ─────────────────────────────────────────────────────────────

fn find_elem<'a>(elements: &'a [Element], name: &str) -> Option<&'a Element> {
    elements.iter().find(|e| e.name == name)
}

/// Find a named child and unwrap its inner `Reference` to the
/// contained field list.  Granny uses Reference for inline struct
/// fields like `PrimaryVertexData` and `PrimaryTopology`.
fn find_reference_children<'a>(elements: &'a [Element], name: &str) -> Option<&'a [Element]> {
    find_elem(elements, name).and_then(|e| match &e.value {
        ElementValue::Reference(c) => Some(c.as_slice()),
        _ => None,
    })
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

fn find_i32(elements: &[Element], name: &str) -> Option<i32> {
    elements.iter().find_map(|e| {
        if e.name == name {
            if let ElementValue::I32(v) = &e.value {
                return Some(*v);
            }
        }
        None
    })
}

/// Read a fixed-size f32 array by name.  Pads missing components with
/// zero and truncates anything past four components.  Granny stores
/// Position/Normal/Tangent as Real32[4] and UV as Real32[2] on D2R.
fn find_f32_array(elements: &[Element], name: &str) -> Option<[f32; 4]> {
    elements.iter().find_map(|e| {
        if e.name != name {
            return None;
        }
        let items = match &e.value {
            ElementValue::Array(items) => items,
            _ => return None,
        };
        let mut out = [0.0f32; 4];
        for (i, v) in items.iter().take(4).enumerate() {
            if let ElementValue::F32(f) = v {
                out[i] = *f;
            }
        }
        Some(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(name: &str, value: &str) -> Element {
        Element {
            name: name.to_string(),
            value: ElementValue::String(value.to_string()),
        }
    }

    fn reference(name: &str, children: Vec<Element>) -> Element {
        Element {
            name: name.to_string(),
            value: ElementValue::Reference(children),
        }
    }

    fn array(name: &str, entries: Vec<Vec<Element>>) -> Element {
        Element {
            name: name.to_string(),
            value: ElementValue::ReferenceArray(entries),
        }
    }

    /// One entry of a material's `Maps`: a usage name and the file
    /// node that carries the texture.
    fn map(usage: &str, file: &str) -> Vec<Element> {
        vec![
            string("Usage", usage),
            reference(
                "Map",
                vec![
                    string("Name", "file1"),
                    reference("Texture", vec![string("FromFileName", file)]),
                ],
            ),
        ]
    }

    fn binding(name: &str, maps: Vec<Vec<Element>>) -> Vec<Element> {
        vec![reference(
            "Material",
            vec![
                string("Name", name),
                array("Maps", maps),
                reference("Texture", vec![]),
            ],
        )]
    }

    #[test]
    fn the_albedo_map_of_the_bound_material_is_the_mesh_texture() {
        let mesh = vec![array(
            "MaterialBindings",
            vec![binding(
                "body",
                vec![
                    map("NormalTexture", "data/hd/cain1_body_nrm.texture"),
                    map("AlbedoTexture", "data/hd/cain1_body_alb.texture"),
                    map("ORMTexture", "data/hd/cain1_body_orm.texture"),
                ],
            )],
        )];
        assert_eq!(
            albedo_texture(&mesh, 0).as_deref(),
            Some("data/hd/cain1_body_alb.texture")
        );
    }

    #[test]
    fn the_material_index_picks_the_binding() {
        let mesh = vec![array(
            "MaterialBindings",
            vec![
                binding("a", vec![map("AlbedoTexture", "a_alb.texture")]),
                binding("b", vec![map("AlbedoTexture", "b_alb.texture")]),
            ],
        )];
        assert_eq!(albedo_texture(&mesh, 1).as_deref(), Some("b_alb.texture"));
    }

    #[test]
    fn a_mesh_without_an_albedo_map_has_no_texture() {
        let no_albedo = vec![array(
            "MaterialBindings",
            vec![binding("a", vec![map("NormalTexture", "a_nrm.texture")])],
        )];
        assert_eq!(albedo_texture(&no_albedo, 0), None);
        assert_eq!(albedo_texture(&[], 0), None);
        assert_eq!(albedo_texture(&no_albedo, 5), None);
    }
}
