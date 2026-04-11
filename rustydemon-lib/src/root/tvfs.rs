use std::collections::{HashMap, HashSet};

use crate::{
    blte,
    encoding::EncodingHandler,
    error::CascError,
    jenkins96::jenkins96,
    local_index::LocalIndexHandler,
    types::{ContentFlags, EKey9, LocaleFlags, Md5Hash, RootEntry},
};

use super::RootHandler;

// ── Constants ─────────────────────────────────────────────────────────────────

const TVFS_MAGIC: u32 = 0x5346_5654; // 'TVFS' in LE

const TVFS_PTE_PATH_SEPARATOR_PRE: u32 = 0x0001;
const TVFS_PTE_PATH_SEPARATOR_POST: u32 = 0x0002;
const TVFS_PTE_NODE_VALUE: u32 = 0x0004;

const TVFS_FOLDER_NODE: u32 = 0x8000_0000;
const TVFS_FOLDER_SIZE_MASK: u32 = 0x7FFF_FFFF;

// ── TVFS directory header ─────────────────────────────────────────────────────

struct TvfsHeader {
    ekey_size: u8,
    path_table: Vec<u8>,
    vfs_table: Vec<u8>,
    cft_table: Vec<u8>,
    cft_offs_size: usize,
}

struct PathTableEntry {
    name: Vec<u8>,
    node_flags: u32,
    node_value: u32,
}

// ── File opener ───────────────────────────────────────────────────────────────

/// Minimal interface for opening files by encoding key during TVFS construction.
/// We can't use CascHandler here because it doesn't exist yet at construction time.
pub(crate) struct FileOpener<'a> {
    pub encoding: &'a EncodingHandler,
    pub local_index: &'a LocalIndexHandler,
    pub data_path: std::path::PathBuf,
}

impl FileOpener<'_> {
    fn open_by_ekey(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let idx = self
            .local_index
            .get_entry(ekey)
            .ok_or_else(|| CascError::IndexNotFound(ekey.to_hex()))?;

        let raw =
            crate::handler::read_data_block(&self.data_path, idx.index, idx.offset, idx.size)?;

        blte::decode(&raw, ekey, false)
    }
}

// ── TvfsRootHandler ───────────────────────────────────────────────────────────

/// Root handler for games using the TVFS (Table Virtual File System) format.
///
/// Used by Diablo IV, Overwatch 2, and other newer Blizzard titles that set
/// `root = 00…00` and provide `vfs-root` / `vfs-N` entries in the build config.
pub struct TvfsRootHandler {
    /// Hash → root entries (path hash → content keys).
    entries: HashMap<u64, Vec<RootEntry>>,
    /// Hash → original file path (for tree building / display).
    pub(crate) file_paths: HashMap<u64, String>,
}

impl TvfsRootHandler {
    /// Parse a TVFS root from the given config, using the provided opener to
    /// load VFS sub-directories and resolve encoding keys.
    pub(crate) fn load(
        vfs_root_ekey: &Md5Hash,
        vfs_root_list: &[(Md5Hash, Md5Hash)],
        opener: &FileOpener<'_>,
    ) -> Result<Self, CascError> {
        // Build the set of known VFS sub-directory EKeys (9-byte prefix).
        let vfs_ekey_set: HashSet<EKey9> = vfs_root_list
            .iter()
            .map(|(_c, e)| EKey9::from_full(e))
            .collect();

        // Map from 9-byte EKey prefix → full EKey for sub-directory lookup.
        let vfs_ekey_map: HashMap<EKey9, Md5Hash> = vfs_root_list
            .iter()
            .map(|(_c, e)| (EKey9::from_full(e), *e))
            .collect();

        let mut handler = TvfsRootHandler {
            entries: HashMap::new(),
            file_paths: HashMap::new(),
        };

        // Decode and parse the primary VFS root.
        let root_data = opener.open_by_ekey(vfs_root_ekey)?;
        let header = parse_header(&root_data)?;

        let mut path_buf: Vec<u8> = Vec::with_capacity(512);
        handler.parse_directory_data(
            &header,
            &vfs_ekey_set,
            &vfs_ekey_map,
            opener,
            &mut path_buf,
        )?;

        // Try to load CoreTOC.dat and resolve SNO IDs to real names.
        handler.resolve_d4_sno_names(opener);

        Ok(handler)
    }

    /// If this looks like a D4 archive (has `Base/CoreTOC.dat`), parse the TOC
    /// and rewrite raw SNO ID paths to human-readable names with group subfolders.
    fn resolve_d4_sno_names(&mut self, opener: &FileOpener<'_>) {
        use super::d4::CoreToc;

        // Look up CoreTOC.dat by its TVFS path.
        let toc_hash = jenkins96("Base/CoreTOC.dat");
        let toc_entries = self.get_all_entries(toc_hash);
        if toc_entries.is_empty() {
            // Also try lowercase (TVFS paths from D4 are lowercase).
            let toc_hash_lower = jenkins96("base/CoreTOC.dat");
            let entries = self.get_all_entries(toc_hash_lower);
            if entries.is_empty() {
                return; // Not a D4 archive.
            }
        }

        // Try to open the file using ekey from entries.
        let toc_hash = if !self
            .get_all_entries(jenkins96("Base/CoreTOC.dat"))
            .is_empty()
        {
            jenkins96("Base/CoreTOC.dat")
        } else {
            jenkins96("base/CoreTOC.dat")
        };

        let ckey = self.get_all_entries(toc_hash)[0].ckey;
        let ekey = match opener.encoding.best_ekey(&ckey) {
            Some(e) => e,
            None => return,
        };

        let toc_data = match opener.open_by_ekey(&ekey) {
            Ok(d) => d,
            Err(_) => return,
        };

        let toc = match CoreToc::parse(&toc_data) {
            Ok(t) => t,
            Err(_) => return,
        };

        // D4 TVFS paths look like:
        //   base/child/12345-0
        //   base/meta/12345
        //   base/payload/12345
        //
        // We want to rewrite them to:
        //   Base/child/Power/SomePower-0.pow
        //   Base/meta/Power/SomePower.pow
        //   Base/payload/Power/SomePower.pow

        let folders_to_remap = ["child", "meta", "payload", "paylow", "paymed"];

        // Collect all paths that need rewriting.
        let old_entries: Vec<(u64, String)> = self
            .file_paths
            .iter()
            .filter_map(|(&hash, path)| {
                let lower = path.to_lowercase();
                // Check if path is under one of the SNO folders.
                for folder in &folders_to_remap {
                    let prefixes = [
                        format!("base/{folder}/"),
                        // Also check locale-prefixed variants.
                    ];
                    for prefix in &prefixes {
                        if lower.starts_with(prefix) {
                            return Some((hash, path.clone()));
                        }
                    }
                }
                None
            })
            .collect();

        for (old_hash, old_path) in &old_entries {
            // Extract the folder prefix and the SNO part.
            // e.g. "base/child/12345-0" → prefix="base/child", remainder="12345-0"
            let parts: Vec<&str> = old_path.splitn(3, '/').collect();
            if parts.len() < 3 {
                continue;
            }
            let folder_prefix = format!("{}/{}", parts[0], parts[1]);
            let remainder = parts[2];

            // Parse the SNO ID and optional sub-id from the remainder.
            // Format: "12345" or "12345-0"
            let (sno_str, sub_id) = if let Some(dash_pos) = remainder.find('-') {
                (&remainder[..dash_pos], Some(&remainder[dash_pos..]))
            } else {
                (remainder, None)
            };

            let sno_id: i32 = match sno_str.parse() {
                Ok(id) => id,
                Err(_) => continue, // Not a numeric SNO ID, skip.
            };

            if let Some(sno) = toc.get(sno_id) {
                // Use the real name, or fall back to the SNO ID if name is blank.
                let display_name = if sno.name.trim().is_empty() {
                    format!("{sno_id}")
                } else {
                    sno.name.clone()
                };

                let new_path = if let Some(sub) = sub_id {
                    format!(
                        "{}/{}/{}{}{}",
                        folder_prefix, sno.group_name, display_name, sub, sno.ext
                    )
                } else {
                    format!(
                        "{}/{}/{}{}",
                        folder_prefix, sno.group_name, display_name, sno.ext
                    )
                };

                let new_hash = jenkins96(&new_path);

                // Move the entry from old hash to new hash.
                if let Some(root_entries) = self.entries.remove(old_hash) {
                    // Also store under old hash so lookups by original hash still work.
                    self.entries
                        .entry(new_hash)
                        .or_default()
                        .extend(root_entries.iter().cloned());
                    self.entries
                        .entry(*old_hash)
                        .or_default()
                        .extend(root_entries);
                }
                self.file_paths.insert(new_hash, new_path);
                // Remove old path so tree only shows renamed version.
                self.file_paths.remove(old_hash);
            }
        }
    }

    fn parse_directory_data(
        &mut self,
        header: &TvfsHeader,
        vfs_ekey_set: &HashSet<EKey9>,
        vfs_ekey_map: &HashMap<EKey9, Md5Hash>,
        opener: &FileOpener<'_>,
        path_buf: &mut Vec<u8>,
    ) -> Result<(), CascError> {
        let mut table = &header.path_table[..];

        // Check for initial folder node marker.
        if table.len() > 5 && table[0] == 0xFF {
            let node_value = read_u32_be(&table[1..5]);
            if node_value & TVFS_FOLDER_NODE == 0 {
                return Err(CascError::Config(
                    "TVFS: root path table entry is not a folder".into(),
                ));
            }
            table = &table[5..];
        }

        self.parse_path_file_table(header, vfs_ekey_set, vfs_ekey_map, opener, path_buf, table)
    }

    fn parse_path_file_table(
        &mut self,
        header: &TvfsHeader,
        vfs_ekey_set: &HashSet<EKey9>,
        vfs_ekey_map: &HashMap<EKey9, Md5Hash>,
        opener: &FileOpener<'_>,
        path_buf: &mut Vec<u8>,
        mut table: &[u8],
    ) -> Result<(), CascError> {
        let save_pos = path_buf.len();

        while !table.is_empty() {
            let (entry, rest) = capture_path_entry(table)?;
            table = rest;

            // Append path component.
            if entry.node_flags & TVFS_PTE_PATH_SEPARATOR_PRE != 0 {
                path_buf.push(b'/');
            }
            path_buf.extend_from_slice(&entry.name);
            if entry.node_flags & TVFS_PTE_PATH_SEPARATOR_POST != 0 {
                path_buf.push(b'/');
            }

            if entry.node_flags & TVFS_PTE_NODE_VALUE != 0 {
                if entry.node_value & TVFS_FOLDER_NODE != 0 {
                    // Sub-directory: recurse into the next N bytes of the table.
                    let dir_len = (entry.node_value & TVFS_FOLDER_SIZE_MASK) as usize - 4; // minus the 4-byte node value
                    if dir_len > table.len() {
                        return Err(CascError::Config(
                            "TVFS: folder size exceeds remaining path table".into(),
                        ));
                    }
                    let (sub_table, remaining) = table.split_at(dir_len);
                    self.parse_path_file_table(
                        header,
                        vfs_ekey_set,
                        vfs_ekey_map,
                        opener,
                        path_buf,
                        sub_table,
                    )?;
                    table = remaining;
                } else {
                    // File leaf: read spans from VFS table.
                    let vfs_offset = entry.node_value as usize;
                    if vfs_offset >= header.vfs_table.len() {
                        path_buf.truncate(save_pos);
                        continue;
                    }

                    let span_count = header.vfs_table[vfs_offset] as usize;
                    if span_count == 0 || span_count > 224 {
                        path_buf.truncate(save_pos);
                        continue;
                    }

                    let mut vfs_pos = vfs_offset + 1;
                    let item_size = 4 + 4 + header.cft_offs_size; // contentOffset + contentLength + cftOffset

                    if span_count == 1 {
                        // Single span — check if it's a VFS sub-directory.
                        let ekey = self.read_vfs_span_ekey(header, &mut vfs_pos, item_size)?;

                        let ekey9 = EKey9::from_full(&ekey);
                        if vfs_ekey_set.contains(&ekey9) {
                            // This is a sub-directory: open and recurse.
                            if let Some(full_ekey) = vfs_ekey_map.get(&ekey9) {
                                path_buf.push(b'/');
                                match opener.open_by_ekey(full_ekey) {
                                    Ok(sub_data) => {
                                        if let Ok(sub_header) = parse_header(&sub_data) {
                                            let _ = self.parse_directory_data(
                                                &sub_header,
                                                vfs_ekey_set,
                                                vfs_ekey_map,
                                                opener,
                                                path_buf,
                                            );
                                        }
                                    }
                                    Err(_) => { /* skip inaccessible sub-directory */ }
                                }
                            }
                        } else {
                            // Regular file.
                            self.add_file_entry(path_buf, &ekey, opener);
                        }
                    } else {
                        // Multi-span file: use the first span's ekey.
                        let first_ekey =
                            self.read_vfs_span_ekey(header, &mut vfs_pos, item_size)?;
                        // Skip remaining spans.
                        // Skip remaining spans (we only need the first).
                        let _ = vfs_pos;
                        self.add_file_entry(path_buf, &first_ekey, opener);
                    }
                }

                path_buf.truncate(save_pos);
            }
        }

        Ok(())
    }

    /// Read one VFS span entry and return the EKey from the CFT table.
    fn read_vfs_span_ekey(
        &self,
        header: &TvfsHeader,
        vfs_pos: &mut usize,
        item_size: usize,
    ) -> Result<Md5Hash, CascError> {
        let vfs = &header.vfs_table;
        if *vfs_pos + item_size > vfs.len() {
            return Err(CascError::Config("TVFS: VFS span overflows table".into()));
        }

        // Skip contentOffset (4) and contentLength (4), read cftOffset.
        let cft_offset = read_int_be(&vfs[*vfs_pos + 8..], header.cft_offs_size) as usize;
        *vfs_pos += item_size;

        // Read EKey from CFT table.
        let ekey_size = header.ekey_size as usize;
        if cft_offset + ekey_size > header.cft_table.len() {
            return Err(CascError::Config("TVFS: CFT offset out of bounds".into()));
        }

        let mut ekey_bytes = [0u8; 16];
        let copy_len = ekey_size.min(16);
        ekey_bytes[..copy_len]
            .copy_from_slice(&header.cft_table[cft_offset..cft_offset + copy_len]);

        Ok(Md5Hash::from_bytes(ekey_bytes))
    }

    /// Register a file entry: resolve the EKey to a CKey via encoding, then
    /// store the mapping from Jenkins96 path hash → RootEntry.
    fn add_file_entry(&mut self, path_buf: &[u8], ekey: &Md5Hash, opener: &FileOpener<'_>) {
        let path = String::from_utf8_lossy(path_buf).into_owned();
        let hash = jenkins96(&path);

        // Try to resolve EKey → CKey via encoding table.
        let ckey = opener
            .encoding
            .ckey_for_ekey(ekey)
            .copied()
            .unwrap_or(*ekey); // fall back to using ekey as ckey

        let entry = RootEntry {
            ckey,
            locale: LocaleFlags::ALL,
            content: ContentFlags::NONE,
        };

        self.entries.entry(hash).or_default().push(entry);
        self.file_paths.entry(hash).or_insert(path);
    }
}

impl RootHandler for TvfsRootHandler {
    fn count(&self) -> usize {
        self.entries.len()
    }

    fn get_all_entries(&self, hash: u64) -> &[RootEntry] {
        self.entries.get(&hash).map_or(&[], Vec::as_slice)
    }

    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_> {
        Box::new(
            self.entries
                .iter()
                .flat_map(|(&h, es)| es.iter().map(move |e| (h, e))),
        )
    }

    fn hash_for_file_data_id(&self, _id: u32) -> Option<u64> {
        None // TVFS doesn't use FileDataIds
    }

    fn file_data_id_for_hash(&self, _hash: u64) -> Option<u32> {
        None
    }

    fn builtin_paths(&self) -> Vec<(u64, String)> {
        self.file_paths
            .iter()
            .map(|(&h, p)| (h, p.clone()))
            .collect()
    }
}

// ── Header parsing ────────────────────────────────────────────────────────────

fn parse_header(data: &[u8]) -> Result<TvfsHeader, CascError> {
    if data.len() < 46 {
        return Err(CascError::Config("TVFS: data too short for header".into()));
    }

    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic != TVFS_MAGIC {
        return Err(CascError::Config(format!(
            "TVFS: bad magic 0x{magic:08X}, expected 0x{TVFS_MAGIC:08X}"
        )));
    }

    let format_version = data[4];
    if format_version != 1 {
        return Err(CascError::Config(format!(
            "TVFS: unsupported format version {format_version}"
        )));
    }

    let _header_size = data[5];
    let ekey_size = data[6];
    let _patch_key_size = data[7];

    // All offsets/sizes are big-endian from here.
    let _flags = read_u32_be(&data[8..12]);
    let path_table_offset = read_u32_be(&data[12..16]) as usize;
    let path_table_size = read_u32_be(&data[16..20]) as usize;
    let vfs_table_offset = read_u32_be(&data[20..24]) as usize;
    let vfs_table_size = read_u32_be(&data[24..28]) as usize;
    let cft_table_offset = read_u32_be(&data[28..32]) as usize;
    let cft_table_size = read_u32_be(&data[32..36]) as usize;
    let _max_depth = read_u16_be(&data[36..38]);
    let _est_table_offset = read_u32_be(&data[38..42]) as usize;
    let _est_table_size = read_u32_be(&data[42..46]) as usize;

    let cft_offs_size = offset_field_size(cft_table_size);

    let slice = |off: usize, sz: usize| -> Result<Vec<u8>, CascError> {
        data.get(off..off + sz)
            .map(|s| s.to_vec())
            .ok_or_else(|| CascError::Config("TVFS: table extends past end of data".into()))
    };

    Ok(TvfsHeader {
        ekey_size,
        path_table: slice(path_table_offset, path_table_size)?,
        vfs_table: slice(vfs_table_offset, vfs_table_size)?,
        cft_table: slice(cft_table_offset, cft_table_size)?,
        cft_offs_size,
    })
}

// ── Path table entry parsing ──────────────────────────────────────────────────

fn capture_path_entry(mut table: &[u8]) -> Result<(PathTableEntry, &[u8]), CascError> {
    let mut entry = PathTableEntry {
        name: Vec::new(),
        node_flags: 0,
        node_value: 0,
    };

    // Leading path separator (0x00).
    if !table.is_empty() && table[0] == 0x00 {
        entry.node_flags |= TVFS_PTE_PATH_SEPARATOR_PRE;
        table = &table[1..];
    }

    // Name bytes (length-prefixed, unless 0xFF which is the node value marker).
    if !table.is_empty() && table[0] != 0xFF {
        let len = table[0] as usize;
        if 1 + len > table.len() {
            return Err(CascError::Config("TVFS: path entry name overflows".into()));
        }
        entry.name = table[1..1 + len].to_vec();
        table = &table[1 + len..];
    }

    // Trailing path separator (0x00).
    if !table.is_empty() && table[0] == 0x00 {
        entry.node_flags |= TVFS_PTE_PATH_SEPARATOR_POST;
        table = &table[1..];
    }

    // Node value (0xFF marker + 4-byte big-endian value).
    if !table.is_empty() {
        if table[0] == 0xFF {
            if table.len() < 5 {
                return Err(CascError::Config(
                    "TVFS: path entry node value truncated".into(),
                ));
            }
            entry.node_value = read_u32_be(&table[1..5]);
            entry.node_flags |= TVFS_PTE_NODE_VALUE;
            table = &table[5..];
        } else {
            // Implicit post-separator (non-zero, non-0xFF byte follows).
            entry.node_flags |= TVFS_PTE_PATH_SEPARATOR_POST;
        }
    }

    Ok((entry, table))
}

// ── Binary helpers ────────────────────────────────────────────────────────────

fn read_u32_be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn read_u16_be(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

/// Read a variable-width big-endian integer (1–4 bytes).
fn read_int_be(b: &[u8], n: usize) -> u32 {
    let mut v: u32 = 0;
    for i in 0..n {
        v = (v << 8) | b[i] as u32;
    }
    v
}

/// Determine the byte width needed for an offset into a table of `size` bytes.
fn offset_field_size(size: usize) -> usize {
    if size > 0xFF_FFFF {
        4
    } else if size > 0xFFFF {
        3
    } else if size > 0xFF {
        2
    } else {
        1
    }
}
