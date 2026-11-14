//! Diablo III Appearance SNOs (`Base/Appearance/*.app`): skinned models.
//!
//! Offsets in the file count from byte 16. The header holds three
//! (count, offset, size) tables and one (offset, size) list:
//!
//! ```text
//! 0x24  bones, 312 bytes each
//!         [64] name, i32 parent (-1 for a root), [40] bounds,
//!         then 8-float transforms (quaternion x y z w, translation, scale):
//!         0x6C world, 0x8C inverse world, 0xAC local to the parent
//! 0xA8  parts, 400 bytes each
//!         +0x04 u32 vertex count, +0x08 vertices (44 bytes each)
//!         +0x18 skin weights (24 bytes each), +0x1C their size
//!         +0x28 u32 index count, +0x2C u16 indices
//!         +0x5C [64] material name
//! 0x1B4 materials, 144 bytes each
//!         [128] name, u32 offset of a parameter block
//!         block +0x08 offset of texture bindings, +0x0C their count;
//!         a binding is 160 bytes with the slot at +0x08 and the texture
//!         SNO at +0x10; slot 1 is the colour map
//! 0x1C8 looks: offset and size of a list of 64-byte names
//! ```
//!
//! A vertex is a position (3 f32) at 0 and a UV (2 u16, `(v - 0x8000) /
//! 512`) at 24. A skin weight is three (u32 bone, f32 weight) pairs.
//!
//! Parts are alternatives as often as they are pieces: the treasure goblin
//! ships its body with and without its backpack, and the sack lying on the
//! floor. Which parts make up each look is not decoded, so the preview
//! draws the parts whose material is named `normal` when there are any.
//!
//! Some materials keep their bindings in a layout that is not decoded;
//! for those the colour map candidates are every SNO that follows a slot 1
//! marker in the material's data, and the caller keeps the first that
//! opens as a texture.

use std::sync::Arc;

use super::{ExportAction, Mesh3dCpu, MeshBatch, MeshMaterial, PreviewOutput, SiblingFetcher};

const SNO_MAGIC: u32 = 0xDEAD_BEEF;
const DATA_BASE: usize = 16;
const BONE: usize = 312;
const PART: usize = 400;
const MATERIAL: usize = 144;
const LOOK: usize = 64;
const VERTEX: usize = 44;
const WEIGHT: usize = 24;
const BINDING: usize = 160;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    /// Rest pose relative to the parent.
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Empty when the part is not skinned.
    pub weights: Vec<[(u32, f32); 3]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    pub name: String,
    /// Colour map SNO candidates, most likely first.
    pub colour_maps: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Appearance {
    pub bones: Vec<Bone>,
    pub parts: Vec<Part>,
    pub materials: Vec<Material>,
    pub looks: Vec<String>,
}

impl Appearance {
    pub fn material(&self, name: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.name == name)
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn u32(&self, at: usize) -> Result<u32, String> {
        self.0
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
            .ok_or_else(|| format!("truncated at byte {at}"))
    }
    fn u16(&self, at: usize) -> Result<u16, String> {
        self.0
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes(b.try_into().expect("2 bytes")))
            .ok_or_else(|| format!("truncated at byte {at}"))
    }
    fn f32(&self, at: usize) -> Result<f32, String> {
        self.u32(at).map(f32::from_bits)
    }
    fn name(&self, at: usize, len: usize) -> Result<String, String> {
        let b = self
            .0
            .get(at..at + len)
            .ok_or_else(|| format!("truncated at byte {at}"))?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
    /// A (count, offset, size) table at `at`: (count, absolute start).
    fn table(&self, at: usize, each: usize) -> Result<(usize, usize), String> {
        let count = self.u32(at)? as usize;
        let start = DATA_BASE + self.u32(at + 4)? as usize;
        let need = count
            .checked_mul(each)
            .and_then(|n| n.checked_add(start))
            .ok_or("table size overflows")?;
        if count > 0 && need > self.0.len() {
            return Err(format!("table at {at:#x} runs past the end"));
        }
        Ok((count, start))
    }
}

pub fn claims(filename: &str, data: &[u8]) -> bool {
    filename.to_ascii_lowercase().ends_with(".app")
        && data.len() >= 0x1d0
        && data[..4] == SNO_MAGIC.to_le_bytes()
}

pub fn parse(data: &[u8]) -> Result<Appearance, String> {
    let r = Reader(data);
    let mut app = Appearance::default();

    let (count, start) = r.table(0x24, BONE)?;
    for i in 0..count {
        let b = start + i * BONE;
        let f = |k: usize| r.f32(b + 0xAC + k * 4);
        app.bones.push(Bone {
            name: r.name(b, 64)?,
            parent: r.u32(b + 64)? as i32,
            rotation: [f(0)?, f(1)?, f(2)?, f(3)?],
            translation: [f(4)?, f(5)?, f(6)?],
            scale: f(7)?,
        });
    }

    let (count, start) = r.table(0xA8, PART)?;
    for i in 0..count {
        app.parts.push(parse_part(&r, start + i * PART)?);
    }

    let (count, start) = r.table(0x1B4, MATERIAL)?;
    for i in 0..count {
        let e = start + i * MATERIAL;
        let name = r.name(e, 128)?;
        let block = DATA_BASE + r.u32(e + 128)? as usize;
        app.materials.push(Material {
            name,
            colour_maps: colour_maps(&r, block).unwrap_or_default(),
        });
    }

    let looks_at = DATA_BASE + r.u32(0x1C8)? as usize;
    let looks_size = r.u32(0x1CC)? as usize;
    if looks_size > 0 && looks_at + looks_size <= data.len() {
        for i in 0..looks_size / LOOK {
            app.looks.push(r.name(looks_at + i * LOOK, LOOK)?);
        }
    }
    Ok(app)
}

fn parse_part(r: &Reader, p: usize) -> Result<Part, String> {
    let count = r.u32(p + 4)? as usize;
    let verts = DATA_BASE + r.u32(p + 8)? as usize;
    if verts + count * VERTEX > r.0.len() {
        return Err("vertices run past the end".into());
    }
    let mut part = Part {
        material: r.name(p + 0x5C, 64)?,
        ..Default::default()
    };
    for v in 0..count {
        let at = verts + v * VERTEX;
        part.positions
            .push([r.f32(at)?, r.f32(at + 4)?, r.f32(at + 8)?]);
        let uv = |o| -> Result<f32, String> { Ok((r.u16(at + o)? as f32 - 32768.0) / 512.0) };
        part.uvs.push([uv(24)?, uv(26)?]);
    }

    let weights = DATA_BASE + r.u32(p + 0x18)? as usize;
    let weights_size = r.u32(p + 0x1C)? as usize;
    if weights_size == count * WEIGHT && weights + weights_size <= r.0.len() {
        for v in 0..count {
            let at = weights + v * WEIGHT;
            let pair = |k: usize| -> Result<(u32, f32), String> {
                Ok((r.u32(at + k * 8)?, r.f32(at + k * 8 + 4)?))
            };
            part.weights.push([pair(0)?, pair(1)?, pair(2)?]);
        }
    }

    let n = r.u32(p + 0x28)? as usize;
    let idx = DATA_BASE + r.u32(p + 0x2C)? as usize;
    for i in 0..n {
        let v = r.u16(idx + i * 2)? as u32;
        if v as usize >= count {
            return Err(format!("index {v} past {count} vertices"));
        }
        part.indices.push(v);
    }
    Ok(part)
}

/// Colour map candidates for the material whose parameter block is at
/// `block`: slot 1 from the bindings first, then any SNO that follows a
/// slot 1 marker further on.
fn colour_maps(r: &Reader, block: usize) -> Result<Vec<u32>, String> {
    let bind = DATA_BASE + r.u32(block + 8)? as usize;
    let count = r.u32(block + 12)? as usize;
    let mut out = Vec::new();
    for k in 0..count.min(256) {
        let at = bind + k * BINDING;
        if r.u32(at + 8).ok() == Some(1) {
            out.push(r.u32(at + 0x10)?);
            break;
        }
    }
    let end = (bind + 0x4000).min(r.0.len().saturating_sub(12));
    let mut at = bind;
    while at < end {
        if r.u32(at)? == 1 && r.u32(at + 4)? == 0 {
            let sno = r.u32(at + 8)?;
            if sno != 0 && sno != u32::MAX && !out.contains(&sno) {
                out.push(sno);
            }
        }
        at += 4;
    }
    Ok(out)
}

/// The parts the preview draws: those whose material is named `normal`,
/// or every part when none is.
pub fn drawn_parts(app: &Appearance) -> Vec<usize> {
    let normal: Vec<usize> = (0..app.parts.len())
        .filter(|&i| {
            app.parts[i]
                .material
                .to_ascii_lowercase()
                .contains("normal")
        })
        .collect();
    if normal.is_empty() {
        (0..app.parts.len()).collect()
    } else {
        normal
    }
}

// ── glTF export ──────────────────────────────────────────────────────────

/// The model, and the animation when given, in the shared glTF writer's
/// terms.
pub fn to_glb(
    app: &Appearance,
    textures: &[(String, Vec<u8>)],
    anim: Option<&super::d3_ani::Animation>,
) -> Vec<u8> {
    use super::gltf;
    let bones: Vec<gltf::Bone> = app
        .bones
        .iter()
        .map(|b| gltf::Bone {
            name: b.name.clone(),
            parent: usize::try_from(b.parent).ok(),
            rotation: b.rotation,
            translation: b.translation,
            scale: [1.0; 3],
        })
        .collect();
    let parts = app
        .parts
        .iter()
        .map(|p| gltf::Part {
            name: p.material.clone(),
            material: Some(p.material.clone()),
            positions: p.positions.clone(),
            uvs: p.uvs.clone(),
            skin: p
                .weights
                .iter()
                .map(|w| {
                    let pair = |k: usize| (w[k].0.min(u16::MAX as u32) as u16, w[k].1);
                    [pair(0), pair(1), pair(2), (0, 0.0)]
                })
                .collect(),
            indices: p.indices.clone(),
        })
        .collect();
    let mut animations = Vec::new();
    if let Some(anim) = anim {
        let secs = |f: u32| f as f32 / super::d3_ani::KEYS_PER_SECOND;
        let mut channels = Vec::new();
        for track in &anim.bones {
            let Some(bone) = app.bones.iter().position(|b| b.name == track.name) else {
                continue;
            };
            if !track.translation.is_empty() {
                channels.push(gltf::Channel {
                    bone,
                    times: track.translation.iter().map(|k| secs(k.0)).collect(),
                    values: gltf::Values::Translation(
                        track.translation.iter().map(|k| k.1).collect(),
                    ),
                });
            }
            if !track.rotation.is_empty() {
                channels.push(gltf::Channel {
                    bone,
                    times: track.rotation.iter().map(|k| secs(k.0)).collect(),
                    values: gltf::Values::Rotation(track.rotation.iter().map(|k| k.1).collect()),
                });
            }
            if !track.scale.is_empty() {
                channels.push(gltf::Channel {
                    bone,
                    times: track.scale.iter().map(|k| secs(k.0)).collect(),
                    values: gltf::Values::Scale(track.scale.iter().map(|k| [k.1; 3]).collect()),
                });
            }
        }
        animations.push(gltf::Animation {
            name: "animation".into(),
            channels,
        });
    }
    gltf::to_glb(&gltf::Model {
        bones,
        parts,
        textures: textures.to_vec(),
        animations,
        z_up: true,
    })
}

// ── Preview ──────────────────────────────────────────────────────────────

/// Decoded colour map of a material: the first candidate that opens as a
/// texture.
fn colour_map(
    app: &Appearance,
    material: &str,
    fetch: &SiblingFetcher<'_>,
) -> Option<super::d3_tex::Image> {
    let m = app.material(material)?;
    m.colour_maps.iter().find_map(|sno| {
        let data = (fetch.by_name)(&format!("sno/{sno}"))?;
        super::d3_tex::looks_like_d3_texture(&data)
            .then(|| super::d3_tex::decode(&data).ok())
            .flatten()
    })
}

/// The colour map of every material the parts use, decoded once.
pub fn colour_maps_for(
    app: &Appearance,
    fetch: &SiblingFetcher<'_>,
) -> Vec<(String, Option<super::d3_tex::Image>)> {
    let mut names: Vec<String> = Vec::new();
    for p in &app.parts {
        if !names.contains(&p.material) {
            names.push(p.material.clone());
        }
    }
    names
        .into_iter()
        .map(|n| {
            let img = colour_map(app, &n, fetch);
            (n, img)
        })
        .collect()
}

pub struct D3AppPreview;

impl super::PreviewPlugin for D3AppPreview {
    fn name(&self) -> &str {
        "Diablo III model"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        claims(filename, data)
    }

    fn build(
        &self,
        _filename: &str,
        data: &[u8],
        _ctx: &egui::Context,
        fetch: &SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let app = match parse(data) {
            Ok(a) => a,
            Err(e) => {
                out.text = Some(format!("Diablo III model\n\nCould not read it: {e}"));
                return out;
            }
        };

        // One decode per material, shared by the viewer and the export.
        let maps = colour_maps_for(&app, fetch);

        let drawn = drawn_parts(&app);
        let mut cpu = Mesh3dCpu {
            positions: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            bbox_min: [f32::MAX; 3],
            bbox_max: [f32::MIN; 3],
            batches: Vec::new(),
            materials: Vec::new(),
        };
        let mut used: Vec<&str> = Vec::new();
        for &i in &drawn {
            let p = &app.parts[i];
            let base = cpu.positions.len() as u32;
            let start = cpu.indices.len() as u32;
            cpu.positions.extend_from_slice(&p.positions);
            cpu.uvs.extend_from_slice(&p.uvs);
            cpu.indices.extend(p.indices.iter().map(|&k| k + base));
            let material_id = match used.iter().position(|&n| n == p.material) {
                Some(m) => m as u32,
                None => {
                    let img = maps
                        .iter()
                        .find(|(n, _)| *n == p.material)
                        .and_then(|(_, i)| i.as_ref());
                    cpu.materials.push(MeshMaterial {
                        rgba: img.map(|i| i.rgba.clone()),
                        width: img.map_or(0, |i| i.width),
                        height: img.map_or(0, |i| i.height),
                    });
                    used.push(&p.material);
                    (used.len() - 1) as u32
                }
            };
            cpu.batches.push(MeshBatch {
                start_index: start,
                index_count: p.indices.len() as u32,
                material_id,
                blend: Default::default(),
            });
        }
        for p in &cpu.positions {
            for k in 0..3 {
                cpu.bbox_min[k] = cpu.bbox_min[k].min(p[k]);
                cpu.bbox_max[k] = cpu.bbox_max[k].max(p[k]);
            }
        }

        let mut text = format!(
            "Diablo III model: {} bones, {} parts\n\nParts (drawn marked *):\n",
            app.bones.len(),
            app.parts.len()
        );
        for (i, p) in app.parts.iter().enumerate() {
            let mark = if drawn.contains(&i) { "*" } else { " " };
            let tex = match maps.iter().find(|(n, _)| *n == p.material) {
                Some((_, Some(img))) => format!("{}x{} colour map", img.width, img.height),
                _ => "no colour map found".into(),
            };
            text.push_str(&format!(
                " {mark} {} ({} vertices, {})\n",
                p.material,
                p.positions.len(),
                tex
            ));
        }
        if !app.looks.is_empty() {
            text.push_str(&format!("\nLooks: {}\n", app.looks.join(", ")));
        }
        out.text = Some(text);
        if !cpu.positions.is_empty() {
            out.mesh3d = Some(Arc::new(cpu));
        }

        let maps = Arc::new(maps);
        out.extra_exports.push(ExportAction {
            label: "Export As glTF (rigged)",
            default_extension: "glb",
            filter_name: "glTF binary",
            build: Arc::new(move |data, _path| {
                let app = parse(data)?;
                let mut pngs = Vec::new();
                for (name, img) in maps.iter() {
                    if let Some(img) = img {
                        pngs.push((
                            name.clone(),
                            super::encode_png(&img.rgba, img.width, img.height)?,
                        ));
                    }
                }
                Ok(to_glb(&app, &pngs, None))
            }),
        });
        out
    }
}
