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

/// D3 is Z-up, glTF is Y-up.
fn y_up(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn y_up_rotation(q: [f32; 4]) -> [f32; 4] {
    [q[0], q[2], -q[1], q[3]]
}

type Mat = [[f32; 4]; 4];

fn matrix(q: [f32; 4], t: [f32; 3]) -> Mat {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
            t[0],
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
            t[1],
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
            t[2],
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// Inverse of a rotation plus translation.
fn rigid_inverse(m: &Mat) -> Mat {
    let mut r = [[0.0; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = m[j][i];
        }
        r[i][3] = -(0..3).map(|k| m[k][i] * m[k][3]).sum::<f32>();
    }
    r[3][3] = 1.0;
    r
}

fn num(v: f32) -> String {
    if v == 0.0 || !v.is_finite() {
        "0".into()
    } else {
        format!("{v}")
    }
}

fn nums(v: &[f32]) -> String {
    v.iter().map(|&x| num(x)).collect::<Vec<_>>().join(",")
}

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

struct Bin {
    data: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl Bin {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let target = target
            .map(|t| format!(",\"target\":{t}"))
            .unwrap_or_default();
        self.views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{},\"byteLength\":{}{target}}}",
            self.data.len(),
            bytes.len()
        ));
        self.data.extend_from_slice(bytes);
        self.views.len() - 1
    }
    fn accessor(
        &mut self,
        bytes: &[u8],
        component: u32,
        count: usize,
        kind: &str,
        target: Option<u32>,
        bounds: Option<(&[f32], &[f32])>,
    ) -> usize {
        let view = self.view(bytes, target);
        let bounds = bounds
            .map(|(lo, hi)| format!(",\"min\":[{}],\"max\":[{}]", nums(lo), nums(hi)))
            .unwrap_or_default();
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":{component},\"count\":{count},\"type\":\"{kind}\"{bounds}}}"
        ));
        self.accessors.len() - 1
    }
}

const FLOAT: u32 = 5126;
const USHORT: u32 = 5123;
const UINT: u32 = 5125;
const ARRAY: u32 = 34962;
const ELEMENTS: u32 = 34963;

fn floats(v: impl IntoIterator<Item = f32>) -> Vec<u8> {
    v.into_iter().flat_map(f32::to_le_bytes).collect()
}

/// Every part as its own skinned mesh on the model's skeleton, with the
/// given PNG colour maps (by material name) embedded, as binary glTF.
pub fn to_glb(app: &Appearance, textures: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut bin = Bin {
        data: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
    };
    let skinned = !app.bones.is_empty();

    // Bones, converted to Y-up, and their inverse bind matrices.
    let mut world: Vec<Mat> = Vec::with_capacity(app.bones.len());
    let mut nodes: Vec<String> = Vec::new();
    for b in &app.bones {
        let (q, t) = (y_up_rotation(b.rotation), y_up(b.translation));
        let local = matrix(q, t);
        let w = match usize::try_from(b.parent).ok().and_then(|p| world.get(p)) {
            Some(parent) => mul(parent, &local),
            None => local,
        };
        world.push(w);
        nodes.push(format!(
            "{{\"name\":{},\"rotation\":[{}],\"translation\":[{}]",
            quote(&b.name),
            nums(&q),
            nums(&t)
        ));
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); app.bones.len()];
    for (i, b) in app.bones.iter().enumerate() {
        if let Some(p) = usize::try_from(b.parent)
            .ok()
            .filter(|&p| p < app.bones.len())
        {
            children[p].push(i);
        }
    }
    for (n, kids) in nodes.iter_mut().zip(&children) {
        if !kids.is_empty() {
            let list: Vec<String> = kids.iter().map(usize::to_string).collect();
            n.push_str(&format!(",\"children\":[{}]", list.join(",")));
        }
        n.push('}');
    }
    let ibm = skinned.then(|| {
        let bytes = floats(world.iter().flat_map(|m| {
            let inv = rigid_inverse(m);
            (0..16).map(move |k| inv[k % 4][k / 4])
        }));
        bin.accessor(&bytes, FLOAT, app.bones.len(), "MAT4", None, None)
    });

    // Materials with a colour map.
    let mut images = Vec::new();
    let mut materials = Vec::new();
    for (name, png) in textures {
        let view = bin.view(png, None);
        images.push(format!(
            "{{\"bufferView\":{view},\"mimeType\":\"image/png\"}}"
        ));
        materials.push(format!(
            "{{\"name\":{},\"pbrMetallicRoughness\":{{\"baseColorTexture\":{{\"index\":{}}},\"metallicFactor\":0,\"roughnessFactor\":0.9}}}}",
            quote(name),
            images.len() - 1
        ));
    }

    let mut meshes = Vec::new();
    let mut mesh_nodes = Vec::new();
    for part in &app.parts {
        let pos: Vec<[f32; 3]> = part.positions.iter().map(|&p| y_up(p)).collect();
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &pos {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if pos.is_empty() {
            lo = [0.0; 3];
            hi = [0.0; 3];
        }
        let n = pos.len();
        let a_pos = bin.accessor(
            &floats(pos.iter().flatten().copied()),
            FLOAT,
            n,
            "VEC3",
            Some(ARRAY),
            Some((&lo, &hi)),
        );
        let a_uv = bin.accessor(
            &floats(part.uvs.iter().flatten().copied()),
            FLOAT,
            n,
            "VEC2",
            Some(ARRAY),
            None,
        );
        let mut attrs = format!("\"POSITION\":{a_pos},\"TEXCOORD_0\":{a_uv}");
        if skinned && part.weights.len() == n {
            let mut joints = Vec::with_capacity(n * 8);
            let mut weights = Vec::with_capacity(n * 4);
            for w in &part.weights {
                let total: f32 = w.iter().map(|p| p.1.max(0.0)).sum();
                let total = if total > 0.0 { total } else { 1.0 };
                for k in 0..4 {
                    let (bone, wt) = w.get(k).copied().unwrap_or((0, 0.0));
                    let bone = if (bone as usize) < app.bones.len() {
                        bone
                    } else {
                        0
                    };
                    joints.extend_from_slice(&(bone as u16).to_le_bytes());
                    weights.push(wt.max(0.0) / total);
                }
            }
            let a_j = bin.accessor(&joints, USHORT, n, "VEC4", Some(ARRAY), None);
            let a_w = bin.accessor(&floats(weights), FLOAT, n, "VEC4", Some(ARRAY), None);
            attrs.push_str(&format!(",\"JOINTS_0\":{a_j},\"WEIGHTS_0\":{a_w}"));
        }
        let idx: Vec<u8> = part.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let a_idx = bin.accessor(
            &idx,
            UINT,
            part.indices.len(),
            "SCALAR",
            Some(ELEMENTS),
            None,
        );
        let material = textures
            .iter()
            .position(|(name, _)| *name == part.material)
            .map(|m| format!(",\"material\":{m}"))
            .unwrap_or_default();
        meshes.push(format!(
            "{{\"name\":{},\"primitives\":[{{\"attributes\":{{{attrs}}},\"indices\":{a_idx}{material}}}]}}",
            quote(&part.material)
        ));
        let skin = if skinned { ",\"skin\":0" } else { "" };
        nodes.push(format!(
            "{{\"name\":{},\"mesh\":{}{skin}}}",
            quote(&part.material),
            meshes.len() - 1
        ));
        mesh_nodes.push(nodes.len() - 1);
    }

    let mut scene: Vec<String> = app
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| b.parent < 0 || b.parent as usize >= app.bones.len())
        .map(|(i, _)| i.to_string())
        .collect();
    scene.extend(mesh_nodes.iter().map(usize::to_string));

    let mut json = format!(
        "{{\"asset\":{{\"version\":\"2.0\",\"generator\":\"rustydemon\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],\"nodes\":[{}],\"meshes\":[{}]",
        scene.join(","),
        nodes.join(","),
        meshes.join(",")
    );
    if let Some(ibm) = ibm {
        let joints: Vec<String> = (0..app.bones.len()).map(|i| i.to_string()).collect();
        json.push_str(&format!(
            ",\"skins\":[{{\"joints\":[{}],\"inverseBindMatrices\":{ibm}}}]",
            joints.join(",")
        ));
    }
    if !images.is_empty() {
        let tex: Vec<String> = (0..images.len())
            .map(|i| format!("{{\"source\":{i}}}"))
            .collect();
        json.push_str(&format!(
            ",\"images\":[{}],\"textures\":[{}],\"materials\":[{}]",
            images.join(","),
            tex.join(","),
            materials.join(",")
        ));
    }
    while !bin.data.len().is_multiple_of(4) {
        bin.data.push(0);
    }
    json.push_str(&format!(
        ",\"accessors\":[{}],\"bufferViews\":[{}],\"buffers\":[{{\"byteLength\":{}}}]}}",
        bin.accessors.join(","),
        bin.views.join(","),
        bin.data.len()
    ));
    let mut json = json.into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }

    let total = 12 + 8 + json.len() + 8 + bin.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin.data);
    out
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
        let mut names: Vec<String> = Vec::new();
        for p in &app.parts {
            if !names.contains(&p.material) {
                names.push(p.material.clone());
            }
        }
        let maps: Vec<(String, Option<super::d3_tex::Image>)> = names
            .into_iter()
            .map(|n| {
                let img = colour_map(&app, &n, fetch);
                (n, img)
            })
            .collect();

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
                Ok(to_glb(&app, &pngs))
            }),
        });
        out
    }
}
