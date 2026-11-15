//! M2 bone animation: the sequence table, and each bone's translation,
//! rotation and scale keys per sequence.
//!
//! A sequence's keys live in the model when its flags carry 0x20, and
//! otherwise in a sibling `.anim` file named by the `AFID` chunk (the
//! file may wrap its data in an `AFM2` chunk).  A sequence flagged 0x40
//! is an alias of another, but one stored in the model still carries its
//! own keys, so where the keys are decides what is read, not the flag;
//! an alias with no file of its own is left out.  Every track is a
//! pair of outer arrays with one `(count, offset)` entry per sequence;
//! the entry's offset counts from the start of whichever blob holds the
//! sequence.  Tracks on a global sequence loop on their own clock and are
//! not read here.  Rotations are four signed shorts; 32767, 32767, 32767,
//! -1 is the identity.

use super::m2_lite::{body_of, F_ANIMATIONS, F_BONES};

const SEQUENCE_SIZE: usize = 64;
const BONE_SIZE: usize = 88;
/// Translation, rotation and scale track headers inside a bone record,
/// with the size of one value each.
const TRACKS: [(usize, usize); 3] = [(0x10, 12), (0x24, 8), (0x38, 12)];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sequence {
    /// Animation id, as in the game's animation table.
    pub id: u16,
    /// Which variation of that animation this is.
    pub variation: u16,
    pub duration_ms: u32,
    pub flags: u32,
    /// For an alias, the sequence it stands for.
    pub alias_next: u16,
}

impl Sequence {
    pub fn stored_in_model(&self) -> bool {
        self.flags & 0x20 != 0
    }
    pub fn is_alias(&self) -> bool {
        self.flags & 0x40 != 0
    }
    /// `0060-01`: the id and variation, as the game names the .anim file.
    pub fn name(&self) -> String {
        format!("{:04}-{:02}", self.id, self.variation)
    }
}

/// One bone's keys in one sequence, times in milliseconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoneKeys {
    pub translation: Vec<(u32, [f32; 3])>,
    /// Quaternion x y z w.
    pub rotation: Vec<(u32, [f32; 4])>,
    pub scale: Vec<(u32, [f32; 3])>,
}

/// A sequence with its keys for every bone, in bone order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clip {
    pub sequence: Sequence,
    pub bones: Vec<BoneKeys>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|c| u16::from_le_bytes([c[0], c[1]]))
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at).unwrap_or(0))
}

/// A `(count, offset)` array header at `at`, as the slice it names, if it
/// fits inside `blob`.
fn array(blob: &[u8], at: usize, item: usize) -> Option<&[u8]> {
    let count = u32_at(blob, at)? as usize;
    let offset = u32_at(blob, at + 4)? as usize;
    let len = count.checked_mul(item)?;
    blob.get(offset..offset.checked_add(len)?)
}

/// The sequence table of a model.
pub fn sequences(data: &[u8]) -> Result<Vec<Sequence>, String> {
    let body = body_of(data)?;
    let table = array(body, F_ANIMATIONS, SEQUENCE_SIZE).unwrap_or(&[]);
    Ok(table
        .as_chunks::<SEQUENCE_SIZE>()
        .0
        .iter()
        .map(|s| Sequence {
            id: u16::from_le_bytes([s[0], s[1]]),
            variation: u16::from_le_bytes([s[2], s[3]]),
            duration_ms: u32::from_le_bytes([s[4], s[5], s[6], s[7]]),
            flags: u32::from_le_bytes([s[0x0C], s[0x0D], s[0x0E], s[0x0F]]),
            alias_next: u16::from_le_bytes([s[0x3E], s[0x3F]]),
        })
        .collect())
}

/// `(animation id, variation, file data id)` for every .anim file the
/// `AFID` chunk names.  Empty for a model without the chunk.
pub fn anim_files(data: &[u8]) -> Vec<(u16, u16, u32)> {
    if data.get(..4) != Some(b"MD21".as_slice()) {
        return Vec::new();
    }
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let size = u32_at(data, at + 4).unwrap_or(0) as usize;
        let Some(chunk) = data.get(at + 8..(at + 8).saturating_add(size)) else {
            break;
        };
        if &data[at..at + 4] == b"AFID" {
            return chunk
                .as_chunks::<8>()
                .0
                .iter()
                .map(|e| {
                    (
                        u16::from_le_bytes([e[0], e[1]]),
                        u16::from_le_bytes([e[2], e[3]]),
                        u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
                    )
                })
                .collect();
        }
        at += 8 + size;
    }
    Vec::new()
}

/// The key data of an .anim file: the file itself, or the body of its
/// `AFM2` chunk.
pub fn anim_blob(file: &[u8]) -> &[u8] {
    if file.get(..4) == Some(b"AFM2".as_slice()) {
        if let Some(blob) = array_body(file) {
            return blob;
        }
    }
    file
}

fn array_body(chunked: &[u8]) -> Option<&[u8]> {
    let size = u32_at(chunked, 4)? as usize;
    chunked.get(8..8usize.checked_add(size)?)
}

/// A compressed quaternion: each short maps -32768..=32767 onto -1..=1
/// with the zero at 32767 (-1 for 1).
fn quat(b: &[u8]) -> [f32; 4] {
    let f = |i: usize| {
        let v = i16::from_le_bytes([b[i], b[i + 1]]) as i32;
        (if v < 0 { v + 32768 } else { v - 32767 }) as f32 / 32767.0
    };
    [f(0), f(2), f(4), f(6)]
}

/// The keys of every sequence whose data is at hand, with `anim`
/// supplying the .anim file for a given id and variation.  A sequence
/// whose file is missing is left out; a key slice that runs past its blob
/// is dropped.
pub fn clips(data: &[u8], anim: &dyn Fn(u16, u16) -> Option<Vec<u8>>) -> Result<Vec<Clip>, String> {
    let body = body_of(data)?;
    let seqs = sequences(data)?;
    let bones = array(body, F_BONES, BONE_SIZE).unwrap_or(&[]);
    let bone_count = bones.len() / BONE_SIZE;
    let mut out = Vec::new();
    for (si, seq) in seqs.iter().enumerate() {
        let file;
        let blob: &[u8] = if seq.stored_in_model() {
            body
        } else {
            match anim(seq.id, seq.variation) {
                Some(f) => {
                    file = f;
                    anim_blob(&file)
                }
                None => continue,
            }
        };
        let mut keys = vec![BoneKeys::default(); bone_count];
        for (bi, bone) in bones.as_chunks::<BONE_SIZE>().0.iter().enumerate() {
            for (k, (at, item)) in TRACKS.iter().enumerate() {
                let global = u16_at(bone, at + 2).unwrap_or(0) as i16;
                if global >= 0 {
                    continue;
                }
                // The outer arrays live in the model; entry `si` of each
                // names this sequence's keys inside `blob`.
                let outer = |header: usize| {
                    array_in(body, &bone[header..header + 8], 8)
                        .and_then(|entries| entries.get(si * 8..si * 8 + 8))
                };
                let (Some(times), Some(values)) = (outer(at + 4), outer(at + 12)) else {
                    continue;
                };
                let (Some(t), Some(v)) = (array_in(blob, times, 4), array_in(blob, values, *item))
                else {
                    continue;
                };
                let n = (t.len() / 4).min(v.len() / item);
                let stamp = |i: usize| {
                    u32::from_le_bytes([t[i * 4], t[i * 4 + 1], t[i * 4 + 2], t[i * 4 + 3]])
                };
                let bk = &mut keys[bi];
                match k {
                    0 => bk.translation = (0..n).map(|i| (stamp(i), vec3(v, i * 12))).collect(),
                    1 => {
                        bk.rotation = (0..n)
                            .map(|i| (stamp(i), quat(&v[i * 8..i * 8 + 8])))
                            .collect()
                    }
                    _ => bk.scale = (0..n).map(|i| (stamp(i), vec3(v, i * 12))).collect(),
                }
            }
        }
        out.push(Clip {
            sequence: seq.clone(),
            bones: keys,
        });
    }
    Ok(out)
}

/// The slice an 8-byte `(count, offset)` entry names inside `blob`.
fn array_in<'a>(blob: &'a [u8], entry: &[u8], item: usize) -> Option<&'a [u8]> {
    let count = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
    let offset = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;
    blob.get(offset..offset.checked_add(count.checked_mul(item)?)?)
}

fn vec3(b: &[u8], at: usize) -> [f32; 3] {
    [f32_at(b, at), f32_at(b, at + 4), f32_at(b, at + 8)]
}
