//! The resource registry, `Package/resource.repository`.
//!
//! It names every resource in the packages, whose files are otherwise
//! stored under their GUID (`Package/ab/abcdef01-...`), and records what
//! each resource depends on: a model on its meshes and materials, a
//! material on its textures.
//!
//! ```text
//! u32  version (1)
//! u32  entry count
//! u16  0
//! u16  length, type names joined by ';'   (Texture2D;ParticleSystem;...)
//! u16  0xFFFF, u32 length, folder names joined by ';'
//! entry * count:
//!   [5]  unknown
//!   [16] GUID
//!   u16  length, name
//!   u16  folder index, u16 type index, u16 dependency count
//!   [16] * dependency count: GUIDs
//! ```

use std::{collections::HashMap, sync::OnceLock};

use crate::MessiahError;

/// One registered resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub guid: [u8; 16],
    pub name: String,
    folder: u16,
    kind: u16,
    deps: std::ops::Range<u32>,
}

/// The parsed registry.
#[derive(Debug, Default)]
pub struct Repository {
    types: Vec<String>,
    folders: Vec<String>,
    resources: Vec<Resource>,
    deps: Vec<[u8; 16]>,
    by_guid: HashMap<[u8; 16], u32>,
    /// For each GUID, the resources that depend on it. Built on first use.
    users: OnceLock<HashMap<[u8; 16], Vec<u32>>>,
    /// Readable path to resource. Built on first use.
    by_readable: OnceLock<HashMap<String, u32>>,
}

/// Format 16 GUID bytes the way package file names spell them.
pub fn uuid_string(b: &[u8; 16]) -> String {
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Parse a GUID spelled as in package file names.
pub fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = s.bytes().filter(|&c| c != b'-').collect();
    if hex.len() != 32 || s.len() != 36 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.as_chunks::<2>().0.iter().enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair.as_slice()).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Where a resource's file lives in the packages (textures add `.6`).
pub fn package_path(guid: &str) -> String {
    format!("Package/{}/{guid}", &guid[..2.min(guid.len())])
}

/// The folder list's first name stands for "no folder".
const NO_FOLDER: &str = "@^_^@";

impl Repository {
    pub fn parse(data: &[u8]) -> Result<Self, MessiahError> {
        let mut r = Reader { data, pos: 0 };
        let _version = r.u32()?;
        let count = r.u32()? as usize;
        let _zero = r.u16()?;
        let types: Vec<String> = r.string16()?.split(';').map(str::to_owned).collect();
        let folders_len = match r.u16()? {
            0xFFFF => r.u32()? as usize,
            n => n as usize,
        };
        let folders: Vec<String> = String::from_utf8_lossy(r.take(folders_len)?)
            .split(';')
            .map(|f| {
                if f == NO_FOLDER {
                    String::new()
                } else {
                    f.to_owned()
                }
            })
            .collect();

        // Each entry is at least 5 + 16 + 2 + 6 bytes.
        if count > data.len() / 29 {
            return Err(MessiahError::Malformed(format!(
                "{count} entries cannot fit"
            )));
        }
        let mut resources = Vec::with_capacity(count);
        let mut deps = Vec::with_capacity(count);
        let mut by_guid = HashMap::with_capacity(count);
        for i in 0..count {
            r.take(5)?;
            let guid: [u8; 16] = r.take(16)?.try_into().expect("16 bytes");
            let name = r.string16()?;
            let folder = r.u16()?;
            let kind = r.u16()?;
            let n = r.u16()? as usize;
            let first = deps.len() as u32;
            for chunk in r.take(16 * n)?.as_chunks::<16>().0 {
                deps.push(*chunk);
            }
            by_guid.insert(guid, i as u32);
            resources.push(Resource {
                guid,
                name,
                folder,
                kind,
                deps: first..deps.len() as u32,
            });
        }
        Ok(Repository {
            types,
            folders,
            resources,
            deps,
            by_guid,
            ..Default::default()
        })
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    pub fn resources(&self) -> &[Resource] {
        &self.resources
    }

    pub fn get(&self, guid: &str) -> Option<&Resource> {
        self.get_raw(&parse_uuid(guid)?)
    }

    pub fn get_raw(&self, guid: &[u8; 16]) -> Option<&Resource> {
        self.by_guid.get(guid).map(|&i| &self.resources[i as usize])
    }

    /// Folder of a resource, empty when it has none.
    pub fn folder(&self, r: &Resource) -> &str {
        self.folders
            .get(r.folder as usize)
            .map_or("", String::as_str)
    }

    /// Type of a resource, e.g. `Mesh`, `Model`, `Material`, `Texture2D`.
    pub fn kind(&self, r: &Resource) -> &str {
        self.types
            .get(r.kind as usize)
            .map_or("Unknown", String::as_str)
    }

    pub fn deps(&self, r: &Resource) -> &[[u8; 16]] {
        &self.deps[r.deps.start as usize..r.deps.end as usize]
    }

    /// A path that names the resource: its folder, name and type.
    pub fn readable_path(&self, r: &Resource) -> String {
        let ext = self.kind(r).to_ascii_lowercase();
        match self.folder(r) {
            "" => format!("{}.{ext}", r.name),
            folder => format!("{folder}/{}.{ext}", r.name),
        }
    }

    /// The GUID behind a readable path or a `Package/ab/<guid>` path.
    pub fn guid_for_path(&self, path: &str) -> Option<String> {
        let readable = self.by_readable.get_or_init(|| {
            self.resources
                .iter()
                .enumerate()
                .map(|(i, r)| (self.readable_path(r), i as u32))
                .collect()
        });
        if let Some(&i) = readable.get(path) {
            return Some(uuid_string(&self.resources[i as usize].guid));
        }
        let file = path.rsplit('/').next()?;
        let guid = parse_uuid(file.split('.').next()?)?;
        self.by_guid.contains_key(&guid).then(|| uuid_string(&guid))
    }

    /// Materials of the models that use this mesh, in dependency order.
    pub fn materials_for_mesh(&self, mesh: &str) -> Vec<String> {
        let Some(mesh) = parse_uuid(mesh) else {
            return Vec::new();
        };
        let users = self.users.get_or_init(|| {
            let mut users: HashMap<[u8; 16], Vec<u32>> = HashMap::new();
            for (i, r) in self.resources.iter().enumerate() {
                for d in self.deps(r) {
                    users.entry(*d).or_default().push(i as u32);
                }
            }
            users
        });
        let mut out = Vec::new();
        for &m in users.get(&mesh).into_iter().flatten() {
            for d in self.deps(&self.resources[m as usize]) {
                let s = uuid_string(d);
                if self.get_raw(d).is_some_and(|r| self.kind(r) == "Material") && !out.contains(&s)
                {
                    out.push(s);
                }
            }
            if !out.is_empty() {
                break;
            }
        }
        out
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
            .ok_or_else(|| {
                MessiahError::Malformed(format!("registry truncated at byte {}", self.pos))
            })?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, MessiahError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self) -> Result<u32, MessiahError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn string16(&mut self) -> Result<String, MessiahError> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}
