//! A small ADT reader that takes only what the terrain preview draws.
//!
//! A terrain tile is a list of chunks; the 256 `MCNK` chunks each hold a
//! header and sub-chunks (heights, normals, texture layers, liquids, ...).
//! A full parser has to understand every sub-chunk, and some carry sizes
//! that are wrong in the file itself (the normals sub-chunk is the famous
//! one), so one bad sub-chunk loses the whole tile.  The preview needs each
//! `MCNK`'s position and its `MCVT` heights, so this reads those and
//! nothing else.

/// Heights per `MCNK`: 9 rows of 9 outer vertices interleaved with 8 rows
/// of 8 inner ones.
pub const HEIGHTS: usize = 9 * 9 + 8 * 8;

/// One `MCNK` terrain chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct AdtChunk {
    /// The header's position, three floats in file order.
    pub position: [f32; 3],
    /// `MCVT` heights relative to the position, if the chunk has them.
    /// Split `_tex0` / `_obj0` files have `MCNK` chunks without heights.
    pub heights: Option<Box<[f32; HEIGHTS]>>,
}

impl AdtChunk {
    /// Height at outer-grid vertex `(x, y)`, each `0..9`.
    pub fn outer(&self, x: usize, y: usize) -> f32 {
        self.heights.as_ref().map_or(0.0, |h| h[y * 17 + x])
    }

    /// Height at inner-grid vertex `(x, y)`, each `0..8`.
    pub fn inner(&self, x: usize, y: usize) -> f32 {
        self.heights.as_ref().map_or(0.0, |h| h[y * 17 + 9 + x])
    }
}

const HEADER_SIZE: usize = 128;
/// Header field: the chunk's position, three floats.
const H_POSITION: usize = 0x68;
/// Header field: offset of the heights sub-chunk from the start of the
/// `MCNK` chunk (its tag, not its body).
const H_OFS_HEIGHTS: usize = 0x14;

/// Tags are stored reversed: `MCNK` is `KNCM` on disk.
const MVER: &[u8; 4] = b"REVM";
const MCNK: &[u8; 4] = b"KNCM";
const MCVT: &[u8; 4] = b"TVCM";

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let bytes = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// The body of the chunk whose tag is at `at`, and where the next one starts.
fn chunk_at(b: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let size = u32_at(b, at.checked_add(4)?)? as usize;
    let start = at.checked_add(8)?;
    let end = start.checked_add(size)?;
    Some((b.get(start..end)?, end))
}

/// The heights in the `MCVT` sub-chunk whose tag is at `at`, if one is there
/// and whole.
fn heights_at(body: &[u8], at: usize) -> Option<Box<[f32; HEIGHTS]>> {
    if body.get(at..at.checked_add(4)?)? != MCVT {
        return None;
    }
    let floats = body.get(at + 8..at + 8 + HEIGHTS * 4)?;
    let mut out = Box::new([0.0f32; HEIGHTS]);
    for (h, c) in out.iter_mut().zip(floats.as_chunks::<4>().0) {
        *h = f32::from_le_bytes(*c);
    }
    Some(out)
}

/// Walk the sub-chunks from `at` until the heights turn up or a size stops
/// making sense.
fn find_heights(body: &[u8], mut at: usize) -> Option<Box<[f32; HEIGHTS]>> {
    while at + 8 <= body.len() {
        if &body[at..at + 4] == MCVT {
            return heights_at(body, at);
        }
        at = chunk_at(body, at)?.1;
    }
    None
}

fn mcnk(body: &[u8]) -> AdtChunk {
    // Split `_tex0` / `_obj0` files store MCNK chunks with no header: the
    // body opens straight on a sub-chunk tag (`MC..`, reversed).
    let headerless = body.len() < HEADER_SIZE || &body[2..4] == b"CM";
    if headerless {
        return AdtChunk {
            position: [0.0; 3],
            heights: find_heights(body, 0),
        };
    }
    let float =
        |at: usize| f32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
    let position = [0, 4, 8].map(|axis| float(H_POSITION + axis));
    // The header says where the heights are; trust the walk first, since
    // newer files leave that field zero.
    let heights = find_heights(body, HEADER_SIZE).or_else(|| {
        let ofs = u32_at(body, H_OFS_HEIGHTS)? as usize;
        heights_at(body, ofs.checked_sub(8)?)
    });
    AdtChunk { position, heights }
}

/// Every `MCNK` chunk of an ADT file, in file order.
pub fn parse(data: &[u8]) -> Result<Vec<AdtChunk>, String> {
    if data.get(..4) != Some(MVER.as_slice()) {
        return Err("not an ADT file (no MVER chunk at the start)".into());
    }
    let mut chunks = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let (body, next) = chunk_at(data, at)
            .ok_or_else(|| format!("a chunk at offset {at} runs past the end of the file"))?;
        if &data[at..at + 4] == MCNK {
            chunks.push(mcnk(body));
        }
        at = next;
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk as stored: reversed tag, size, body.
    fn chunk(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c: Vec<u8> = tag.iter().rev().copied().collect();
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(body);
        c
    }

    fn mver() -> Vec<u8> {
        chunk(b"MVER", &18u32.to_le_bytes())
    }

    /// An MCNK body: 128-byte header with `pos` at 0x68, then `subs`.
    fn mcnk(pos: [f32; 3], subs: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        for (i, f) in pos.iter().enumerate() {
            b[0x68 + i * 4..0x6C + i * 4].copy_from_slice(&f.to_le_bytes());
        }
        b.extend_from_slice(subs);
        chunk(b"MCNK", &b)
    }

    /// An MCVT sub-chunk whose height at index `i` is `base + i`.
    fn mcvt(base: f32) -> Vec<u8> {
        let body: Vec<u8> = (0..HEIGHTS)
            .flat_map(|i| (base + i as f32).to_le_bytes())
            .collect();
        chunk(b"MCVT", &body)
    }

    #[test]
    fn reads_positions_and_heights() {
        let mut f = mver();
        f.extend(chunk(b"MHDR", &[0; 64]));
        f.extend(mcnk([1.0, 2.0, 3.0], &mcvt(100.0)));
        f.extend(mcnk([4.0, 5.0, 6.0], &mcvt(500.0)));
        let chunks = parse(&f).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(chunks[1].position, [4.0, 5.0, 6.0]);
        // Row 0 is nine outer heights, then eight inner, then row 1.
        assert_eq!(chunks[0].outer(0, 0), 100.0);
        assert_eq!(chunks[0].outer(8, 0), 108.0);
        assert_eq!(chunks[0].inner(0, 0), 109.0);
        assert_eq!(chunks[0].outer(0, 1), 117.0);
        assert_eq!(chunks[1].outer(8, 8), 500.0 + 144.0);
    }

    /// The reason this reader exists.  The normals sub-chunk says it is
    /// 435 bytes and is followed by 13 that belong to nothing; whatever
    /// comes after the heights must not cost the tile its terrain.
    #[test]
    fn broken_sub_chunks_after_the_heights_are_ignored() {
        let mut subs = mcvt(0.0);
        subs.extend(chunk(b"MCNR", &[7; 435]));
        subs.extend_from_slice(&[9; 13]);
        subs.extend_from_slice(b"YLCM\xFF\xFF\xFF\x7F");
        let mut f = mver();
        f.extend(mcnk([0.0; 3], &subs));
        f.extend(mcnk([1.0; 3], &mcvt(10.0)));
        let chunks = parse(&f).unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].heights.is_some());
        assert_eq!(chunks[1].outer(0, 0), 10.0);
    }

    #[test]
    fn heights_are_found_behind_other_sub_chunks() {
        let mut subs = chunk(b"MCCV", &[1; 20]);
        subs.extend(mcvt(3.0));
        let mut f = mver();
        f.extend(mcnk([0.0; 3], &subs));
        assert_eq!(parse(&f).unwrap()[0].outer(0, 0), 3.0);
    }

    /// `_tex0` and `_obj0` files have MCNK chunks with no header and no
    /// heights; they are reported as chunks without terrain.
    #[test]
    fn chunks_without_heights_are_kept_without_them() {
        let mut f = mver();
        f.extend(chunk(b"MCNK", &chunk(b"MCLY", &[0; 16])));
        f.extend(mcnk([0.0; 3], &chunk(b"MCCV", &[1; 20])));
        let chunks = parse(&f).unwrap();
        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|c| c.heights.is_none()));
    }

    #[test]
    fn a_file_with_no_terrain_chunks_is_an_empty_list() {
        assert_eq!(parse(&mver()).unwrap(), []);
    }

    #[test]
    fn a_heights_sub_chunk_cut_short_gives_no_heights() {
        let mut subs = mcvt(0.0);
        subs.truncate(8 + 100);
        let mut f = mver();
        f.extend(mcnk([0.0; 3], &subs));
        // The chunk's own size is honest; only its heights are short.
        assert!(parse(&f).unwrap()[0].heights.is_none());
    }

    #[test]
    fn a_top_level_chunk_past_the_end_of_the_file_is_an_error() {
        let mut f = mver();
        f.extend_from_slice(b"KNCM");
        f.extend_from_slice(&5000u32.to_le_bytes());
        f.extend_from_slice(&[0; 40]);
        assert!(parse(&f).is_err());
    }

    #[test]
    fn other_files_are_errors() {
        assert!(parse(b"").is_err());
        assert!(parse(b"REV").is_err());
        assert!(parse(b"MD21aaaaaaaaaaaa").is_err());
    }
}
