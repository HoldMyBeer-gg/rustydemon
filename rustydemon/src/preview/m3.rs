//! StarCraft II and Heroes of the Storm models (`.m3`).
//!
//! The file is a header, a run of chunks, and an index table at the end.
//! Every chunk has a four letter tag (stored back to front), a version and
//! an entry count.  A structure points at another chunk with a reference:
//! how many entries, and the chunk's number in the index.
//!
//! ```text
//! header   "MD34" back to front, index offset, index length, model ref
//! MODL     the model: name, bones, vertex flags, vertices, divisions
//! U8__     vertex bytes; the layout follows the vertex flags
//! DIV_     faces (u16), regions, batches
//! REGN     a run of vertices and a run of face indices; the indices
//!          count from the region's first vertex
//! BAT_     which region is drawn with which material reference
//! MATM     material reference: kind and number; kind 1 is MAT_
//! MAT_     standard material; its first layer is the colour texture
//! LAYR     a layer; holds the texture path
//! BONE     name, parent, rest translation, rotation and scale
//! ```
//!
//! A vertex is position (3 f32), four bone weights (u8), four bone lookup
//! slots (u8), normal (3 u8 and a sign), an optional colour (4 u8), one to
//! five uvs (2 i16 each) and a tangent (4 u8).  A lookup slot counts from
//! the region's first entry in the model's bone lookup table.
//!
//! Field layouts follow the structure tables the community's Blender tools
//! publish; they were checked here against models from a live install.
//! Every offset and count is checked against the file before use.

use std::sync::Arc;

use super::gltf;
use super::{ExportAction, Mesh3dCpu, MeshBatch, PreviewOutput, PreviewPlugin, SiblingFetcher};

/// A model, as far as the preview and the export need it.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Up to four bones and weights for each vertex.  A weight of zero
    /// means the pair is unused.
    pub skin: Vec<[(u16, f32); 4]>,
    /// Triangle corners as vertex numbers, every part's run after the last.
    pub indices: Vec<u32>,
    pub parts: Vec<Part>,
    pub bones: Vec<Bone>,
    pub materials: Vec<Material>,
}

/// One draw: a run of `indices` and the material it uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub first_index: u32,
    pub index_count: u32,
    /// Number of the material in [`Model::materials`], if it is a standard
    /// material.
    pub material: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: String,
    /// Path of the colour texture as the file spells it.
    pub diffuse: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Rest pose, relative to the parent.
    pub translation: [f32; 3],
    /// Rest rotation as x, y, z, w.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// `MD34` as it is stored.
const MAGIC: &[u8; 4] = b"43DM";

/// Vertex flag: a colour follows the normal.
const HAS_COLOUR: u32 = 0x200;
/// Vertex flags that each add one uv to the first.
const EXTRA_UVS: u32 = 0x4000_0000 | 0x10_0000 | 0x8_0000 | 0x4_0000;

/// uv units to a texture coordinate, where a region gives no factor.
const UV_SCALE: f32 = 1.0 / 2048.0;

pub fn claims(filename: &str, data: &[u8]) -> bool {
    filename.to_ascii_lowercase().ends_with(".m3") && data.starts_with(MAGIC)
}

// ── Reading ──────────────────────────────────────────────────────────────

struct Entry {
    tag: [u8; 4],
    offset: usize,
    count: usize,
    version: u32,
}

/// A reference to a chunk.
#[derive(Clone, Copy)]
struct Ref {
    entries: usize,
    chunk: usize,
}

/// A chunk's bytes, cut to the entries a reference asked for.
struct Chunk<'a> {
    bytes: &'a [u8],
    version: u32,
    entries: usize,
}

struct File<'a> {
    data: &'a [u8],
    index: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("read of 2 bytes at {at} is past the end"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("read of 4 bytes at {at} is past the end"))
}

fn f32_at(b: &[u8], at: usize) -> Result<f32, String> {
    u32_at(b, at).map(f32::from_bits)
}

fn vec3_at(b: &[u8], at: usize) -> Result<[f32; 3], String> {
    Ok([f32_at(b, at)?, f32_at(b, at + 4)?, f32_at(b, at + 8)?])
}

fn ref_at(b: &[u8], at: usize) -> Result<Ref, String> {
    Ok(Ref {
        entries: u32_at(b, at)? as usize,
        chunk: u32_at(b, at + 4)? as usize,
    })
}

fn stored_tag(tag: &str) -> [u8; 4] {
    let mut t = [0u8; 4];
    t.copy_from_slice(tag.as_bytes());
    t.reverse();
    t
}

impl<'a> File<'a> {
    fn open(data: &'a [u8]) -> Result<Self, String> {
        if !data.starts_with(MAGIC) {
            return Err("not an MD34 model".into());
        }
        let offset = u32_at(data, 4)? as usize;
        let count = u32_at(data, 8)? as usize;
        let table = count
            .checked_mul(16)
            .and_then(|len| offset.checked_add(len))
            .and_then(|end| data.get(offset..end))
            .ok_or("the index table is past the end of the file")?;
        let index = table
            .as_chunks::<16>()
            .0
            .iter()
            .map(|e| Entry {
                tag: [e[0], e[1], e[2], e[3]],
                offset: u32::from_le_bytes([e[4], e[5], e[6], e[7]]) as usize,
                count: u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize,
                version: u32::from_le_bytes([e[12], e[13], e[14], e[15]]),
            })
            .collect();
        Ok(Self { data, index })
    }

    /// The entries a reference points at, `size` bytes each.  `None` for
    /// an empty reference; an error if it names the wrong kind of chunk or
    /// reaches past the file.
    fn chunk(&self, r: Ref, tag: &str, size: usize) -> Result<Option<Chunk<'a>>, String> {
        if r.entries == 0 {
            return Ok(None);
        }
        let entry = self
            .index
            .get(r.chunk)
            .ok_or_else(|| format!("{tag}: chunk {} is not in the index", r.chunk))?;
        if entry.tag != stored_tag(tag) {
            return Err(format!("chunk {} is not {tag}", r.chunk));
        }
        if r.entries > entry.count {
            return Err(format!(
                "{tag}: {} entries asked of a chunk of {}",
                r.entries, entry.count
            ));
        }
        let bytes = r
            .entries
            .checked_mul(size)
            .and_then(|len| entry.offset.checked_add(len))
            .and_then(|end| self.data.get(entry.offset..end))
            .ok_or_else(|| format!("{tag}: chunk {} is past the end of the file", r.chunk))?;
        Ok(Some(Chunk {
            bytes,
            version: entry.version,
            entries: r.entries,
        }))
    }

    /// Like [`chunk`](Self::chunk), where the entry size depends on the
    /// chunk's version.  `None` too when the version is not known.
    fn versioned(
        &self,
        r: Ref,
        tag: &str,
        size: impl Fn(u32) -> Option<usize>,
    ) -> Result<Option<Chunk<'a>>, String> {
        let Some(version) = self.index.get(r.chunk).map(|e| e.version) else {
            return self.chunk(r, tag, 1);
        };
        match size(version) {
            Some(size) => self.chunk(r, tag, size),
            None => Ok(None),
        }
    }

    /// A reference to the whole of the first chunk with this tag.
    fn first(&self, tag: &str) -> Option<Ref> {
        let stored = stored_tag(tag);
        self.index
            .iter()
            .position(|e| e.tag == stored)
            .map(|chunk| Ref {
                entries: self.index[chunk].count,
                chunk,
            })
    }

    /// Text a reference points at, up to its first zero byte.
    fn text(&self, r: Ref) -> Result<String, String> {
        let Some(chunk) = self.chunk(r, "CHAR", 1)? else {
            return Ok(String::new());
        };
        let end = chunk.bytes.iter().position(|&b| b == 0);
        let bytes = &chunk.bytes[..end.unwrap_or(chunk.bytes.len())];
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

// ── Parsing ──────────────────────────────────────────────────────────────

/// A region: a run of vertices and a run of face indices.
struct Region {
    first_vertex: usize,
    vertex_count: usize,
    first_index: usize,
    index_count: usize,
    first_lookup: usize,
    /// uv units to a texture coordinate, and what is added after.
    uv_scale: f32,
    uv_offset: f32,
}

fn region_size(version: u32) -> Option<usize> {
    match version {
        2 => Some(28),
        3 => Some(36),
        4 => Some(40),
        5 => Some(48),
        _ => None,
    }
}

fn regions(chunk: &Chunk) -> Result<Vec<Region>, String> {
    let size = region_size(chunk.version).ok_or("region version is not known")?;
    let mut out = Vec::with_capacity(chunk.entries);
    for r in chunk.bytes.chunks_exact(size) {
        // Version 2 has one less field ahead and keeps the vertex run in
        // 16 bits.
        let (first_vertex, vertex_count, rest) = if chunk.version == 2 {
            (u16_at(r, 4)? as usize, u16_at(r, 6)? as usize, 8)
        } else {
            (u32_at(r, 8)? as usize, u32_at(r, 12)? as usize, 16)
        };
        let (uv_scale, uv_offset) = if chunk.version >= 5 {
            let factor = f32_at(r, 40)?;
            let scale = if factor == 0.0 {
                UV_SCALE
            } else {
                factor / 32768.0
            };
            (scale, f32_at(r, 44)?)
        } else {
            (UV_SCALE, 0.0)
        };
        out.push(Region {
            first_vertex,
            vertex_count,
            first_index: u32_at(r, rest)? as usize,
            index_count: u32_at(r, rest + 4)? as usize,
            first_lookup: u16_at(r, rest + 10)? as usize,
            uv_scale,
            uv_offset,
        });
    }
    Ok(out)
}

fn material_size(version: u32) -> Option<usize> {
    match version {
        15 => Some(268),
        16..=18 => Some(280),
        19 => Some(340),
        20 => Some(352),
        _ => None,
    }
}

fn materials(file: &File) -> Result<Vec<Material>, String> {
    let Some(all) = file.first("MAT_") else {
        return Ok(Vec::new());
    };
    let Some(chunk) = file.versioned(all, "MAT_", material_size)? else {
        return Ok(Vec::new());
    };
    let size = chunk.bytes.len() / chunk.entries;
    // Three floats were added ahead of the layers in version 20.  The one
    // byte field before them is padded to four.
    let diffuse_at = if chunk.version >= 20 { 64 } else { 52 };
    chunk
        .bytes
        .chunks_exact(size)
        .map(|m| {
            let name = file.text(ref_at(m, 0)?)?;
            // A layer that cannot be read leaves the material untextured.
            let diffuse = file
                .chunk(ref_at(m, diffuse_at)?, "LAYR", 16)
                .ok()
                .flatten()
                .and_then(|layer| ref_at(layer.bytes, 4).ok())
                .and_then(|path| file.text(path).ok())
                .filter(|path| !path.is_empty());
            Ok(Material { name, diffuse })
        })
        .collect()
}

fn bones(file: &File, r: Ref) -> Result<Vec<Bone>, String> {
    let Some(chunk) = file.chunk(r, "BONE", 160)? else {
        return Ok(Vec::new());
    };
    chunk
        .bytes
        .as_chunks::<160>()
        .0
        .iter()
        .map(|b| {
            let b = b.as_slice();
            let parent = u16_at(b, 20)? as i16;
            Ok(Bone {
                name: file.text(ref_at(b, 4)?)?,
                parent: usize::try_from(parent).ok().filter(|&p| p < chunk.entries),
                translation: vec3_at(b, 32)?,
                rotation: [
                    f32_at(b, 68)?,
                    f32_at(b, 72)?,
                    f32_at(b, 76)?,
                    f32_at(b, 80)?,
                ],
                scale: vec3_at(b, 112)?,
            })
        })
        .collect()
}

pub fn parse(data: &[u8]) -> Result<Model, String> {
    let file = File::open(data)?;
    let modl = file
        .chunk(ref_at(data, 12)?, "MODL", 136)?
        .ok_or("the file has no model")?
        .bytes;

    let name = file.text(ref_at(modl, 0)?)?;
    let bones = bones(&file, ref_at(modl, 80)?)?;
    let flags = u32_at(modl, 96)?;

    // ── Vertices ─────────────────────────────────────────────────────
    let uv_count = 1 + (flags & EXTRA_UVS).count_ones() as usize;
    let colour = if flags & HAS_COLOUR != 0 { 4 } else { 0 };
    let uv_at = 24 + colour;
    let stride = uv_at + 4 * uv_count + 4;
    let vertex_bytes = file
        .chunk(ref_at(modl, 100)?, "U8__", 1)?
        .map_or(&[][..], |c| c.bytes);
    if !vertex_bytes.len().is_multiple_of(stride) {
        return Err(format!(
            "{} vertex bytes do not divide by the {stride} of flags {flags:#x}",
            vertex_bytes.len()
        ));
    }
    let count = vertex_bytes.len() / stride;
    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count);
    let mut raw_uvs = Vec::with_capacity(count);
    let mut raw_skin = Vec::with_capacity(count);
    for v in vertex_bytes.chunks_exact(stride) {
        positions.push(vec3_at(v, 0)?);
        raw_skin.push(([v[12], v[13], v[14], v[15]], [v[16], v[17], v[18], v[19]]));
        normals.push([20, 21, 22].map(|at| f32::from(v[at]) / 255.0 * 2.0 - 1.0));
        raw_uvs.push([
            i16::from_le_bytes([v[uv_at], v[uv_at + 1]]),
            i16::from_le_bytes([v[uv_at + 2], v[uv_at + 3]]),
        ]);
    }
    let mut uvs: Vec<[f32; 2]> = raw_uvs
        .iter()
        .map(|uv| uv.map(|c| f32::from(c) * UV_SCALE))
        .collect();
    let mut skin = vec![[(0u16, 0f32); 4]; count];

    // ── Regions and batches ──────────────────────────────────────────
    let lookup = file
        .chunk(ref_at(modl, 124)?, "U16_", 2)?
        .map_or(&[][..], |c| c.bytes);
    let lookup: Vec<u16> = lookup
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();

    let materials = materials(&file)?;
    let material_refs = match file.first("MATM") {
        Some(all) => file.chunk(all, "MATM", 8)?.map_or(&[][..], |c| c.bytes),
        None => &[][..],
    };

    let mut indices = Vec::new();
    let mut parts = Vec::new();
    if let Some(div) = file.chunk(ref_at(modl, 112)?, "DIV_", 52)? {
        let div = div.bytes;
        let faces = file
            .chunk(ref_at(div, 0)?, "U16_", 2)?
            .map_or(&[][..], |c| c.bytes);
        let regions = match file.versioned(ref_at(div, 12)?, "REGN", region_size)? {
            Some(chunk) => regions(&chunk)?,
            None => Vec::new(),
        };

        for region in &regions {
            let vertices = region
                .first_vertex
                .checked_add(region.vertex_count)
                .filter(|&end| end <= count)
                .map(|end| region.first_vertex..end)
                .ok_or("a region's vertices are past the end of the vertex list")?;
            for v in vertices {
                uvs[v] = raw_uvs[v].map(|c| f32::from(c) * region.uv_scale + region.uv_offset);
                let (weights, slots) = raw_skin[v];
                for k in 0..4 {
                    let bone = lookup
                        .get(region.first_lookup + slots[k] as usize)
                        .copied()
                        .unwrap_or(0);
                    skin[v][k] = (bone, f32::from(weights[k]) / 255.0);
                }
            }
        }

        let batches = file
            .chunk(ref_at(div, 24)?, "BAT_", 14)?
            .map_or(&[][..], |c| c.bytes);
        for batch in batches.as_chunks::<14>().0 {
            let batch = batch.as_slice();
            let region = regions
                .get(u16_at(batch, 4)? as usize)
                .ok_or("a batch names a region that is not there")?;
            let corners = region
                .first_index
                .checked_add(region.index_count)
                .and_then(|end| faces.get(region.first_index * 2..end * 2))
                .ok_or("a region's faces are past the end of the face list")?;
            let first_index = indices.len() as u32;
            for corner in corners.as_chunks::<2>().0 {
                let corner = u16::from_le_bytes(*corner) as usize;
                if corner >= region.vertex_count {
                    return Err(format!(
                        "face index {corner} is past the region's {} vertices",
                        region.vertex_count
                    ));
                }
                indices.push((region.first_vertex + corner) as u32);
            }
            // Kind 1 is a standard material; the others are not drawn
            // with a colour texture here.
            let material = material_refs
                .as_chunks::<8>()
                .0
                .get(u16_at(batch, 10)? as usize)
                .map(<[u8; 8]>::as_slice)
                .filter(|m| u32_at(m, 0) == Ok(1))
                .and_then(|m| u32_at(m, 4).ok())
                .map(|n| n as usize)
                .filter(|&n| n < materials.len());
            parts.push(Part {
                first_index,
                index_count: indices.len() as u32 - first_index,
                material,
            });
        }
    }

    Ok(Model {
        name,
        positions,
        normals,
        uvs,
        skin,
        indices,
        parts,
        bones,
        materials,
    })
}

// ── Preview ──────────────────────────────────────────────────────────────

/// The model as the 3D viewer draws it: one batch per part, in flat
/// colours.
pub fn to_mesh(model: &Model) -> Mesh3dCpu {
    let mut bbox_min = [f32::MAX; 3];
    let mut bbox_max = [f32::MIN; 3];
    for p in &model.positions {
        for (k, c) in p.iter().enumerate() {
            bbox_min[k] = bbox_min[k].min(*c);
            bbox_max[k] = bbox_max[k].max(*c);
        }
    }
    if model.positions.is_empty() {
        bbox_min = [0.0; 3];
        bbox_max = [0.0; 3];
    }
    Mesh3dCpu {
        positions: model.positions.clone(),
        uvs: model.uvs.clone(),
        indices: model.indices.clone(),
        bbox_min,
        bbox_max,
        batches: model
            .parts
            .iter()
            .map(|p| MeshBatch {
                start_index: p.first_index,
                index_count: p.index_count,
                material_id: u32::MAX,
                blend: Default::default(),
            })
            .collect(),
        materials: Vec::new(),
    }
}

/// The text shown beside the model.
pub fn summary(model: &Model) -> String {
    let mut text = format!(
        "StarCraft II model\n\n\
         Vertices     {}\n\
         Triangles    {}\n\
         Parts        {}\n\
         Bones        {}\n\
         Materials    {}\n",
        model.positions.len(),
        model.indices.len() / 3,
        model.parts.len(),
        model.bones.len(),
        model.materials.len(),
    );
    for m in &model.materials {
        let texture = m.diffuse.as_deref().unwrap_or("no colour texture");
        text.push_str(&format!("  {}: {texture}\n", m.name));
    }
    if !model.name.is_empty() {
        text.push_str(&format!("\nSource       {}\n", model.name));
    }
    text
}

/// The model for the shared glTF writer: the skeleton in its rest pose and
/// one part per draw, each with only the vertices it uses.
///
/// `filename` names the parts, since the name stored in the file is the
/// path of the artist's source scene.
pub fn to_gltf(model: &Model, filename: &str) -> gltf::Model {
    let stem = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .rsplit_once('.')
        .map_or(filename, |(stem, _)| stem);
    let bones: Vec<gltf::Bone> = model
        .bones
        .iter()
        .map(|b| gltf::Bone {
            name: b.name.clone(),
            parent: b.parent,
            rotation: b.rotation,
            translation: b.translation,
            scale: b.scale,
        })
        .collect();

    let parts = model
        .parts
        .iter()
        .enumerate()
        .map(|(n, part)| {
            let first = part.first_index as usize;
            let corners = &model.indices[first..first + part.index_count as usize];
            // Old vertex number to its number in this part.
            let mut renumbered = std::collections::HashMap::new();
            let mut positions = Vec::new();
            let mut uvs = Vec::new();
            let mut skin = Vec::new();
            let mut indices = Vec::with_capacity(corners.len());
            for &old in corners {
                let new = *renumbered.entry(old).or_insert_with(|| {
                    let v = old as usize;
                    positions.push(model.positions[v]);
                    uvs.push(model.uvs[v]);
                    if !bones.is_empty() {
                        // A pair that names a bone the model lacks is
                        // dropped, so the writer never sees a bad joint.
                        skin.push(model.skin[v].map(|(bone, weight)| {
                            if (bone as usize) < bones.len() {
                                (bone, weight)
                            } else {
                                (0, 0.0)
                            }
                        }));
                    }
                    positions.len() as u32 - 1
                });
                indices.push(new);
            }
            gltf::Part {
                name: format!("{stem}_{n}"),
                material: part.material.map(|m| model.materials[m].name.clone()),
                positions,
                uvs,
                skin,
                indices,
            }
        })
        .collect();

    gltf::Model {
        name: stem.to_owned(),
        bones,
        parts,
        textures: Vec::new(),
        animations: Vec::new(),
        z_up: true,
    }
}

pub struct M3Preview;

impl PreviewPlugin for M3Preview {
    fn name(&self) -> &str {
        ".m3 model"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        claims(filename, data)
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        _siblings: &SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let model = match parse(data) {
            Ok(model) => model,
            Err(e) => {
                out.text = Some(format!("StarCraft II model\n\nCould not be read: {e}"));
                return out;
            }
        };
        out.text = Some(summary(&model));
        if !model.indices.is_empty() {
            let mesh = Arc::new(to_mesh(&model));
            let for_obj = Arc::clone(&mesh);
            out.extra_exports.push(ExportAction {
                label: "Export As OBJ",
                default_extension: "obj",
                filter_name: "Wavefront OBJ",
                build: Arc::new(move |_raw, _path| Ok(super::encode_obj(&for_obj))),
            });
            out.mesh3d = Some(mesh);

            let label = if model.bones.is_empty() {
                "Export As glTF"
            } else {
                "Export As glTF (rigged)"
            };
            let name = filename.to_owned();
            out.extra_exports.push(ExportAction {
                label,
                default_extension: "glb",
                filter_name: "glTF binary",
                build: Arc::new(move |_raw, _path| Ok(gltf::to_glb(&to_gltf(&model, &name)))),
            });
        }
        out
    }
}
