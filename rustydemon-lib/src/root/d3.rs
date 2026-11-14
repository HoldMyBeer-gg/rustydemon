//! Diablo III root manifest.
//!
//! The root names one sub-root per top-level folder (`Base`, `Windows`,
//! `Mac` and one per locale such as `enUS`). Each sub-root lists its files
//! three ways: by SNO ID, by SNO ID plus a sub-file number, and by name.
//!
//! ```text
//! root
//!   u32  0x8007D0C4
//!   u32  count
//!   count * ([16] content key, NUL-terminated folder name)
//!
//! sub-root
//!   u32  0xEAF1FE87
//!   u32  count, count * ([16] content key, u32 SNO ID)
//!   u32  count, count * ([16] content key, u32 SNO ID, u32 sub-file number)
//!   u32  count, count * ([16] content key, NUL-terminated path)
//! ```
//!
//! SNO IDs get their names from `Base/CoreTOC.dat`, which lists each SNO's
//! group and name:
//!
//! ```text
//! u32 * 70  entry count per group
//! u32 * 70  offset of each group's block, from the end of the header
//! u32 * 70  a second count per group (meaning not known)
//! u32       unknown
//! per group: count * (u32 group, u32 SNO ID, u32 name offset), then the
//!            NUL-terminated names the offsets point into
//! ```
//!
//! A group's folder and extension (`Actor`, `.acr`) are not in CoreTOC.
//! `Base/Data_D3/PC/Misc/Packages.dat` lists every file as
//! `Folder\Name.ext` (`u32 0xAABB0002`, u32 count, NUL-terminated names), so
//! each group takes the folder and extension its names are filed under.
//! Sub-files appear there as `Folder\Name\NNNN.ext`.
//!
//! Sub-roots for locales that are not installed cannot be read; they are
//! skipped.
//!
//! Every SNO also opens as `sno/<id>` (the `Base` copy first), for formats
//! that refer to other files by SNO ID. Those aliases are not listed.

use std::collections::HashMap;

use crate::{
    error::CascError,
    jenkins96::jenkins96,
    root::RootHandler,
    types::{ContentFlags, LocaleFlags, Md5Hash, RootEntry},
};

pub const ROOT_MAGIC: u32 = 0x8007_D0C4;
pub const SUBDIR_MAGIC: u32 = 0xEAF1_FE87;
pub const PACKAGES_MAGIC: u32 = 0xAABB_0002;

const TOC_GROUPS: usize = 70;
const TOC_HEADER: usize = TOC_GROUPS * 3 * 4 + 4;

/// SNO ID to (group, name), from CoreTOC.dat.
pub type CoreToc = HashMap<u32, (u32, String)>;

pub fn is_d3_root(data: &[u8]) -> bool {
    data.len() >= 8 && data[..4] == ROOT_MAGIC.to_le_bytes()
}

/// The sub-roots named by the top-level root.
pub fn parse_root(data: &[u8]) -> Result<Vec<(Md5Hash, String)>, CascError> {
    let mut r = Reader::new(data);
    if r.u32()? != ROOT_MAGIC {
        return Err(bad("D3 root has the wrong magic"));
    }
    let count = r.u32()? as usize;
    let mut out = Vec::with_capacity(count.min(data.len() / 17));
    for _ in 0..count {
        let key = r.key()?;
        out.push((key, r.cstr()?));
    }
    Ok(out)
}

/// The three file lists of one sub-root.
#[derive(Debug, Default)]
pub struct SubDir {
    pub snos: Vec<(Md5Hash, u32)>,
    pub subs: Vec<(Md5Hash, u32, u32)>,
    pub named: Vec<(Md5Hash, String)>,
}

pub fn parse_subdir(data: &[u8]) -> Result<SubDir, CascError> {
    let mut r = Reader::new(data);
    if r.u32()? != SUBDIR_MAGIC {
        return Err(bad("D3 sub-root has the wrong magic"));
    }
    let mut s = SubDir::default();
    let n = r.count(20)?;
    for _ in 0..n {
        let key = r.key()?;
        s.snos.push((key, r.u32()?));
    }
    let n = r.count(24)?;
    for _ in 0..n {
        let key = r.key()?;
        let sno = r.u32()?;
        s.subs.push((key, sno, r.u32()?));
    }
    let n = r.count(17)?;
    for _ in 0..n {
        let key = r.key()?;
        s.named.push((key, r.cstr()?));
    }
    Ok(s)
}

pub fn parse_core_toc(data: &[u8]) -> Result<CoreToc, CascError> {
    if data.len() < TOC_HEADER {
        return Err(bad("CoreTOC.dat is shorter than its header"));
    }
    let word = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
    let mut toc = HashMap::new();
    for g in 0..TOC_GROUPS {
        let count = word(g) as usize;
        let block = TOC_HEADER + word(TOC_GROUPS + g) as usize;
        let names = block + count * 12;
        let entries = data
            .get(block..names)
            .ok_or_else(|| bad("CoreTOC.dat group runs past the end"))?;
        for e in entries.as_chunks::<12>().0 {
            let field = |o: usize| u32::from_le_bytes(e[o..o + 4].try_into().expect("4 bytes"));
            let (group, sno, offset) = (field(0), field(4), field(8) as usize);
            let mut r = Reader::new(data);
            r.pos = names + offset;
            toc.insert(sno, (group, r.cstr()?));
        }
    }
    Ok(toc)
}

/// Every path listed in Packages.dat, with `\` separators as stored.
pub fn parse_packages(data: &[u8]) -> Result<Vec<String>, CascError> {
    let mut r = Reader::new(data);
    if r.u32()? != PACKAGES_MAGIC {
        return Err(bad("Packages.dat has the wrong magic"));
    }
    let n = r.count(2)?;
    (0..n).map(|_| r.cstr()).collect()
}

/// Folder and extension for each SNO group.
///
/// A name can be filed under several folders (an actor and its appearance
/// share a name), so each group scores every folder by how many of its
/// names it holds against the larger of the two sizes, and the best fit
/// wins.
pub fn group_folders(toc: &CoreToc, packages: &[String]) -> HashMap<u32, (String, String)> {
    let mut by_name: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    let mut folder_size: HashMap<(&str, &str), usize> = HashMap::new();
    for p in packages {
        let Some((folder, file)) = p.split_once('\\') else {
            continue;
        };
        let Some((name, ext)) = file.rsplit_once('.') else {
            continue;
        };
        if file.contains('\\') {
            continue;
        }
        by_name.entry(name).or_default().push((folder, ext));
        *folder_size.entry((folder, ext)).or_default() += 1;
    }
    let mut votes: HashMap<u32, HashMap<(&str, &str), usize>> = HashMap::new();
    let mut group_size: HashMap<u32, usize> = HashMap::new();
    for (group, name) in toc.values() {
        *group_size.entry(*group).or_default() += 1;
        for fe in by_name.get(name.as_str()).into_iter().flatten() {
            *votes.entry(*group).or_default().entry(*fe).or_default() += 1;
        }
    }
    votes
        .into_iter()
        .filter_map(|(group, v)| {
            let size = group_size[&group];
            let ((folder, ext), _) = v.into_iter().max_by(|a, b| {
                let score =
                    |(fe, n): &((&str, &str), usize)| *n as f64 / size.max(folder_size[fe]) as f64;
                score(a)
                    .total_cmp(&score(b))
                    .then(a.1.cmp(&b.1))
                    .then(b.0.cmp(&a.0))
            })?;
            Some((group, (folder.to_owned(), ext.to_owned())))
        })
        .collect()
}

pub struct D3RootHandler {
    entries_by_hash: HashMap<u64, Vec<RootEntry>>,
    paths: HashMap<u64, String>,
    /// `sno/<id>` aliases, so a file that refers to another by SNO ID
    /// (a model naming its texture) can open it. Kept apart from the
    /// paths so they never show in the tree or the file count.
    by_sno: HashMap<u64, Vec<RootEntry>>,
}

impl D3RootHandler {
    /// Read the root, every sub-root `open` can reach, and the name tables
    /// in `Base`, and name every file.
    pub fn load(
        root: &[u8],
        open: impl Fn(&Md5Hash) -> Result<Vec<u8>, CascError>,
    ) -> Result<Self, CascError> {
        let mut dirs = Vec::new();
        for (key, name) in parse_root(root)? {
            match open(&key).and_then(|d| parse_subdir(&d)) {
                Ok(sub) => dirs.push((name, sub)),
                Err(e) => eprintln!("D3 root: skipping {name}: {e}"),
            }
        }

        let named_file = |want: &str| {
            dirs.iter()
                .filter(|(n, _)| n == "Base")
                .flat_map(|(_, s)| &s.named)
                .find(|(_, n)| n.eq_ignore_ascii_case(want))
                .and_then(|(k, _)| open(k).ok())
        };
        let toc = named_file("CoreTOC.dat")
            .and_then(|d| parse_core_toc(&d).ok())
            .unwrap_or_default();
        let packages = named_file("Data_D3\\PC\\Misc\\Packages.dat")
            .and_then(|d| parse_packages(&d).ok())
            .unwrap_or_default();
        let folders = group_folders(&toc, &packages);
        let sub_ext: HashMap<String, &str> = packages
            .iter()
            .filter(|p| p.matches('\\').count() == 2)
            .filter_map(|p| p.rsplit_once('.'))
            .map(|(stem, ext)| (stem.to_ascii_lowercase(), ext))
            .collect();

        let mut h = D3RootHandler {
            entries_by_hash: HashMap::new(),
            paths: HashMap::new(),
            by_sno: HashMap::new(),
        };
        // (folder, name) of a SNO, as a path below the sub-root.
        let sno_path = |sno: u32| -> (String, Option<&str>) {
            match toc.get(&sno) {
                Some((group, name)) => match folders.get(group) {
                    Some((folder, ext)) => (format!("{folder}/{name}"), Some(ext.as_str())),
                    None => (format!("Group{group}/{name}"), None),
                },
                None => (format!("SNO/{sno}"), None),
            }
        };
        for (dir, sub) in &dirs {
            for (key, sno) in &sub.snos {
                let (stem, ext) = sno_path(*sno);
                let path = match ext {
                    Some(ext) => format!("{dir}/{stem}.{ext}"),
                    None => format!("{dir}/{stem}"),
                };
                h.add(path, *key);
                h.by_sno
                    .entry(jenkins96(&format!("sno/{sno}")))
                    .or_default()
                    .push(entry(*key));
            }
            for (key, sno, n) in &sub.subs {
                let (stem, _) = sno_path(*sno);
                let file = format!("{stem}/{n:04}");
                let lookup = file.replace('/', "\\").to_ascii_lowercase();
                let path = match sub_ext.get(&lookup) {
                    Some(ext) => format!("{dir}/{file}.{ext}"),
                    None => format!("{dir}/{file}"),
                };
                h.add(path, *key);
            }
            for (key, name) in &sub.named {
                h.add(format!("{dir}/{}", name.replace('\\', "/")), *key);
            }
        }
        Ok(h)
    }

    fn add(&mut self, path: String, ckey: Md5Hash) {
        let hash = jenkins96(&path);
        self.entries_by_hash
            .entry(hash)
            .or_default()
            .push(entry(ckey));
        self.paths.entry(hash).or_insert(path);
    }
}

fn entry(ckey: Md5Hash) -> RootEntry {
    RootEntry {
        ckey,
        locale: LocaleFlags::ALL,
        content: ContentFlags::NONE,
    }
}

impl RootHandler for D3RootHandler {
    fn count(&self) -> usize {
        self.entries_by_hash.len()
    }

    fn get_all_entries(&self, hash: u64) -> &[RootEntry] {
        self.entries_by_hash
            .get(&hash)
            .or_else(|| self.by_sno.get(&hash))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_> {
        Box::new(
            self.entries_by_hash
                .iter()
                .flat_map(|(&h, v)| v.iter().map(move |e| (h, e))),
        )
    }

    fn hash_for_file_data_id(&self, _id: u32) -> Option<u64> {
        None
    }

    fn file_data_id_for_hash(&self, _hash: u64) -> Option<u32> {
        None
    }

    fn builtin_paths(&self) -> Vec<(u64, String)> {
        self.paths.iter().map(|(&h, p)| (h, p.clone())).collect()
    }

    fn has_builtin_paths(&self) -> bool {
        !self.paths.is_empty()
    }

    fn type_name(&self) -> &'static str {
        "D3"
    }
}

fn bad(msg: &str) -> CascError {
    CascError::InvalidData(msg.to_owned())
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CascError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| bad("D3 root data is truncated"))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, CascError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn key(&mut self) -> Result<Md5Hash, CascError> {
        Ok(Md5Hash(self.take(16)?.try_into().expect("16 bytes")))
    }

    /// A count whose entries take at least `min` bytes each, checked
    /// against what is left so a junk count cannot reserve gigabytes.
    fn count(&mut self, min: usize) -> Result<usize, CascError> {
        let n = self.u32()? as usize;
        if n > (self.data.len() - self.pos) / min {
            return Err(bad("D3 root count runs past the end"));
        }
        Ok(n)
    }

    fn cstr(&mut self) -> Result<String, CascError> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| bad("D3 root name is not terminated"))?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }
}
