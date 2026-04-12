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
    root::{self, tvfs::TvfsRootHandler, RootHandler},
    static_container::StaticContainer,
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

    /// Encoding table — absent for static-container installations.
    encoding: Option<EncodingHandler>,
    /// Local index files (`*.idx`) — absent for static-container installations.
    local_index: Option<LocalIndexHandler>,
    /// Static container backend — present only for Steam D4/OW-style builds.
    static_container: Option<StaticContainer>,
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
        let base_path = base_path.as_ref();

        // Steam-style static-container installs ship only a `.build.config`
        // (no `.build.info`, no CDN config).  Detect that case by looking
        // for the file directly and route to the static path.
        let static_candidates = [
            base_path.join(".build.config"),
            base_path.join("Data").join(".build.config"),
        ];
        let has_build_info = base_path.join(".build.info").is_file();
        let has_static_cfg = static_candidates.iter().any(|p| p.is_file());

        if has_static_cfg {
            // Prefer the static container path whenever a `.build.config`
            // exists — even if `.build.info` is also present (Windows Steam
            // D4 ships both, but only the static config has key-layout fields).
            let static_config = CascConfig::load_local_static(base_path)?;
            if static_config.is_static_container() {
                return Self::finish_static(static_config);
            }
        }

        if !has_build_info {
            return Err(CascError::Config(format!(
                "No .build.info found in {}",
                base_path.display()
            )));
        }

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
        let root_handler: Box<dyn RootHandler> = if config.is_vfs_root() {
            // Newer TVFS-based root (D4, OW2, etc.).
            let vfs_ekey = config
                .vfs_root_ekey()
                .ok_or_else(|| CascError::Config("build config missing vfs-root ekey".into()))?;

            let vfs_list = config.vfs_root_list();

            let opener = crate::root::tvfs::LocalFileOpener {
                encoding: &encoding,
                local_index: &local_index,
                data_path: config.data_path(),
            };

            let tvfs = TvfsRootHandler::load(&vfs_ekey, &vfs_list, &opener)?;
            Box::new(tvfs) as Box<dyn RootHandler>
        } else {
            // Traditional root manifest (WoW, D3, etc.).
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
            root::load(root_decoded)?
        };

        Ok(CascHandler {
            config,
            encoding: Some(encoding),
            local_index: Some(local_index),
            static_container: None,
            root: root_handler,
            locale: LocaleFlags::ALL_WOW,
            root_folder: None,
            filenames: HashMap::new(),
            data_files: Mutex::new(HashMap::new()),
            validate_hashes: false,
        })
    }

    /// Open a local static-container install (Steam Diablo IV / Overwatch).
    ///
    /// Static containers have no `.idx` index files and no encoding file:
    /// every EKey encodes its own storage location through the `key-layout-*`
    /// bit fields in the build config.  The VFS root is loaded directly from
    /// the container using the EKey-driven backend.
    fn finish_static(config: CascConfig) -> Result<Self, CascError> {
        // Static containers store chunk directories (`000/`, `001/`, …)
        // directly under `<base>/Data/`, one level higher than the
        // traditional `<base>/Data/data/` layout.
        let container = StaticContainer::from_config(config.static_container_path(), &config)?;

        // ── VFS root ───────────────────────────────────────────────────────
        // Static containers use a TVFS-only layout: there is no `root` field,
        // only `vfs-root` + `vfs-N` entries.
        let vfs_ekey = config
            .vfs_root_ekey()
            .ok_or_else(|| CascError::Config("static container: vfs-root missing".into()))?;
        let vfs_list = config.vfs_root_list();

        let opener = crate::root::tvfs::StaticFileOpener {
            container: &container,
        };

        let tvfs = TvfsRootHandler::load(&vfs_ekey, &vfs_list, &opener)?;

        Ok(CascHandler {
            config,
            encoding: None,
            local_index: None,
            static_container: Some(container),
            root: Box::new(tvfs),
            locale: LocaleFlags::ALL,
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

    /// Number of entries in the encoding table (0 for static containers).
    pub fn encoding_count(&self) -> usize {
        self.encoding.as_ref().map_or(0, EncodingHandler::count)
    }

    /// Number of entries in the local index (0 for static containers).
    pub fn local_index_count(&self) -> usize {
        self.local_index
            .as_ref()
            .map_or(0, LocalIndexHandler::count)
    }

    /// `true` if this installation uses a static container backend.
    pub fn is_static_container(&self) -> bool {
        self.static_container.is_some()
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

    /// Populate the file tree from the root handler's built-in path table
    /// (e.g. TVFS manifests that already know all file paths).
    ///
    /// This is a no-op if the root handler doesn't provide built-in paths.
    pub fn load_builtin_paths(&mut self) {
        let paths = self.root.builtin_paths();
        if paths.is_empty() {
            return;
        }

        let mut entries: Vec<(u64, String, Option<u32>)> = Vec::new();
        for (hash, path) in paths {
            self.filenames.insert(hash, path.clone());
            entries.push((hash, path, None));
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
        // Static containers have no encoding table: the CKey is the EKey.
        let ekey = match &self.encoding {
            Some(enc) => enc
                .best_ekey(ckey)
                .ok_or_else(|| CascError::EncodingNotFound(ckey.to_hex()))?,
            None => *ckey,
        };
        self.open_by_ekey(&ekey)
    }

    /// Decode and return the raw bytes of a file by encoding key.
    pub fn open_by_ekey(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        if let Some(container) = &self.static_container {
            // `open_by_ekey` autodetects BLTE vs raw zlib (D4's VFS roots
            // and locale manifests use `espec = z`, not BLTE).
            return container.open_by_ekey(ekey);
        }

        let local_index = self
            .local_index
            .as_ref()
            .ok_or_else(|| CascError::Config("no storage backend configured".into()))?;

        let idx = local_index
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
pub(crate) fn read_data_block(
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
