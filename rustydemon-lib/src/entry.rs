use std::collections::HashMap;
use std::path::PathBuf;

/// A node in the virtual CASC file tree (either a folder or a file).
#[derive(Debug)]
pub enum CascEntry {
    Folder(CascFolder),
    File(CascFile),
}

impl CascEntry {
    /// Name of this entry (the last component of its path).
    pub fn name(&self) -> &str {
        match self {
            Self::Folder(f) => &f.name,
            Self::File(f)   => &f.name,
        }
    }

    /// Returns `true` if this is a folder.
    pub fn is_folder(&self) -> bool { matches!(self, Self::Folder(_)) }

    /// Returns `true` if this is a file.
    pub fn is_file(&self) -> bool { matches!(self, Self::File(_)) }
}

// ── CascFolder ────────────────────────────────────────────────────────────────

/// A virtual directory in the CASC file tree.
#[derive(Debug)]
pub struct CascFolder {
    /// Directory name (not the full path).
    pub name: String,
    /// Child files, keyed by lower-case name.
    pub files: HashMap<String, CascFile>,
    /// Child sub-folders, keyed by lower-case name.
    pub folders: HashMap<String, CascFolder>,
}

impl CascFolder {
    /// Create an empty folder with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        CascFolder {
            name:    name.into(),
            files:   HashMap::new(),
            folders: HashMap::new(),
        }
    }

    /// Look up a child file (case-insensitive).
    pub fn file(&self, name: &str) -> Option<&CascFile> {
        self.files.get(&name.to_lowercase())
    }

    /// Look up a child folder (case-insensitive).
    pub fn folder(&self, name: &str) -> Option<&CascFolder> {
        self.folders.get(&name.to_lowercase())
    }

    /// Navigate to a sub-folder by path (e.g. `"Interface/Glues"`).
    ///
    /// Returns `None` if any component along the path is missing.
    pub fn navigate(&self, path: &str) -> Option<&CascFolder> {
        let mut current = self;
        for component in path.split(['/', '\\']) {
            if component.is_empty() { continue; }
            current = current.folder(component)?;
        }
        Some(current)
    }

    /// Recursively iterate all files under this folder.
    pub fn walk_files(&self) -> impl Iterator<Item = &CascFile> {
        // Use a collect-then-iterate approach to avoid lifetime headaches.
        let mut files: Vec<&CascFile> = Vec::new();
        walk_files_impl(self, &mut files);
        files.into_iter()
    }
}

fn walk_files_impl<'a>(folder: &'a CascFolder, out: &mut Vec<&'a CascFile>) {
    out.extend(folder.files.values());
    for sub in folder.folders.values() {
        walk_files_impl(sub, out);
    }
}

// ── CascFile ──────────────────────────────────────────────────────────────────

/// A file entry in the virtual CASC tree.
#[derive(Debug, Clone)]
pub struct CascFile {
    /// File name (last path component).
    pub name: String,
    /// Full virtual path within the CASC archive (e.g. `"Interface/Glues/file.blp"`).
    pub full_path: String,
    /// Jenkins96 hash of the full path, used for all CASC lookups.
    pub hash: u64,
    /// FileDataId, if known (WoW-specific).
    pub file_data_id: Option<u32>,
}

impl CascFile {
    /// Construct a new file entry.
    pub fn new(full_path: impl Into<String>, hash: u64, file_data_id: Option<u32>) -> Self {
        let full_path = full_path.into();
        let name = PathBuf::from(&full_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&full_path)
            .to_string();
        CascFile { name, full_path, hash, file_data_id }
    }
}

// ── Tree builder ──────────────────────────────────────────────────────────────

/// Build a [`CascFolder`] tree from a flat list of `(hash, full_path, fdid)` tuples.
///
/// This is used by the handler after loading the listfile to construct the
/// navigable tree exposed to the UI.
pub fn build_tree(
    entries: impl IntoIterator<Item = (u64, String, Option<u32>)>,
) -> CascFolder {
    let mut root = CascFolder::new("(root)");

    for (hash, path, fdid) in entries {
        insert_path(&mut root, hash, &path, fdid);
    }

    root
}

fn insert_path(root: &mut CascFolder, hash: u64, path: &str, fdid: Option<u32>) {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() { return; }

    let mut folder = root;
    for &component in &parts[..parts.len() - 1] {
        let key = component.to_lowercase();
        folder = folder
            .folders
            .entry(key)
            .or_insert_with(|| CascFolder::new(component));
    }

    let file_name = *parts.last().unwrap();
    let file = CascFile::new(path, hash, fdid);
    folder.files.insert(file_name.to_lowercase(), file);
}

// ── Listfile helpers ───────────────────────────────────────────────────────────

/// Load a community listfile (one path per line, optionally `fileDataId;path`).
///
/// Returns an iterator of `(path, Option<file_data_id>)` pairs, skipping
/// blank lines and comments.
pub fn parse_listfile(
    content: &str,
) -> impl Iterator<Item = (String, Option<u32>)> + '_ {
    content.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { return None; }

        // Try "fileDataId;path" format first.
        if let Some(semi) = line.find(';') {
            let id_str = line[..semi].trim();
            let path   = line[semi+1..].trim().to_string();
            if let Ok(id) = id_str.parse::<u32>() {
                return Some((path, Some(id)));
            }
        }

        Some((line.to_string(), None))
    })
}
