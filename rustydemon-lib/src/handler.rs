use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read, Seek, SeekFrom},
    path::PathBuf,
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

/// Parse a listfile and build the filename map + folder tree off-thread.
///
/// Call [`CascHandler::fdid_hash_snapshot`] to obtain `fdid_hashes` before
/// spawning the background thread, then pass the result to
/// [`CascHandler::apply_listfile`] on the UI thread.
pub fn prepare_listfile(
    content: &str,
    fdid_hashes: &HashMap<u32, u64>,
) -> (HashMap<u64, String>, CascFolder) {
    let mut filenames = HashMap::new();
    let mut entries: Vec<(u64, String, Option<u32>)> = Vec::new();

    for (path, fdid) in parse_listfile(content) {
        let hash = fdid
            .and_then(|id| fdid_hashes.get(&id).copied())
            .unwrap_or_else(|| jenkins96(&path));

        filenames.insert(hash, path.clone());
        entries.push((hash, path, fdid));
    }

    let tree = build_tree(entries);
    (filenames, tree)
}

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
    /// Optional CDN fallback fetcher.  Only present when the `cdn` feature
    /// is compiled in and the install's `.build.info` carries CDN host
    /// info.  Used to load D2R 3.1.2+ loose metadata blobs that aren't in
    /// any local `.idx` or `.index` file.  Wrapped in `Arc` so
    /// `PreparedLoad` can clone-and-ship it to a background thread.
    #[cfg(feature = "cdn")]
    cdn: Option<std::sync::Arc<crate::cdn::CdnFetcher>>,
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

        // ── Local index (primary + optional ecache) ───────────────────────
        // D2R 3.1.2 splits its CASC across two storages: the traditional
        // `<data>/data/` (game assets) and a new `<data>/ecache/` (loose
        // metadata: ENCODING, DOWNLOAD, root manifests, TVFS tables).
        // `load_multi` silently skips directories that don't exist so
        // passing both unconditionally is safe for every other game.
        let data_path = config.data_path();
        let ecache_path = config.ecache_path();
        let mut local_index =
            LocalIndexHandler::load_multi(&[data_path.as_path(), ecache_path.as_path()])?;

        // D2R 3.1.2+ ships archive-style `<hash>.index` files alongside
        // the legacy `.idx` files.  If present, merge their entries into
        // the index map so reads can resolve ekeys against `data.NNN`
        // files via the CDN config's `archives = ...` ordering.  Called
        // AFTER load_multi so legacy `.idx` entries take precedence on
        // conflict (first-seen wins).
        let indices_path = config.archive_indices_path();
        if indices_path.is_dir() {
            let archive_hashes = config.archives();
            if !archive_hashes.is_empty() {
                local_index.merge_archive_indices(&indices_path, archive_hashes, 0)?;
            }
        }

        // ── Optional CDN fallback fetcher ─────────────────────────────────
        // Built before loading ENCODING so the encoding load path itself
        // can fall back to CDN — required for D2R 3.1.2+ where ENCODING
        // lives in `file-index` as a loose CDN blob, not in any local
        // `data.NNN` archive.
        #[cfg(feature = "cdn")]
        let cdn_fetcher: Option<std::sync::Arc<crate::cdn::CdnFetcher>> =
            if !config.cdn_hosts().is_empty() && !config.cdn_path().is_empty() {
                let cache_dir = config.data_path().join("cdn-cache");
                match crate::cdn::CdnFetcher::new(
                    config.cdn_hosts().to_vec(),
                    config.cdn_path().to_owned(),
                    cache_dir,
                ) {
                    Ok(f) => Some(std::sync::Arc::new(f)),
                    Err(e) => {
                        eprintln!("cdn: disabled — {e}");
                        None
                    }
                }
            } else {
                None
            };

        // Fallback read: try the local index first, then CDN (when the
        // `cdn` feature is compiled in and a fetcher was built).  Used for
        // ENCODING and for the legacy root manifest — both can be loose
        // blobs on D2R 3.1.2+.
        #[allow(unused_variables)] // cdn arg is unused when feature is off
        let read_metadata_blob = |ekey: &Md5Hash| -> Result<Vec<u8>, CascError> {
            match local_index.read_block(ekey) {
                Ok(bytes) => Ok(bytes),
                Err(local_err) => {
                    #[cfg(feature = "cdn")]
                    if let Some(fetcher) = cdn_fetcher.as_ref() {
                        match fetcher.fetch(ekey) {
                            Ok(bytes) => return Ok(bytes),
                            Err(cdn_err) => {
                                return Err(CascError::Config(format!(
                                    "read {} failed locally ({local_err}) and via CDN ({cdn_err})",
                                    ekey.to_hex()
                                )));
                            }
                        }
                    }
                    Err(local_err)
                }
            }
        };

        // ── Encoding file ──────────────────────────────────────────────────
        let enc_ekey = config
            .encoding_ekey()
            .ok_or_else(|| CascError::Config("build config missing encoding ekey".into()))?;

        let enc_data = read_metadata_blob(&enc_ekey)?;
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

            let root_data = read_metadata_blob(&root_ekey)?;

            let root_decoded = blte::decode(&root_data, &root_ekey, false)?;
            let mut handler = root::load(root_decoded)?;

            // Fallback 1: D2R 3.1.2+ ships a non-zero `root` that's a
            // legacy stub no current root handler recognises, plus a real
            // `vfs-root` alongside.  When the traditional root returns
            // Dummy AND the build config has a vfs-root entry, try TVFS
            // before giving up on the root manifest.  Retail WoW also
            // ships both fields populated, but its `root` is a valid
            // MFST so this branch never fires there — MFST wins and WoW
            // keeps working.
            if handler.type_name() == "Dummy" && config.vfs_root_ekey().is_some() {
                let vfs_ekey = config.vfs_root_ekey().unwrap();
                let vfs_list = config.vfs_root_list();
                let opener = crate::root::tvfs::LocalFileOpener {
                    encoding: &encoding,
                    local_index: &local_index,
                };
                match TvfsRootHandler::load(&vfs_ekey, &vfs_list, &opener) {
                    Ok(tvfs) => {
                        eprintln!(
                            "root fallback: TVFS loaded ({} entries) — legacy root was unrecognised",
                            tvfs.count()
                        );
                        handler = Box::new(tvfs);
                    }
                    Err(e) => {
                        eprintln!("root fallback: TVFS failed: {e}");
                    }
                }
            }

            // Fallback 2: if the root manifest format still isn't
            // recognised (e.g. SC1 Remastered), use the INSTALL manifest
            // as the authoritative file list.  Every CASC install has one.
            if handler.type_name() == "Dummy" {
                match load_install_as_root(&config, &encoding, &local_index) {
                    Ok(install) => {
                        eprintln!(
                            "root fallback: INSTALL loaded ({} entries)",
                            install.count()
                        );
                        handler = Box::new(install);
                    }
                    Err(e) => {
                        eprintln!("root fallback: INSTALL failed: {e}");
                    }
                }
            }

            handler
        };

        Ok(CascHandler {
            config,
            encoding: Some(encoding),
            local_index: Some(local_index),
            static_container: None,
            #[cfg(feature = "cdn")]
            cdn: cdn_fetcher,
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
            #[cfg(feature = "cdn")]
            cdn: None,
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

    /// Look up the filename for a hash, if known.
    pub fn filename_for_hash(&self, hash: u64) -> Option<String> {
        self.filenames.get(&hash).cloned()
    }

    /// Whether the root handler already provides a complete built-in path
    /// table (MNDX, TVFS). When true, loading an external listfile is
    /// unnecessary and will clobber the working tree with unrelated paths.
    pub fn has_builtin_paths(&self) -> bool {
        self.root.has_builtin_paths()
    }

    /// Short name of the root handler format (e.g. "MNDX", "MFST (WoW)", "TVFS").
    pub fn root_type_name(&self) -> &'static str {
        self.root.type_name()
    }

    /// Number of entries in the filename map (populated from builtin paths
    /// and/or listfile).
    pub fn filename_count(&self) -> usize {
        self.filenames.len()
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

    /// Snapshot the FileDataId → hash mapping so a background thread can
    /// resolve listfile entries without holding a reference to the handler.
    pub fn fdid_hash_snapshot(&self) -> HashMap<u32, u64> {
        self.root.fdid_hash_map()
    }

    /// Apply pre-computed listfile results produced by [`prepare_listfile`].
    ///
    /// When the handler already has a populated filename map (e.g. from
    /// [`load_builtin_paths`](Self::load_builtin_paths) on an MNDX/TVFS game),
    /// the new entries are merged in rather than replacing the existing tree.
    /// This prevents a mismatched listfile from clobbering a working state.
    pub fn apply_listfile(&mut self, filenames: HashMap<u64, String>, tree: CascFolder) {
        if self.filenames.is_empty() {
            self.filenames = filenames;
            self.root_folder = Some(tree);
            return;
        }

        // Merge: add new filenames that don't conflict, rebuild the tree.
        for (hash, path) in filenames {
            self.filenames.entry(hash).or_insert(path);
        }
        let entries: Vec<(u64, String, Option<u32>)> = self
            .filenames
            .iter()
            .map(|(&h, p)| (h, p.clone(), None))
            .collect();
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
    ///
    /// Try local first; on any failure (lookup miss, read error, or BLTE
    /// decode error from stale/partial local data) fall back to CDN when
    /// the `cdn` feature is compiled in and a fetcher was configured.
    /// This matches olegbl/CascLib's D2R 3.1.2+ fix where local `.idx`
    /// entries can be stale or truncated and need a CDN-fetched blob to
    /// supersede them.
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

        // Attempt 1: full local read + BLTE decode.  `read_block` routes
        // the lookup to the correct storage (primary / ecache / archive
        // indices) so we don't care which one holds the ekey.
        let local_attempt = local_index
            .read_block(ekey)
            .and_then(|raw| blte::decode(&raw, ekey, self.validate_hashes));

        match local_attempt {
            Ok(bytes) => Ok(bytes),
            Err(local_err) => {
                #[cfg(feature = "cdn")]
                {
                    if let Some(fetcher) = self.cdn.as_ref() {
                        let cdn_bytes = fetcher.fetch(ekey).map_err(|cdn_err| {
                            CascError::Config(format!(
                                "read {} failed locally ({local_err}) and via CDN ({cdn_err})",
                                ekey.to_hex()
                            ))
                        })?;
                        return blte::decode(&cdn_bytes, ekey, self.validate_hashes);
                    }
                }
                Err(local_err)
            }
        }
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

    /// Resolve a content key to everything a background thread needs to read
    /// and decompress the file, without holding a reference to the handler.
    ///
    /// The returned [`PreparedLoad`] is `Send` and can be executed on any
    /// thread via [`PreparedLoad::execute`].  This keeps the heavy file I/O
    /// and BLTE decompression off the UI thread.
    ///
    /// Only works for traditional CASC (`.idx`-based).  For static containers
    /// use [`CascHandler::open_by_ckey`] directly.
    pub fn prepare_load(&self, ckey: &Md5Hash) -> Result<PreparedLoad, CascError> {
        let ekey = match &self.encoding {
            Some(enc) => enc
                .best_ekey(ckey)
                .ok_or_else(|| CascError::EncodingNotFound(ckey.to_hex()))?,
            None => *ckey,
        };

        let local_index = self
            .local_index
            .as_ref()
            .ok_or_else(|| CascError::Config("no storage backend configured".into()))?;

        // If we have a local entry, route it to the correct storage
        // directory (primary `<data>/data/`, secondary `<data>/ecache/`,
        // or whatever else `load_multi` + `merge_archive_indices` added).
        // Missing entries aren't fatal when we have a CDN fetcher — the
        // background thread will fall through to CDN on execute.
        let (storage_dir, archive_index, offset, size) = match local_index.get_entry(&ekey) {
            Some(idx) => {
                let dir = local_index
                    .storages()
                    .get(idx.storage as usize)
                    .cloned()
                    .ok_or_else(|| {
                        CascError::InvalidData(format!(
                            "ekey {} references unknown storage id {}",
                            ekey.to_hex(),
                            idx.storage
                        ))
                    })?;
                (Some(dir), idx.index, idx.offset, idx.size)
            }
            None => {
                // No local entry — the read will go straight to CDN if the
                // fetcher is available, otherwise `execute` returns
                // IndexNotFound so the GUI can surface the error.
                (None, 0, 0, 0)
            }
        };

        Ok(PreparedLoad {
            storage_dir,
            archive_index,
            offset,
            size,
            ekey,
            validate_hashes: self.validate_hashes,
            #[cfg(feature = "cdn")]
            cdn: self.cdn.clone(),
        })
    }
}

/// All the data a background thread needs to read and decompress a file
/// from the CASC archives.
///
/// Created by [`CascHandler::prepare_load`] on the UI thread (fast hash
/// lookups), then sent to a worker thread for the heavy I/O + BLTE decode.
///
/// Carries the correct storage directory (so ecache- and archive-index-
/// routed entries resolve to the right `data.NNN` file) and an optional
/// clone of the handler's CDN fetcher (so loose-blob reads fall back to
/// CDN without holding a handler reference).
pub struct PreparedLoad {
    /// `None` when there's no local entry at all — execute() must fall
    /// straight to CDN.  `Some(dir)` points at whichever storage directory
    /// owns `data.{archive_index}`.
    storage_dir: Option<PathBuf>,
    archive_index: u32,
    offset: u32,
    size: u32,
    ekey: Md5Hash,
    validate_hashes: bool,
    #[cfg(feature = "cdn")]
    cdn: Option<std::sync::Arc<crate::cdn::CdnFetcher>>,
}

impl PreparedLoad {
    /// Execute the file read + BLTE decompression (consuming).
    ///
    /// This is the expensive part that should run on a background thread.
    pub fn execute(self) -> Result<Vec<u8>, CascError> {
        self.execute_ref()
    }

    /// Execute the file read + BLTE decompression (borrowing).
    ///
    /// Same as [`execute`](Self::execute) but borrows, allowing the same
    /// `PreparedLoad` to be retried or used multiple times.  Mirrors
    /// [`CascHandler::open_by_ekey`]'s try-local-then-CDN logic so the
    /// GUI's background loader gets the same D2R 3.1.2 fallback the
    /// synchronous API does.
    pub fn execute_ref(&self) -> Result<Vec<u8>, CascError> {
        // Attempt 1: local read + BLTE decode, skipped entirely when
        // there was no local index entry.
        let local_attempt: Result<Vec<u8>, CascError> = match self.storage_dir.as_ref() {
            Some(dir) => read_data_block(dir, self.archive_index, self.offset, self.size)
                .and_then(|raw| blte::decode(&raw, &self.ekey, self.validate_hashes)),
            None => Err(CascError::IndexNotFound(self.ekey.to_hex())),
        };

        match local_attempt {
            Ok(bytes) => Ok(bytes),
            Err(local_err) => {
                #[cfg(feature = "cdn")]
                {
                    if let Some(fetcher) = self.cdn.as_ref() {
                        let cdn_bytes = fetcher.fetch(&self.ekey).map_err(|cdn_err| {
                            CascError::Config(format!(
                                "read {} failed locally ({local_err}) and via CDN ({cdn_err})",
                                self.ekey.to_hex()
                            ))
                        })?;
                        return blte::decode(&cdn_bytes, &self.ekey, self.validate_hashes);
                    }
                }
                Err(local_err)
            }
        }
    }
}

// ── Data archive reader ────────────────────────────────────────────────────────

/// Try to load the INSTALL manifest and parse it as a root handler.
fn load_install_as_root(
    config: &CascConfig,
    encoding: &EncodingHandler,
    local_index: &LocalIndexHandler,
) -> Result<crate::root::install::InstallRootHandler, CascError> {
    let install_ckey = config
        .install_ckey()
        .ok_or_else(|| CascError::Config("build config missing install ckey".into()))?;

    let install_ekey = encoding
        .best_ekey(&install_ckey)
        .ok_or_else(|| CascError::EncodingNotFound(install_ckey.to_hex()))?;

    let entry = local_index
        .get_entry(&install_ekey)
        .ok_or_else(|| CascError::IndexNotFound(install_ekey.to_hex()))?;

    let raw = read_data_block(&config.data_path(), entry.index, entry.offset, entry.size)?;
    let decoded = blte::decode(&raw, &install_ekey, false)?;

    crate::root::install::InstallRootHandler::parse(&decoded)
}

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
