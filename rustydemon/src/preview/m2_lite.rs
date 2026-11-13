//! A small M2 reader that takes only what the preview draws.
//!
//! A full M2 parser has to understand every table in the file: animation
//! tracks, particle emitters, lights, attachments.  One value it does not
//! know (Blizzard adds them every patch) and the whole model is rejected,
//! though the vertices were fine.  The preview needs the vertices, the
//! texture lookup table, a few counts for the info text, and two chunks of
//! file ids, so this reads those and nothing else.
//!
//! Every offset and count comes from the file and is checked against the
//! data before use; nothing is allocated from an unchecked count.

/// What the preview needs from an M2.
#[derive(Debug, Clone, PartialEq)]
pub struct M2Lite {
    /// `true` for the chunked container (`MD21`, Legion and later).
    pub chunked: bool,
    pub version: u32,
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub texture_count: usize,
    /// What each texture slot is: 0 for a texture named in the file, 11 to
    /// 13 for the creature's "monster skin" 1 to 3, other values for
    /// textures the game supplies at runtime.  Empty if the table is
    /// unreadable.
    pub texture_types: Vec<u32>,
    pub material_count: usize,
    /// Blend mode of each material (the pair's second half; flags come first).
    pub blend_modes: Vec<u16>,
    pub bone_count: usize,
    pub animation_count: usize,
    /// Corners of the model's bounding box, as stored in the header.
    pub bounding_box: [[f32; 3]; 2],
    /// Tag and body size of every chunk, in file order (empty for a bare
    /// `MD20`).
    pub chunks: Vec<([u8; 4], usize)>,
    /// Texture combo index to texture index.
    pub texture_lookup: Vec<i16>,
    /// `SFID` chunk: skin files, then LOD skins.
    pub skin_fdids: Vec<u32>,
    /// `TXID` chunk: one texture file per texture slot, 0 for none.
    pub texture_fdids: Vec<u32>,
}

/// Oldest header layout this reads (Wrath of the Lich King).  Older files
/// have extra fields before the vertices and embed their skins.
const MIN_VERSION: u32 = 264;
const VERTEX_SIZE: usize = 48;

// Header fields, as offsets into the MD20 body.  Arrays are a `(count,
// offset)` pair of `u32`s; the offset is from the start of the body.
const F_NAME: usize = 0x08;
const F_ANIMATIONS: usize = 0x1C;
const F_BONES: usize = 0x2C;
const F_VERTICES: usize = 0x3C;
const F_TEXTURES: usize = 0x50;
const F_MATERIALS: usize = 0x70;
const F_TEXTURE_LOOKUP: usize = 0x80;
/// Six floats: minimum corner, then maximum corner.
const F_BOUNDING_BOX: usize = 0xA0;
/// The header must reach at least the end of the last field read.
const HEADER_MIN: usize = F_BOUNDING_BOX + 24;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let bytes = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The bytes of the array at header `field`, `item` bytes per element, or
/// an error naming `what` if the file is too short to hold it.
fn array<'a>(body: &'a [u8], field: usize, item: usize, what: &str) -> Result<&'a [u8], String> {
    let count = u32_at(body, field).ok_or("header cut short")? as usize;
    let offset = u32_at(body, field + 4).ok_or("header cut short")? as usize;
    count
        .checked_mul(item)
        .and_then(|len| offset.checked_add(len))
        .and_then(|end| body.get(offset..end))
        .ok_or_else(|| {
            format!(
                "{what} run past the end of the file ({count} at offset {offset}, {} bytes of model)",
                body.len()
            )
        })
}

fn u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

impl M2Lite {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let magic = data.get(..4).ok_or("file is shorter than its magic")?;
        let mut chunks = Vec::new();
        let (body, chunked, skin_fdids, texture_fdids) = match magic {
            b"MD20" => (data, false, Vec::new(), Vec::new()),
            b"MD21" => {
                let (mut body, mut skins, mut textures) = (None, Vec::new(), Vec::new());
                let mut at = 0usize;
                while at < data.len() {
                    let size = u32_at(data, at + 4).ok_or("chunk header cut short")? as usize;
                    let chunk = (at + 8)
                        .checked_add(size)
                        .and_then(|end| data.get(at + 8..end))
                        .ok_or("a chunk runs past the end of the file")?;
                    chunks.push(([data[at], data[at + 1], data[at + 2], data[at + 3]], size));
                    match &data[at..at + 4] {
                        b"MD21" => body = Some(chunk),
                        b"SFID" => skins = u32s(chunk),
                        b"TXID" => textures = u32s(chunk),
                        _ => {}
                    }
                    at += 8 + size;
                }
                (body.ok_or("no MD21 chunk")?, true, skins, textures)
            }
            _ => return Err("not an M2 file (no MD20 or MD21 magic)".into()),
        };

        if body.get(..4) != Some(b"MD20".as_slice()) || body.len() < HEADER_MIN {
            return Err("model header cut short".into());
        }
        let version = u32_at(body, 4).ok_or("model header cut short")?;
        if version < MIN_VERSION {
            return Err(format!(
                "M2 version {version} is older than this preview reads (needs {MIN_VERSION} or later)"
            ));
        }

        let vertices = array(body, F_VERTICES, VERTEX_SIZE, "vertices")?;
        let positions = vertices
            .as_chunks::<VERTEX_SIZE>()
            .0
            .iter()
            .map(|v| [f32_at(v, 0), f32_at(v, 4), f32_at(v, 8)])
            .collect();
        let normals = vertices
            .as_chunks::<VERTEX_SIZE>()
            .0
            .iter()
            .map(|v| [f32_at(v, 20), f32_at(v, 24), f32_at(v, 28)])
            .collect();
        let uvs = vertices
            .as_chunks::<VERTEX_SIZE>()
            .0
            .iter()
            .map(|v| [f32_at(v, 32), f32_at(v, 36)])
            .collect();
        let texture_lookup = array(body, F_TEXTURE_LOOKUP, 2, "texture lookups")?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect();

        // The name is only shown; a bad one is not worth losing the model.
        let name = array(body, F_NAME, 1, "name")
            .ok()
            .and_then(|n| std::str::from_utf8(n).ok())
            .map(|n| n.trim_end_matches('\0').to_owned())
            .unwrap_or_default();

        let count = |field| u32_at(body, field).unwrap_or(0) as usize;
        Ok(Self {
            chunked,
            version,
            name,
            positions,
            normals,
            uvs,
            texture_count: count(F_TEXTURES),
            // Sixteen bytes per slot, the type first.  Only shown and used
            // to pick creature skins, so a bad table costs nothing else.
            texture_types: array(body, F_TEXTURES, 16, "textures")
                .map(|t| {
                    t.as_chunks::<16>()
                        .0
                        .iter()
                        .map(|slot| u32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]))
                        .collect()
                })
                .unwrap_or_default(),
            material_count: count(F_MATERIALS),
            // Only changes how see-through parts look; not worth failing on.
            blend_modes: array(body, F_MATERIALS, 4, "materials")
                .map(|m| {
                    m.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes([pair[2], pair[3]]))
                        .collect()
                })
                .unwrap_or_default(),
            bone_count: count(F_BONES),
            animation_count: count(F_ANIMATIONS),
            bounding_box: [0, 12]
                .map(|corner| [0, 4, 8].map(|axis| f32_at(body, F_BOUNDING_BOX + corner + axis))),
            chunks,
            texture_lookup,
            skin_fdids,
            texture_fdids,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A header-only MD20 body: `0x130` zero bytes with magic and version.
    fn md20(version: u32) -> Vec<u8> {
        let mut b = vec![0u8; 0x130];
        b[..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&version.to_le_bytes());
        b
    }

    /// Append `bytes` and point the `(count, offset)` pair at `field` to it.
    fn put(b: &mut Vec<u8>, field: usize, count: u32, bytes: &[u8]) {
        let off = b.len() as u32;
        b[field..field + 4].copy_from_slice(&count.to_le_bytes());
        b[field + 4..field + 8].copy_from_slice(&off.to_le_bytes());
        b.extend_from_slice(bytes);
    }

    fn vertex(pos: [f32; 3], uv: [f32; 2]) -> Vec<u8> {
        let mut v = Vec::with_capacity(VERTEX_SIZE);
        for f in pos {
            v.extend_from_slice(&f.to_le_bytes());
        }
        v.extend_from_slice(&[0u8; 8]); // bone weights, bone indices
        for f in [0.0f32, 0.0, 1.0] {
            v.extend_from_slice(&f.to_le_bytes()); // normal
        }
        for f in uv {
            v.extend_from_slice(&f.to_le_bytes());
        }
        v.extend_from_slice(&[0u8; 8]); // second uv set
        v
    }

    fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = tag.to_vec();
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(body);
        c
    }

    fn le(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// Two vertices, a name, a lookup table, and some counts.
    fn body() -> Vec<u8> {
        let mut b = md20(274);
        put(&mut b, 0x08, 5, b"Crab\0");
        let mut verts = vertex([1.0, 2.0, 3.0], [0.25, 0.5]);
        verts.extend(vertex([4.0, 5.0, 6.0], [0.75, 1.0]));
        put(&mut b, 0x3C, 2, &verts);
        put(&mut b, 0x80, 2, &[1, 0, 0xFF, 0xFF]); // lookup: 1, -1
        b[0x1C..0x20].copy_from_slice(&9u32.to_le_bytes()); // animations
        for (i, f) in [-1.0f32, -2.0, -3.0, 1.0, 2.0, 3.0].iter().enumerate() {
            b[0xA0 + i * 4..0xA4 + i * 4].copy_from_slice(&f.to_le_bytes());
        }
        b[0x2C..0x30].copy_from_slice(&7u32.to_le_bytes()); // bones
        let mut textures = Vec::new();
        for kind in [0u32, 11, 12] {
            textures.extend_from_slice(&kind.to_le_bytes());
            textures.extend_from_slice(&[0u8; 12]); // flags, name
        }
        put(&mut b, 0x50, 3, &textures);
        // Materials: (flags, blend mode) pairs.
        let materials: Vec<u8> = [(0u16, 0u16), (16, 2), (21, 4), (0, 1)]
            .iter()
            .flat_map(|(f, m)| [f.to_le_bytes(), m.to_le_bytes()].concat())
            .collect();
        put(&mut b, 0x70, 4, &materials);
        b
    }

    pub(crate) fn chunked() -> Vec<u8> {
        let mut f = chunk(b"MD21", &body());
        f.extend(chunk(b"SFID", &le(&[111, 222])));
        f.extend(chunk(b"TXID", &le(&[0, 333])));
        f
    }

    #[test]
    fn reads_a_chunked_model() {
        let m = M2Lite::parse(&chunked()).unwrap();
        assert!(m.chunked);
        assert_eq!(m.version, 274);
        assert_eq!(m.name, "Crab");
        assert_eq!(m.positions, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(m.uvs, [[0.25, 0.5], [0.75, 1.0]]);
        assert_eq!(m.texture_lookup, [1, -1]);
        assert_eq!((m.bone_count, m.texture_count, m.material_count), (7, 3, 4));
        assert_eq!(m.skin_fdids, [111, 222]);
        assert_eq!(m.texture_fdids, [0, 333]);
    }

    #[test]
    fn reads_each_material_blend_mode() {
        let m = M2Lite::parse(&chunked()).unwrap();
        assert_eq!(m.blend_modes, [0, 2, 4, 1]);
    }

    #[test]
    fn reads_what_the_command_line_inspector_prints() {
        let m = M2Lite::parse(&chunked()).unwrap();
        assert_eq!(m.animation_count, 9);
        assert_eq!(m.texture_types, [0, 11, 12]);
        assert_eq!(m.normals, [[0.0, 0.0, 1.0]; 2]);
        assert_eq!(m.bounding_box, [[-1.0, -2.0, -3.0], [1.0, 2.0, 3.0]]);
        let tags: Vec<(&[u8; 4], usize)> = m.chunks.iter().map(|(t, n)| (t, *n)).collect();
        assert_eq!(tags[1..], [(b"SFID", 8), (b"TXID", 8)]);
        assert_eq!(tags[0].0, b"MD21");
        assert!(M2Lite::parse(&body()).unwrap().chunks.is_empty());
    }

    #[test]
    fn reads_a_bare_md20_with_no_chunks() {
        let m = M2Lite::parse(&body()).unwrap();
        assert!(!m.chunked);
        assert_eq!(m.positions.len(), 2);
        assert!(m.skin_fdids.is_empty());
    }

    /// The reason this reader exists: tables the preview does not draw can
    /// hold anything without costing the model its geometry.
    #[test]
    fn junk_in_tables_it_does_not_read_is_ignored() {
        let mut b = body();
        for field in [0x1C, 0x48, 0x58, 0x60, 0xF0, 0x100, 0x128] {
            b[field..field + 8].copy_from_slice(&[0xFF; 8]);
        }
        let mut f = chunk(b"MD21", &b);
        f.extend(chunk(b"SFID", &le(&[111])));
        f.extend(chunk(b"XXXX", &[0xAB; 9]));
        let m = M2Lite::parse(&f).unwrap();
        assert_eq!(m.positions.len(), 2);
        assert_eq!(m.skin_fdids, [111]);
    }

    /// A count that would need gigabytes is refused, not allocated.
    #[test]
    fn a_vertex_count_past_the_end_of_the_file_is_an_error() {
        let mut b = body();
        b[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = M2Lite::parse(&b).unwrap_err();
        assert!(err.contains("vertices"), "{err}");
    }

    #[test]
    fn a_lookup_table_past_the_end_of_the_file_is_an_error() {
        let mut b = body();
        b[0x84..0x88].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        assert!(M2Lite::parse(&b).is_err());
    }

    #[test]
    fn a_chunk_longer_than_the_file_is_an_error() {
        let mut f = chunked();
        f.extend_from_slice(b"TXID");
        f.extend_from_slice(&1000u32.to_le_bytes());
        assert!(M2Lite::parse(&f).is_err());
    }

    #[test]
    fn a_name_that_is_not_utf8_or_runs_off_the_end_does_not_fail_the_model() {
        let mut b = body();
        b[0x0C..0x10].copy_from_slice(&u32::MAX.to_le_bytes());
        let m = M2Lite::parse(&b).unwrap();
        assert_eq!(m.name, "");
        assert_eq!(m.positions.len(), 2);
    }

    #[test]
    fn a_texture_table_past_the_end_of_the_file_does_not_fail_the_model() {
        let mut b = body();
        b[0x54..0x58].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
        let m = M2Lite::parse(&b).unwrap();
        assert!(m.texture_types.is_empty());
        assert_eq!(m.texture_count, 3);
        assert_eq!(m.positions.len(), 2);
    }

    #[test]
    fn other_files_and_short_files_are_errors() {
        assert!(M2Lite::parse(b"").is_err());
        assert!(M2Lite::parse(b"MD2").is_err());
        assert!(M2Lite::parse(b"BLP2aaaaaaaaaaaaaaaa").is_err());
        assert!(M2Lite::parse(&md20(274)[..0x40]).is_err());
        assert!(M2Lite::parse(&chunk(b"SFID", &le(&[1]))).is_err());
    }

    #[test]
    fn header_layouts_older_than_wrath_are_refused_by_name() {
        let err = M2Lite::parse(&md20(260)).unwrap_err();
        assert!(err.contains("260"), "{err}");
    }
}
