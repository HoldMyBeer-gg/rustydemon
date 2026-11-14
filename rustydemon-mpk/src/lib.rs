//! Reader for the MPK archives of NetEase's Messiah engine, as shipped by
//! Diablo Immortal under `Package/MPK/`.
//!
//! ## Layout
//!
//! Each package is an index file `<Name>.mpkinfo` plus data files
//! `<Name>.mpk`, `<Name>1.mpk`, `<Name>2.mpk`, ...  Diablo Immortal ships
//! two packages: `Engine` and `Resources`.
//!
//! ```text
//! .mpkinfo
//!   u32 version (1)
//!   u32 entry count
//!   entry * count:
//!     u16 name length, name (UTF-8, '/' separated)
//!     u32 offset into the data file
//!     u32 stored size
//!     u32 field: data file number << 1 | flag
//!   [16] MD5 of every byte before it
//! ```
//!
//! A stored payload is either `ZZZ4` + u32 decompressed size + one LZ4
//! block, or the file's bytes as they are.  The meaning of the flag bit is
//! not known yet; it is kept on [`MpkEntry::flag`].

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use md5::{Digest, Md5};

#[cfg(test)]
mod tests;

/// Errors from reading an MPK package.
#[derive(Debug, thiserror::Error)]
pub enum MpkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("MPK index is truncated or malformed: {0}")]
    Malformed(String),
    #[error("MPK index checksum does not match")]
    Checksum,
    #[error("no .mpkinfo files in {0}")]
    NotAnArchive(PathBuf),
    #[error("LZ4 payload did not decompress: {0}")]
    Decompress(String),
}

/// One file in a package index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MpkEntry {
    /// Path inside the package, e.g. `Package/Char/npc/a_drink.anim`.
    pub path: String,
    pub offset: u32,
    /// Size as stored (compressed when the payload is ZZZ4).
    pub size: u32,
    /// Data file number: 0 is `<Name>.mpk`, n is `<Name>n.mpk`.
    pub archive: u32,
    /// Low bit of the archive field. Meaning not known yet.
    pub flag: bool,
}

const MAGIC_ZZZ4: &[u8; 4] = b"ZZZ4";
const MAGIC_CCCC: &[u8; 4] = b"CCCC";

/// Parse an `.mpkinfo` index, checking its trailing MD5.
pub fn parse_index(data: &[u8]) -> Result<Vec<MpkEntry>, MpkError> {
    if data.len() < 8 + 16 {
        return Err(MpkError::Malformed(
            "shorter than header and checksum".into(),
        ));
    }
    let (body, digest) = data.split_at(data.len() - 16);
    if Md5::digest(body).as_slice() != digest {
        return Err(MpkError::Checksum);
    }

    let mut r = Cursor { data: body, pos: 0 };
    let _version = r.u32()?;
    let count = r.u32()? as usize;
    // Every entry takes at least 14 bytes, so a count beyond that is junk.
    if count > body.len() / 14 {
        return Err(MpkError::Malformed(format!(
            "entry count {count} too large"
        )));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.u16()? as usize;
        let name = r.take(len)?;
        let path = String::from_utf8(name.to_vec())
            .map_err(|_| MpkError::Malformed("entry name is not UTF-8".into()))?;
        let offset = r.u32()?;
        let size = r.u32()?;
        let field = r.u32()?;
        entries.push(MpkEntry {
            path,
            offset,
            size,
            archive: field >> 1,
            flag: field & 1 == 1,
        });
    }
    Ok(entries)
}

/// Turn a stored payload into the file's bytes.
pub fn decode_payload(stored: &[u8]) -> Result<Vec<u8>, MpkError> {
    // The resource registry wraps its ZZZ4 payload in a `CCCC` tag, and a
    // few stray bytes follow the last LZ4 sequence there, so decode it with
    // a reader that stops at the stated size.
    if let Some(inner) = stored.strip_prefix(MAGIC_CCCC) {
        if inner.len() >= 8 && inner.starts_with(MAGIC_ZZZ4) {
            let size = u32::from_le_bytes(inner[4..8].try_into().expect("4 bytes")) as usize;
            return lz4_block_until(&inner[8..], size);
        }
    }
    if stored.len() < 8 || &stored[..4] != MAGIC_ZZZ4 {
        return Ok(stored.to_vec());
    }
    let size = u32::from_le_bytes(stored[4..8].try_into().expect("4 bytes")) as usize;
    let out = lz4_flex::block::decompress(&stored[8..], size)
        .map_err(|e| MpkError::Decompress(e.to_string()))?;
    if out.len() != size {
        return Err(MpkError::Decompress(format!(
            "expected {size} bytes, got {}",
            out.len()
        )));
    }
    Ok(out)
}

/// Decode an LZ4 block until `size` bytes are out, ignoring anything after.
fn lz4_block_until(src: &[u8], size: usize) -> Result<Vec<u8>, MpkError> {
    let short = || MpkError::Decompress("LZ4 block ends early".into());
    let mut out = Vec::with_capacity(size);
    let mut i = 0;
    let read_len = |i: &mut usize, mut n: usize| -> Result<usize, MpkError> {
        loop {
            let b = *src.get(*i).ok_or_else(short)?;
            *i += 1;
            n += b as usize;
            if b != 255 {
                return Ok(n);
            }
        }
    };
    while out.len() < size {
        let token = *src.get(i).ok_or_else(short)?;
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            lit = read_len(&mut i, lit)?;
        }
        let literals = src.get(i..i + lit).ok_or_else(short)?;
        out.extend_from_slice(literals);
        i += lit;
        if out.len() >= size {
            break;
        }
        let lo = *src.get(i).ok_or_else(short)? as usize;
        let hi = *src.get(i + 1).ok_or_else(short)? as usize;
        i += 2;
        let offset = lo | hi << 8;
        let mut len = (token & 15) as usize;
        if len == 15 {
            len = read_len(&mut i, len)?;
        }
        len += 4;
        if offset == 0 || offset > out.len() {
            return Err(MpkError::Decompress(format!("bad LZ4 offset {offset}")));
        }
        let start = out.len() - offset;
        for k in 0..len {
            out.push(out[start + k]);
        }
    }
    out.truncate(size);
    Ok(out)
}

/// Every package found in one `MPK` directory, merged into one entry list.
#[derive(Debug)]
pub struct MpkArchive {
    entries: Vec<MpkEntry>,
    /// For each entry, the package stem it came from (index into `stems`).
    package_of: Vec<u16>,
    stems: Vec<String>,
    dir: PathBuf,
}

impl MpkArchive {
    /// Load every `*.mpkinfo` in `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, MpkError> {
        let dir = dir.as_ref().to_path_buf();
        let mut infos: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("mpkinfo"))
            })
            .collect();
        if infos.is_empty() {
            return Err(MpkError::NotAnArchive(dir));
        }
        infos.sort();

        let mut entries = Vec::new();
        let mut package_of = Vec::new();
        let mut stems = Vec::new();
        for info in infos {
            let stem = info
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| MpkError::Malformed(format!("bad file name {}", info.display())))?
                .to_owned();
            let parsed = parse_index(&fs::read(&info)?)?;
            let id = u16::try_from(stems.len())
                .map_err(|_| MpkError::Malformed("too many packages".into()))?;
            package_of.extend(std::iter::repeat_n(id, parsed.len()));
            entries.extend(parsed);
            stems.push(stem);
        }
        Ok(Self {
            entries,
            package_of,
            stems,
            dir,
        })
    }

    pub fn entries(&self) -> &[MpkEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Path of the data file holding entry `index`.
    pub fn data_file(&self, index: usize) -> Option<PathBuf> {
        let e = self.entries.get(index)?;
        let stem = &self.stems[self.package_of[index] as usize];
        let name = if e.archive == 0 {
            format!("{stem}.mpk")
        } else {
            format!("{stem}{}.mpk", e.archive)
        };
        Some(self.dir.join(name))
    }

    /// Read and decode entry `index`.
    pub fn read(&self, index: usize) -> Result<Vec<u8>, MpkError> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| MpkError::Malformed(format!("no entry {index}")))?;
        let path = self.data_file(index).expect("entry exists");
        let mut file = fs::File::open(&path)?;
        let end = u64::from(entry.offset) + u64::from(entry.size);
        if end > file.metadata()?.len() {
            return Err(MpkError::Malformed(format!(
                "{} runs past the end of {}",
                entry.path,
                path.display()
            )));
        }
        file.seek(SeekFrom::Start(u64::from(entry.offset)))?;
        let mut stored = vec![0; entry.size as usize];
        file.read_exact(&mut stored)?;
        decode_payload(&stored)
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], MpkError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| MpkError::Malformed(format!("truncated at byte {}", self.pos)))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16, MpkError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, MpkError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
}
