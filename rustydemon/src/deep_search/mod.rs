pub mod pow;

/// A match found inside a container file by a [`ContentSearcher`].
#[derive(Debug, Clone)]
pub struct ContentMatch {
    /// Path inside the container (e.g. `"character_02/diffuse.tex"`).
    pub inner_path: String,
    /// Byte offset of the entry inside the container data, if known.
    #[allow(dead_code)]
    pub offset: Option<u64>,
    /// Human-readable kind label (e.g. `"texture"`, `"formula"`, `"SF def"`).
    pub kind: String,
}

/// Plug-in interface for searching inside container file formats.
///
/// Implement this trait for each format that supports deep search
/// (`.pow`, `.gam`, etc.).  Register instances in [`registry`].
pub trait ContentSearcher: Send + Sync {
    /// Return `true` if this searcher can inspect files with the given name.
    fn can_search(&self, filename: &str) -> bool;

    /// Search `data` for entries matching `query` (case-insensitive substring).
    ///
    /// An empty `query` returns **all** entries in the container.
    fn search(&self, data: &[u8], query: &str) -> Vec<ContentMatch>;

    /// Human-readable format name shown in the UI (e.g. `".pow (D4 skill)"`).
    #[allow(dead_code)]
    fn format_name(&self) -> &str;
}

/// All registered deep-search plug-ins, in priority order.
///
/// Add a new searcher here when porting a new container format.
pub fn registry() -> Vec<Box<dyn ContentSearcher>> {
    vec![
        Box::new(pow::PowSearcher),
    ]
}
