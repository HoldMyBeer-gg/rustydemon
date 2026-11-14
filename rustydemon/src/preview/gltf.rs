//! Binary glTF (.glb) writer shared by the model exports.
//!
//! A caller describes its model in plain terms (a skeleton of local bone
//! transforms, parts with up to four bone weights per vertex, PNG colour
//! maps by material name, and animations as per-bone channels with times
//! in seconds) and gets one self-contained .glb back. Formats that are
//! Z-up set [`Model::z_up`] and are turned to glTF's Y-up here.
//!
//! Every part becomes its own mesh and node, so alternatives a format keeps
//! side by side (a body with and without a backpack) can be shown or hidden
//! after import. Bones without a name are labelled `bone_NNN` by index.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// Rest pose relative to the parent: quaternion x y z w.
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub name: String,
    /// Material name, matched against [`Model::textures`].
    pub material: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Per vertex, four (bone index, weight) pairs; weights need not sum
    /// to one. Empty for parts that are not skinned.
    pub skin: Vec<[(u16, f32); 4]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    Translation(Vec<[f32; 3]>),
    Rotation(Vec<[f32; 4]>),
    Scale(Vec<[f32; 3]>),
}

/// Keys for one property of one bone, by index into [`Model::bones`].
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub bone: usize,
    pub times: Vec<f32>,
    pub values: Values,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub bones: Vec<Bone>,
    pub parts: Vec<Part>,
    /// (material name, PNG bytes)
    pub textures: Vec<(String, Vec<u8>)>,
    pub animations: Vec<Animation>,
    pub z_up: bool,
}

/// Z-up to glTF's Y-up.
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

pub fn to_glb(m: &Model) -> Vec<u8> {
    let pos = |v: [f32; 3]| if m.z_up { y_up(v) } else { v };
    let rot = |q: [f32; 4]| if m.z_up { y_up_rotation(q) } else { q };
    let scl = |s: [f32; 3]| if m.z_up { [s[0], s[2], s[1]] } else { s };
    let mut bin = Bin {
        data: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
    };
    let bone_count = m.bones.len();
    let skinned = bone_count > 0;
    let parent = |b: &Bone| b.parent.filter(|&p| p < bone_count);

    // Bones and their inverse bind matrices. Parents come before children
    // in the formats this serves; a later parent is treated as a root.
    let mut world: Vec<Mat> = Vec::with_capacity(bone_count);
    let mut nodes: Vec<String> = Vec::new();
    for (i, b) in m.bones.iter().enumerate() {
        let (q, t) = (rot(b.rotation), pos(b.translation));
        let local = matrix(q, t);
        let w = match parent(b).and_then(|p| world.get(p)) {
            Some(p) => mul(p, &local),
            None => local,
        };
        world.push(w);
        let name = if b.name.is_empty() {
            format!("bone_{i:03}")
        } else {
            b.name.clone()
        };
        let s = scl(b.scale);
        let scale = if s == [1.0; 3] {
            String::new()
        } else {
            format!(",\"scale\":[{}]", nums(&s))
        };
        nodes.push(format!(
            "{{\"name\":{},\"rotation\":[{}],\"translation\":[{}]{scale}",
            quote(&name),
            nums(&q),
            nums(&t)
        ));
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); bone_count];
    for (i, b) in m.bones.iter().enumerate() {
        if let Some(p) = parent(b).filter(|&p| p != i) {
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
        let bytes = floats(world.iter().flat_map(|w| {
            let inv = rigid_inverse(w);
            (0..16).map(move |k| inv[k % 4][k / 4])
        }));
        bin.accessor(&bytes, FLOAT, bone_count, "MAT4", None, None)
    });

    let mut images = Vec::new();
    let mut materials = Vec::new();
    for (name, png) in &m.textures {
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
    for part in &m.parts {
        let p: Vec<[f32; 3]> = part.positions.iter().map(|&v| pos(v)).collect();
        let n = p.len();
        let (mut lo, mut hi) = ([0.0f32; 3], [0.0f32; 3]);
        if n > 0 {
            lo = [f32::MAX; 3];
            hi = [f32::MIN; 3];
            for v in &p {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
        }
        let a_pos = bin.accessor(
            &floats(p.iter().flatten().copied()),
            FLOAT,
            n,
            "VEC3",
            Some(ARRAY),
            Some((&lo, &hi)),
        );
        let mut attrs = format!("\"POSITION\":{a_pos}");
        if part.uvs.len() == n {
            let a_uv = bin.accessor(
                &floats(part.uvs.iter().flatten().copied()),
                FLOAT,
                n,
                "VEC2",
                Some(ARRAY),
                None,
            );
            attrs.push_str(&format!(",\"TEXCOORD_0\":{a_uv}"));
        }
        if skinned && part.skin.len() == n && n > 0 {
            let mut joints = Vec::with_capacity(n * 8);
            let mut weights = Vec::with_capacity(n * 4);
            for w in &part.skin {
                let total: f32 = w.iter().map(|p| p.1.max(0.0)).sum();
                let total = if total > 0.0 { total } else { 1.0 };
                for &(bone, wt) in w {
                    let bone = if (bone as usize) < bone_count {
                        bone
                    } else {
                        0
                    };
                    joints.extend_from_slice(&bone.to_le_bytes());
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
        let material = part
            .material
            .as_ref()
            .and_then(|mat| m.textures.iter().position(|(name, _)| name == mat))
            .map(|i| format!(",\"material\":{i}"))
            .unwrap_or_default();
        meshes.push(format!(
            "{{\"name\":{},\"primitives\":[{{\"attributes\":{{{attrs}}},\"indices\":{a_idx}{material}}}]}}",
            quote(&part.name)
        ));
        let skin = if skinned { ",\"skin\":0" } else { "" };
        nodes.push(format!(
            "{{\"name\":{},\"mesh\":{}{skin}}}",
            quote(&part.name),
            meshes.len() - 1
        ));
        mesh_nodes.push(nodes.len() - 1);
    }

    let mut animations = Vec::new();
    for anim in &m.animations {
        let mut channels = Vec::new();
        let mut samplers = Vec::new();
        for c in &anim.channels {
            if c.bone >= bone_count || c.times.is_empty() {
                continue;
            }
            let (values, kind, path, count) = match &c.values {
                Values::Translation(v) => (
                    floats(v.iter().flat_map(|&x| pos(x))),
                    "VEC3",
                    "translation",
                    v.len(),
                ),
                Values::Rotation(v) => (
                    floats(v.iter().flat_map(|&x| rot(x))),
                    "VEC4",
                    "rotation",
                    v.len(),
                ),
                Values::Scale(v) => (
                    floats(v.iter().flat_map(|&x| scl(x))),
                    "VEC3",
                    "scale",
                    v.len(),
                ),
            };
            if count != c.times.len() {
                continue;
            }
            let lo = c.times.iter().copied().fold(f32::MAX, f32::min);
            let hi = c.times.iter().copied().fold(f32::MIN, f32::max);
            let input = bin.accessor(
                &floats(c.times.iter().copied()),
                FLOAT,
                count,
                "SCALAR",
                None,
                Some((&[lo], &[hi])),
            );
            let output = bin.accessor(&values, FLOAT, count, kind, None, None);
            samplers.push(format!(
                "{{\"input\":{input},\"output\":{output},\"interpolation\":\"LINEAR\"}}"
            ));
            channels.push(format!(
                "{{\"sampler\":{},\"target\":{{\"node\":{},\"path\":\"{path}\"}}}}",
                samplers.len() - 1,
                c.bone
            ));
        }
        if !channels.is_empty() {
            animations.push(format!(
                "{{\"name\":{},\"channels\":[{}],\"samplers\":[{}]}}",
                quote(&anim.name),
                channels.join(","),
                samplers.join(",")
            ));
        }
    }

    let mut scene: Vec<String> = m
        .bones
        .iter()
        .enumerate()
        .filter(|(i, b)| parent(b).is_none_or(|p| p == *i))
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
        let joints: Vec<String> = (0..bone_count).map(|i| i.to_string()).collect();
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
    if !animations.is_empty() {
        json.push_str(&format!(",\"animations\":[{}]", animations.join(",")));
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
