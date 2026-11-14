//! Reader for assets of NetEase's Messiah engine, as shipped by Diablo
//! Immortal inside its MPK packages.
//!
//! ## Mesh objects
//!
//! A mesh is a `.MESSIAH` object that names its vertex layouts:
//!
//! ```text
//! [8]  ".MESSIAH"
//! u32  version (8)
//! u32  unknown
//! u16  flag: 0 plain, 2 compressed (not decoded yet)
//! u16  submesh count
//! u32  vertex count
//! u32  index count
//! 4 *  (u16 length, layout name)   e.g. "P3F", "Q4H_T2H", "W4B_I4B", "None"
//! 6 f32 bounding box min, max
//! 4 f32 bounding sphere centre, radius
//! submesh count * 4 u32: start index, index count, start vertex, vertex count
//! indices (u16, or u32 above 65535 vertices), absolute into the vertex list
//! one stream per layout that is not "None", vertex count entries each
//! submesh count * 52 bytes of per-submesh bounds
//! ```
//!
//! A layout name is elements joined by `_`: a letter for the meaning
//! (P position, N normal, T texture coordinate, Q tangent frame, C colour,
//! W bone weights, I bone indices), a component count, and a type letter
//! (F f32, H f16, B byte).

pub mod material;
pub mod repository;
pub mod texture;

#[cfg(test)]
mod material_tests;
#[cfg(test)]
mod repo_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod texture_tests;

/// Errors from reading a Messiah asset.
#[derive(Debug, thiserror::Error)]
pub enum MessiahError {
    #[error("not a Messiah mesh object")]
    NotAMesh,
    #[error("mesh data is compressed, which is not decoded yet")]
    Compressed,
    #[error("mesh object is malformed: {0}")]
    Malformed(String),
}

/// One draw range of a mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Submesh {
    pub start_index: u32,
    pub index_count: u32,
    pub start_vertex: u32,
    pub vertex_count: u32,
}

/// Geometry decoded from a mesh object.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// First texture coordinate set, or zeros when the mesh has none.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<Submesh>,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Layout name of each vertex stream, in file order.
    pub formats: Vec<String>,
}

const MAGIC: &[u8; 8] = b".MESSIAH";
const SUBMESH_BOUNDS: usize = 52;

/// Cheap check for a mesh object: the magic, then a vertex layout name
/// where the first one belongs.
pub fn looks_like_mesh(data: &[u8]) -> bool {
    if data.len() < 32 || &data[..8] != MAGIC {
        return false;
    }
    let len = u16::from_le_bytes([data[28], data[29]]) as usize;
    data.get(30..30 + len)
        .and_then(|n| std::str::from_utf8(n).ok())
        .is_some_and(|n| n.starts_with("P3F"))
}

#[derive(Clone, Copy)]
struct Element {
    meaning: u8,
    count: usize,
    kind: u8,
    offset: usize,
}

impl Element {
    fn size(self) -> usize {
        self.count
            * if self.kind == b'F' {
                4
            } else if self.kind == b'H' {
                2
            } else {
                1
            }
    }
}

/// Split a layout name into elements with their byte offsets in a vertex.
fn parse_layout(name: &str) -> Result<(Vec<Element>, usize), MessiahError> {
    if name == "None" {
        return Ok((Vec::new(), 0));
    }
    let mut elements = Vec::new();
    let mut offset = 0;
    for token in name.split('_') {
        let b = token.as_bytes();
        let ok = b.len() == 3
            && b[0].is_ascii_uppercase()
            && (b'1'..=b'4').contains(&b[1])
            && matches!(b[2], b'F' | b'H' | b'B');
        if !ok {
            return Err(MessiahError::Malformed(format!(
                "unknown vertex element {token:?} in {name:?}"
            )));
        }
        let e = Element {
            meaning: b[0],
            count: (b[1] - b'0') as usize,
            kind: b[2],
            offset,
        };
        offset += e.size();
        elements.push(e);
    }
    Ok((elements, offset))
}

impl Mesh {
    /// Decode a mesh object.
    pub fn parse(data: &[u8]) -> Result<Self, MessiahError> {
        if !looks_like_mesh(data) {
            return Err(MessiahError::NotAMesh);
        }
        let mut r = Reader { data, pos: 8 };
        let _version = r.u32()?;
        let _unknown = r.u32()?;
        let flag = r.u16()?;
        let submesh_count = r.u16()? as usize;
        let vertex_count = r.u32()? as usize;
        let index_count = r.u32()? as usize;
        if flag == 2 {
            return Err(MessiahError::Compressed);
        }
        if flag != 0 {
            return Err(MessiahError::Malformed(format!("unknown flag {flag}")));
        }

        let mut layouts = Vec::new();
        for _ in 0..4 {
            let len = r.u16()? as usize;
            let name = std::str::from_utf8(r.take(len)?)
                .map_err(|_| MessiahError::Malformed("layout name is not text".into()))?;
            if name != "None" {
                let (elements, stride) = parse_layout(name)?;
                layouts.push((name.to_owned(), elements, stride));
            }
        }

        let bbox_min = [r.f32()?, r.f32()?, r.f32()?];
        let bbox_max = [r.f32()?, r.f32()?, r.f32()?];
        r.take(16)?; // bounding sphere

        let mut submeshes = Vec::with_capacity(submesh_count);
        for _ in 0..submesh_count {
            submeshes.push(Submesh {
                start_index: r.u32()?,
                index_count: r.u32()?,
                start_vertex: r.u32()?,
                vertex_count: r.u32()?,
            });
        }

        let wide = vertex_count > 0xFFFF;
        let index_bytes = r.take(index_count * if wide { 4 } else { 2 })?;
        let indices: Vec<u32> = if wide {
            index_bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect()
        } else {
            index_bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u32::from(u16::from_le_bytes(*c)))
                .collect()
        };
        if let Some(&bad) = indices.iter().find(|&&i| i as usize >= vertex_count) {
            return Err(MessiahError::Malformed(format!(
                "index {bad} past {vertex_count} vertices"
            )));
        }

        let mut positions = None;
        let mut uvs = None;
        for (_, elements, stride) in &layouts {
            let stream = r.take(stride * vertex_count)?;
            let vertices = stream.chunks_exact(*stride);
            for e in elements {
                match (e.meaning, e.count, e.kind) {
                    (b'P', 3, b'F') if positions.is_none() => {
                        positions = Some(
                            vertices
                                .clone()
                                .map(|v| {
                                    [
                                        f32_at(v, e.offset),
                                        f32_at(v, e.offset + 4),
                                        f32_at(v, e.offset + 8),
                                    ]
                                })
                                .collect::<Vec<_>>(),
                        );
                    }
                    (b'T', 2, kind) if uvs.is_none() => {
                        let read = |v: &[u8], o: usize| {
                            if kind == b'H' {
                                f16_at(v, o)
                            } else {
                                f32_at(v, o)
                            }
                        };
                        let step = e.size() / 2;
                        uvs = Some(
                            vertices
                                .clone()
                                .map(|v| [read(v, e.offset), read(v, e.offset + step)])
                                .collect::<Vec<_>>(),
                        );
                    }
                    _ => {}
                }
            }
        }

        let expected_end = r.pos + SUBMESH_BOUNDS * submesh_count;
        if expected_end != data.len() {
            return Err(MessiahError::Malformed(format!(
                "mesh ends at byte {expected_end}, object is {} bytes",
                data.len()
            )));
        }
        let positions =
            positions.ok_or_else(|| MessiahError::Malformed("no P3F position element".into()))?;
        let uvs = uvs.unwrap_or_else(|| vec![[0.0, 0.0]; vertex_count]);

        Ok(Mesh {
            positions,
            uvs,
            indices,
            submeshes,
            bbox_min,
            bbox_max,
            formats: layouts.into_iter().map(|(name, _, _)| name).collect(),
        })
    }
}

fn f32_at(v: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]])
}

fn f16_at(v: &[u8], o: usize) -> f32 {
    let h = u16::from_le_bytes([v[o], v[o + 1]]);
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let mant = f32::from(h & 0x3ff);
    match exp {
        0 => sign * mant * 2f32.powi(-24),
        31 => {
            if mant == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + mant / 1024.0) * 2f32.powi(exp - 15),
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], MessiahError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| MessiahError::Malformed(format!("truncated at byte {}", self.pos)))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16, MessiahError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, MessiahError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32(&mut self) -> Result<f32, MessiahError> {
        Ok(f32::from_bits(self.u32()?))
    }
}
