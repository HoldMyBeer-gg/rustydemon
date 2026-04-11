use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read, Seek, SeekFrom},
    sync::Mutex,
};

use crate::{
    blte,
    config::CascConfig,
    encoding::EncodingHandler,
    entry::{build_tree, parse_listfile, CascFolder},
    error::CascError,
    jenkins96::jenkins96,
    local_index::LocalIndexHandler,
    root::{self, RootHandler},
    types::{LocaleFlags, Md5Hash},
};

// How many bytes precede the BLTE payload in each data.NNN block.
const DATA_HEADER_BYTES: u64 = 30;

/// Top-level CASC storage handler for local installations.
///
/// Loads the index files, encoding table, and root manifest from a game
/// directory, then exposes methods to open and extract individual files.
///
/// # Example
///
/// ```no_run
/// use rustydemon_lib::{CascHandler, LocaleFlags};
///
/// let mut casc = CascHandler::open_local("/path/to/game", "wow")?;
/// casc.set_locale(LocaleFlags::EN_US);
///
/// let data = casc.open_file_by_name("Interface/Glues/Models/UI_MainMenu/UI_MainMenu.m2")?;
/// println!("Read {} bytes", data.len());
/// # Ok::<(), rustydemon_lib::CascError>(())
/// ```
pub struct CascHandler {
    /// Parsed configuration (paths, keys, etc.).
    pub config: CascConfig,

    encoding: EncodingHandler,
    local_index: LocalIndexHandler,
    pub(crate) root: Box<dyn RootHandler>,

    /// Active locale filter applied to root lookups.
    locale: LocaleFlags,

    /// Navigable file tree (populated after [`CascHandler::load_listfile`]).
    pub root_folder: Option<CascFolder>,

    /// Flat hash → filename map (populated from listfile).
    pub(crate) filenames: HashMap<u64, String>,

    /// Cached open file handles for data.NNN archives (lazily opened).
    #[allow(dead_code)]
    data_files: Mutex<HashMap<u32, fs::File>>,

    /// Whether to validate MD5 hashes during BLTE decoding.
    pub validate_hashes: bool,
}

impl CascHandler {
    /// Open a local CASC installation.
    ///
    /// This loads the index files, encoding file, and root manifest.  It does
    /// *not* load a listfile; call [`CascHandler::load_listfile`] separately.
    pub fn open_local(
        base_path: impl AsRef<std::path::Path>,
        product: &str,
    ) -> Result<Self, CascError> {
        let config = CascConfig::load_local(base_path, product)?;

        // ── Local index ────────────────────────────────────────────────────
        let local_index = LocalIndexHandler::load(config.data_path())?;

        // ── Encoding file ──────────────────────────────────────────────────
        let enc_ekey = config
            .encoding_ekey()
            .ok_or_else(|| CascError::Config("build config missing encoding ekey".into()))?;

        let enc_data = {
            let entry = local_index
                .get_entry(&enc_ekey)
                .ok_or_else(|| CascError::IndexNotFound(enc_ekey.to_hex()))?;
            read_data_block(&config.data_path(), entry.index, entry.offset, entry.size)?
        };

        let enc_decoded = blte::decode(&enc_data, &enc_ekey, false)?;
        let encoding = EncodingHandler::from_reader(Cursor::new(enc_decoded))?;

        // ── Root manifest ──────────────────────────────────────────────────
        let root_ckey = config
            .root_ckey()
            .ok_or_else(|| CascError::Config("build config missing root ckey".into()))?;

        let root_ekey = encoding
            .best_ekey(&root_ckey)
            .ok_or_else(|| CascError::EncodingNotFound(root_ckey.to_hex()))?;

        let root_data = {
            let entry = local_index
                .get_entry(&root_ekey)
                .ok_or_else(|| CascError::IndexNotFound(root_ekey.to_hex()))?;
            read_data_block(&config.data_path(), entry.index, entry.offset, entry.size)?
        };

        let root_decoded = blte::decode(&root_data, &root_ekey, false)?;
        let root_handler = root::load(root_decoded)?;

        Ok(CascHandler {
            config,
            encoding,
            local_index,
            root: root_handler,
            locale: LocaleFlags::ALL_WOW,
            root_folder: None,
            filenames: HashMap::new(),
            data_files: Mutex::new(HashMap::new()),
            validate_hashes: false,
        })
    }

    // ── Configuration ──────────────────────────────────────────────────────────

    /// Set the locale used when resolving files that exist in multiple locales.
    pub fn set_locale(&mut self, locale: LocaleFlags) {
        self.locale = locale;
    }

    /// Return the currently active locale.
    pub fn locale(&self) -> LocaleFlags {
        self.locale
    }

    /// Number of entries in the root manifest.
    pub fn root_count(&self) -> usize {
        self.root.count()
    }

    /// Number of entries in the encoding table.
    pub fn encoding_count(&self) -> usize {
        self.encoding.count()
    }

    /// Number of entries in the local index.
    pub fn local_index_count(&self) -> usize {
        self.local_index.count()
    }

    // ── Listfile ───────────────────────────────────────────────────────────────

    /// Load a community listfile and build the virtual file tree.
    ///
    /// The listfile may use either `path\npath\n…` or `fileDataId;path\n…`
    /// format.  Both formats can be mixed in the same file.
    pub fn load_listfile(&mut self, content: &str) {
        let mut entries: Vec<(u64, String, Option<u32>)> = Vec::new();

        for (path, fdid) in parse_listfile(content) {
            let hash = self
                .root
                .hash_for_file_data_id(fdid.unwrap_or(0))
                .unwrap_or_else(|| jenkins96(&path));

            self.filenames.insert(hash, path.clone());
            entries.push((hash, path, fdid));
        }

        self.root_folder = Some(build_tree(entries));
    }

    /// Look up the filename for a hash (requires listfile to be loaded).
    pub fn filename(&self, hash: u64) -> Option<&str> {
        self.filenames.get(&hash).map(std::string::String::as_str)
    }

    // ── File existence ─────────────────────────────────────────────────────────

    /// Return `true` if the file is present in the root manifest.
    pub fn file_exists_by_name(&self, path: &str) -> bool {
        let hash = jenkins96(path);
        !self.root.get_all_entries(hash).is_empty()
    }

    /// Return `true` if the file is present in the root manifest.
    pub fn file_exists_by_hash(&self, hash: u64) -> bool {
        !self.root.get_all_entries(hash).is_empty()
    }

    /// Return `true` if a file with this FileDataId exists.
    pub fn file_exists_by_fdid(&self, id: u32) -> bool {
        self.root
            .hash_for_file_data_id(id)
            .is_some_and(|h| self.file_exists_by_hash(h))
    }

    // ── File opening ───────────────────────────────────────────────────────────

    /// Decode and return the raw bytes of a file by its virtual path.
    pub fn open_file_by_name(&self, path: &str) -> Result<Vec<u8>, CascError> {
        let hash = jenkins96(path);
        self.open_file_by_hash(hash).map_err(|e| match e {
            CascError::FileNotFound(_) => CascError::FileNotFound(path.to_owned()),
            other => other,
        })
    }

    /// Decode and return the raw bytes of a file by its Jenkins96 hash.
    pub fn open_file_by_hash(&self, hash: u64) -> Result<Vec<u8>, CascError> {
        let ckey = self.resolve_hash(hash)?;
        self.open_by_ckey(&ckey)
    }

    /// Decode and return the raw bytes of a file by FileDataId.
    pub fn open_file_by_fdid(&self, id: u32) -> Result<Vec<u8>, CascError> {
        let hash = self
            .root
            .hash_for_file_data_id(id)
            .ok_or_else(|| CascError::FileNotFound(format!("FileDataId {id}")))?;
        self.open_file_by_hash(hash)
    }

    /// Decode and return the raw bytes of a file by content key.
    pub fn open_by_ckey(&self, ckey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let ekey = self
            .encoding
            .best_ekey(ckey)
            .ok_or_else(|| CascError::EncodingNotFound(ckey.to_hex()))?;
        self.open_by_ekey(&ekey)
    }

    /// Decode and return the raw bytes of a file by encoding key.
    pub fn open_by_ekey(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let idx = self
            .local_index
            .get_entry(ekey)
            .ok_or_else(|| CascError::IndexNotFound(ekey.to_hex()))?;

        let raw = read_data_block(&self.config.data_path(), idx.index, idx.offset, idx.size)?;

        blte::decode(&raw, ekey, self.validate_hashes)
    }

    // ── Internal helpers ───────────────────────────────────────────────────────

    /// Resolve a filename hash to a content key using the root manifest.
    fn resolve_hash(&self, hash: u64) -> Result<Md5Hash, CascError> {
        let entries = self.root.get_entries(hash, self.locale);

        if entries.is_empty() {
            // Fall back to any locale.
            let all = self.root.get_all_entries(hash);
            if all.is_empty() {
                return Err(CascError::FileNotFound(format!("hash {hash:016X}")));
            }
            return Ok(all[0].ckey);
        }

        Ok(entries[0].ckey)
    }
}

// ── Data archive reader ────────────────────────────────────────────────────────

/// Read one data block from a `data.NNN` archive file.
///
/// Each block starts with a 30-byte header (16-byte reversed eKey, 4-byte
/// size, 10 unknown bytes).  The BLTE payload follows immediately after.
fn read_data_block(
    data_dir: &std::path::Path,
    archive_index: u32,
    offset: u32,
    size: u32,
) -> Result<Vec<u8>, CascError> {
    let path = data_dir.join(format!("data.{archive_index:03}"));

    let mut file = fs::File::open(&path).map_err(CascError::Io)?;

    let blte_offset = (offset as u64)
        .checked_add(DATA_HEADER_BYTES)
        .ok_or(CascError::Overflow("data block BLTE offset"))?;

    let blte_size = (size as u64)
        .checked_sub(DATA_HEADER_BYTES)
        .ok_or(CascError::Overflow("data block BLTE size"))?;

    file.seek(SeekFrom::Start(blte_offset))?;

    let mut buf = vec![0u8; blte_size as usize];
    file.read_exact(&mut buf)?;

    Ok(buf)
}
