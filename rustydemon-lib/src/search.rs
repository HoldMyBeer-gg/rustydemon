use crate::{
    handler::CascHandler,
    types::{ContentFlags, LocaleFlags, Md5Hash},
};

/// A single search result from [`CascHandler::search`].
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Jenkins96 hash of the file.
    pub hash: u64,
    /// Resolved filename (only available when a listfile is loaded and the
    /// hash was found in it).
    pub filename: Option<String>,
    /// FileDataId, if this game uses them and the id is known.
    pub file_data_id: Option<u32>,
    /// Locale flags from the root entry.
    pub locale: LocaleFlags,
    /// Content flags from the root entry.
    pub content: ContentFlags,
    /// Content key for this root entry.
    pub ckey: Md5Hash,
}

/// A query to pass to [`CascHandler::search`].
///
/// All fields are optional filters; unset fields match everything.
/// Multiple set fields are ANDed together.
#[derive(Debug, Default)]
pub struct SearchQuery {
    /// Case-insensitive substring to match against the filename.
    /// Only meaningful when a listfile is loaded.
    pub filename_contains: Option<String>,

    /// Hex prefix to match against the Jenkins96 hash.
    pub hash_prefix: Option<String>,

    /// Hex prefix to match against the content key hex string.
    pub ckey_prefix: Option<String>,

    /// If set, only return entries whose locale intersects this mask.
    pub locale: Option<LocaleFlags>,

    /// If set, only return entries whose content flags intersect this mask.
    pub content: Option<ContentFlags>,

    /// Maximum number of results to return (0 = unlimited).
    pub limit: usize,
}

impl SearchQuery {
    /// Create a new empty query (matches all entries).
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by a case-insensitive filename substring.
    pub fn filename(mut self, s: impl Into<String>) -> Self {
        self.filename_contains = Some(s.into().to_lowercase());
        self
    }

    /// Filter by a hash hex prefix (case-insensitive, e.g. `"1A2B"`).
    pub fn hash(mut self, prefix: impl Into<String>) -> Self {
        self.hash_prefix = Some(prefix.into().to_uppercase());
        self
    }

    /// Filter by a content-key hex prefix.
    pub fn ckey(mut self, prefix: impl Into<String>) -> Self {
        self.ckey_prefix = Some(prefix.into().to_uppercase());
        self
    }

    /// Restrict to entries with matching locale bits.
    pub fn locale(mut self, flags: LocaleFlags) -> Self {
        self.locale = Some(flags);
        self
    }

    /// Restrict to entries with matching content bits.
    pub fn content(mut self, flags: ContentFlags) -> Self {
        self.content = Some(flags);
        self
    }

    /// Cap the result count.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = n;
        self
    }
}

impl CascHandler {
    /// Search the entire CASC storage for files matching `query`.
    ///
    /// Unlike a folder-scoped search, this iterates *every* entry in the root
    /// manifest — exactly like regedit's "Find" which searches all keys and
    /// values, not just the currently selected subtree.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use rustydemon_lib::{CascHandler, SearchQuery};
    ///
    /// let casc = CascHandler::open_local("/path/to/game", "wow")?;
    ///
    /// // Find all known .m2 files in the en-US locale.
    /// let results = casc.search(
    ///     SearchQuery::new()
    ///         .filename(".m2")
    ///         .locale(rustydemon_lib::LocaleFlags::EN_US)
    ///         .limit(100)
    /// );
    ///
    /// for r in results {
    ///     println!("{:016X}  {:?}", r.hash, r.filename);
    /// }
    /// # Ok::<(), rustydemon_lib::CascError>(())
    /// ```
    pub fn search(&self, query: SearchQuery) -> Vec<SearchResult> {
        let fname_filter = query.filename_contains.as_deref();
        let hash_filter = query.hash_prefix.as_deref();
        let ckey_filter = query.ckey_prefix.as_deref();
        let limit = if query.limit == 0 {
            usize::MAX
        } else {
            query.limit
        };

        let mut results = Vec::new();

        for (hash, entry) in self.root.all_entries() {
            if results.len() >= limit {
                break;
            }

            // ── Locale filter ──────────────────────────────────────────────
            if let Some(lf) = query.locale {
                if !entry.locale.intersects(lf) {
                    continue;
                }
            }

            // ── Content-flag filter ────────────────────────────────────────
            if let Some(cf) = query.content {
                if !entry.content.intersects(cf) {
                    continue;
                }
            }

            // ── Hash prefix filter ─────────────────────────────────────────
            if let Some(pfx) = hash_filter {
                let hash_hex = format!("{hash:016X}");
                if !hash_hex.starts_with(pfx) {
                    continue;
                }
            }

            // ── CKey prefix filter ─────────────────────────────────────────
            if let Some(pfx) = ckey_filter {
                if !entry.ckey.to_hex().starts_with(pfx) {
                    continue;
                }
            }

            // ── Filename filter ────────────────────────────────────────────
            let filename: Option<String> = self.filenames.get(&hash).cloned();

            if let Some(needle) = fname_filter {
                match &filename {
                    Some(name) if name.to_lowercase().contains(needle) => {}
                    _ => continue,
                }
            }

            // ── FileDataId ─────────────────────────────────────────────────
            let file_data_id = self.root.file_data_id_for_hash(hash);

            results.push(SearchResult {
                hash,
                filename,
                file_data_id,
                locale: entry.locale,
                content: entry.content,
                ckey: entry.ckey,
            });
        }

        results
    }

    /// Search by exact filename hash.
    ///
    /// Returns all root-manifest entries for that hash (one per locale /
    /// content variant), without requiring a listfile.
    pub fn search_by_hash(&self, hash: u64) -> Vec<SearchResult> {
        let filename = self.filenames.get(&hash).cloned();
        let file_data_id = self.root.file_data_id_for_hash(hash);

        self.root
            .get_all_entries(hash)
            .iter()
            .map(|entry| SearchResult {
                hash,
                filename: filename.clone(),
                file_data_id,
                locale: entry.locale,
                content: entry.content,
                ckey: entry.ckey,
            })
            .collect()
    }

    /// Run a glob/path query against the virtual file tree and return one
    /// [`SearchResult`] per matched file.
    ///
    /// Requires a populated [`root_folder`](CascHandler::root_folder) — i.e.
    /// the handler has either a TVFS built-in path table or a loaded
    /// listfile.  Returns an empty vec when the query is valid but matches
    /// nothing, or when the handler has no tree at all.
    pub fn search_by_path_query(
        &self,
        query: &crate::query::PathQuery,
        limit: usize,
    ) -> Vec<SearchResult> {
        let Some(tree) = self.root_folder.as_ref() else {
            return Vec::new();
        };
        let Ok(files) = query.resolve(tree) else {
            return Vec::new();
        };

        let cap = if limit == 0 { usize::MAX } else { limit };
        let mut results = Vec::with_capacity(files.len().min(cap));
        for file in files {
            if results.len() >= cap {
                break;
            }
            // Pick the first root entry for this hash; locale/content
            // variants are irrelevant for a path-driven search.
            let entry = match self.root.get_all_entries(file.hash).first() {
                Some(e) => *e,
                None => continue,
            };
            results.push(SearchResult {
                hash: file.hash,
                filename: Some(file.full_path),
                file_data_id: file
                    .file_data_id
                    .or_else(|| self.root.file_data_id_for_hash(file.hash)),
                locale: entry.locale,
                content: entry.content,
                ckey: entry.ckey,
            });
        }
        results
    }

    /// High-level search dispatcher for a user-typed query string.
    ///
    /// - If the input contains glob metacharacters (`*`, `?`, `{`, `[`), it
    ///   is parsed as a [`PathQuery`](crate::query::PathQuery) and resolved
    ///   against the virtual tree — this is the "**/*.m2" / "sylvanas*.wmo"
    ///   case.
    /// - Otherwise it falls through to the existing case-insensitive
    ///   substring search over the root manifest, matching GUI behaviour
    ///   from before globs existed.
    pub fn search_by_text(&self, text: &str, limit: usize) -> Vec<SearchResult> {
        let looks_like_glob = text.bytes().any(|b| matches!(b, b'*' | b'?' | b'{' | b'['));
        if looks_like_glob {
            match crate::query::PathQuery::parse(text) {
                Ok(q) => self.search_by_path_query(&q, limit),
                Err(_) => Vec::new(),
            }
        } else {
            let mut q = SearchQuery::new().filename(text);
            if limit > 0 {
                q = q.limit(limit);
            }
            self.search(q)
        }
    }

    /// Iterate every known hash in the root manifest.
    ///
    /// This is the raw "all keys" view — equivalent to regedit showing every
    /// registry key regardless of which tree node is selected.
    pub fn all_hashes(&self) -> impl Iterator<Item = u64> + '_ {
        // Deduplicate hashes; root.all_entries() yields one row per RootEntry
        // variant (locale × content), so the same hash may appear many times.
        let mut seen = std::collections::HashSet::new();
        self.root
            .all_entries()
            .filter_map(move |(h, _)| seen.insert(h).then_some(h))
    }
}
