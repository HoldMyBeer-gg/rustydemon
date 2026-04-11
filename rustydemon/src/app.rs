use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::Context;
use rustydemon_lib::{CascConfig, CascHandler, LocaleFlags, SearchResult};

use crate::deep_search::{registry, ContentMatch, ContentSearcher};

// ── Selected file ──────────────────────────────────────────────────────────────

/// State for whichever file is shown in the right preview panel.
pub struct SelectedFile {
    pub result: SearchResult,
    /// Decoded bytes (lazily loaded on selection).
    pub data: Option<Vec<u8>>,
    /// Error string if loading failed.
    pub load_error: Option<String>,
    /// For BLP files: decoded RGBA texture ready for the GPU.
    pub texture: Option<egui::TextureHandle>,
    /// Deep-search hits inside this container file.
    pub content_matches: Vec<ContentMatch>,
}

impl SelectedFile {
    pub fn new(result: SearchResult) -> Self {
        Self {
            result,
            data: None,
            load_error: None,
            texture: None,
            content_matches: vec![],
        }
    }
}

// ── Background task results ────────────────────────────────────────────────────

enum BgResult {
    /// A CASC archive was opened successfully.
    Opened {
        handler: CascHandler,
        path: std::path::PathBuf,
    },
    /// Opening failed.
    OpenError(String),
    /// A file was loaded (by hash).
    FileLoaded {
        result: SearchResult,
        data: Result<Vec<u8>, String>,
    },
}

// ── App state ──────────────────────────────────────────────────────────────────

pub struct CascExplorerApp {
    // ── CASC backend ──────────────────────────────────────────────────────────
    pub handler: Option<CascHandler>,
    /// Internal product UID (e.g. "fenris", "wow").  Editable in Tools menu.
    pub product: String,
    /// Products detected from the last directory's .build.info.
    pub detected_products: Vec<String>,

    // ── Tree state ────────────────────────────────────────────────────────────
    /// Folder paths that should be expanded (written by View menu).
    pub expanded: HashSet<String>,
    /// Set to `true` when `expanded` changes so the tree re-applies the states.
    pub expansion_dirty: bool,

    // ── Search / browsing ────────────────────────────────────────────────────
    pub search_text: String,
    pub search_results: Vec<SearchResult>,
    pub deep_search_enabled: bool,
    /// True while a search is in progress (placeholder for async later).
    #[allow(dead_code)]
    pub searching: bool,
    /// Currently browsed folder path (set when clicking a folder in the tree).
    pub browsed_folder: Option<String>,

    // ── Selection ─────────────────────────────────────────────────────────────
    pub selected: Option<SelectedFile>,
    /// Multi-selected file hashes for batch export.
    pub multi_selected: HashSet<u64>,

    // ── Deep-search plug-ins ──────────────────────────────────────────────────
    pub searchers: Vec<Box<dyn ContentSearcher>>,

    // ── Status bar ────────────────────────────────────────────────────────────
    pub status: String,

    // ── Background tasks ──────────────────────────────────────────────────────
    bg_rx: Option<mpsc::Receiver<BgResult>>,
    cancel: Arc<AtomicBool>,
    /// True while a background task is running.
    pub loading: bool,
}

impl CascExplorerApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            handler: None,
            product: String::new(),
            detected_products: vec![],
            expanded: HashSet::new(),
            expansion_dirty: false,
            search_text: String::new(),
            search_results: vec![],
            deep_search_enabled: false,
            searching: false,
            browsed_folder: None,
            selected: None,
            multi_selected: HashSet::new(),
            searchers: registry(),
            status: "No archive open. Use File → Open Game Directory.".into(),
            bg_rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            loading: false,
        }
    }

    /// Cancel any in-flight background task.
    fn cancel_bg(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.bg_rx = None;
        self.loading = false;
    }

    /// Poll for background task completion. Called every frame.
    pub fn poll_background(&mut self, ctx: &Context) {
        let rx = match &self.bg_rx {
            Some(rx) => rx,
            None => return,
        };

        match rx.try_recv() {
            Ok(BgResult::Opened { mut handler, path }) => {
                handler.set_locale(LocaleFlags::EN_US);
                handler.load_builtin_paths();
                let count = handler.root_count();
                self.status = format!(
                    "Opened: {} (product: {})  |  {} root entries",
                    path.display(),
                    handler.config.product,
                    count
                );
                self.handler = Some(handler);
                self.search_results.clear();
                self.selected = None;
                self.expanded.clear();
                self.bg_rx = None;
                self.loading = false;
            }
            Ok(BgResult::OpenError(e)) => {
                self.status = e;
                self.bg_rx = None;
                self.loading = false;
            }
            Ok(BgResult::FileLoaded { result, data }) => {
                let mut sel = SelectedFile::new(result.clone());
                match data {
                    Ok(data) => {
                        if result
                            .filename
                            .as_deref()
                            .map(|n| n.to_lowercase().ends_with(".blp"))
                            .unwrap_or(false)
                        {
                            sel.texture = decode_blp_texture(&data, ctx);
                        }

                        if self.deep_search_enabled {
                            for searcher in &self.searchers {
                                let name = result.filename.as_deref().unwrap_or("");
                                if searcher.can_search(name) {
                                    let hits = searcher.search(&data, "");
                                    sel.content_matches.extend(hits);
                                }
                            }
                        }

                        sel.data = Some(data);
                    }
                    Err(e) => {
                        sel.load_error = Some(e);
                    }
                }
                self.selected = Some(sel);
                self.bg_rx = None;
                self.loading = false;
            }
            Err(mpsc::TryRecvError::Empty) => {
                // Still working — poll again in 100ms (not every frame).
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                // Thread dropped without sending — treat as cancelled.
                self.bg_rx = None;
                self.loading = false;
            }
        }
    }

    // ── Handler actions ────────────────────────────────────────────────────────

    pub fn open_game_dir(&mut self, path: std::path::PathBuf) {
        self.cancel_bg();

        // Detect products (fast, stays on UI thread).
        let products = CascConfig::detect_products(&path);
        self.detected_products = products.clone();

        let product = if products.iter().any(|p| p == &self.product) {
            self.product.clone()
        } else if let Some(first) = products.into_iter().next() {
            self.product = first.clone();
            first
        } else {
            if self.product.is_empty() {
                self.product = "wow".into();
            }
            self.product.clone()
        };

        self.status = format!("Opening {}…", path.display());
        self.loading = true;

        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        self.bg_rx = Some(rx);

        std::thread::spawn(move || {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            match CascHandler::open_local(&path, &product) {
                Ok(handler) => {
                    if !cancel.load(Ordering::Relaxed) {
                        let _ = tx.send(BgResult::Opened { handler, path });
                    }
                }
                Err(e) => {
                    let _ = tx.send(BgResult::OpenError(format!(
                        "Error opening {}: {e}",
                        path.display()
                    )));
                }
            }
        });
    }

    pub fn load_listfile(&mut self, path: std::path::PathBuf) {
        let Some(handler) = self.handler.as_mut() else {
            self.status = "Open a game directory first.".into();
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                handler.load_listfile(&content);
                self.status = format!("Listfile loaded: {}", path.display());
            }
            Err(e) => {
                self.status = format!("Failed to read listfile: {e}");
            }
        }
    }

    /// Run a search and populate `search_results`.
    pub fn run_search(&mut self) {
        let Some(handler) = self.handler.as_ref() else {
            return;
        };
        self.browsed_folder = None;
        let query = rustydemon_lib::SearchQuery::new()
            .filename(&self.search_text)
            .limit(500);
        self.search_results = handler.search(query);
        self.status = format!(
            "{} results for {:?}",
            self.search_results.len(),
            self.search_text
        );
    }

    /// Run the full global (deep) search — every entry, optionally searching
    /// inside container files.
    pub fn run_deep_search(&mut self) {
        let Some(handler) = self.handler.as_ref() else {
            return;
        };
        let query = rustydemon_lib::SearchQuery::new().filename(&self.search_text);
        self.search_results = handler.search(query);
        self.status = format!(
            "Deep search: {} top-level results for {:?} (deep-search into containers: {})",
            self.search_results.len(),
            self.search_text,
            if self.deep_search_enabled {
                "on"
            } else {
                "off"
            }
        );
    }

    /// Select a search result and load its raw bytes.
    pub fn select_result(&mut self, result: SearchResult, _ctx: &Context) {
        if self.handler.is_none() {
            return;
        }

        // Cancel any previous file load.
        self.cancel_bg();

        let ckey = result.ckey;
        let data_result = self.handler.as_ref().unwrap().open_by_ckey(&ckey);

        let (tx, rx) = mpsc::channel();
        self.bg_rx = Some(rx);
        self.loading = true;

        match data_result {
            Ok(data) => {
                let _ = tx.send(BgResult::FileLoaded {
                    result,
                    data: Ok(data),
                });
            }
            Err(e) => {
                let _ = tx.send(BgResult::FileLoaded {
                    result,
                    data: Err(format!("{e}")),
                });
            }
        }
    }

    /// Export all multi-selected files to a directory.
    pub fn export_selected(&self) {
        let handler = match self.handler.as_ref() {
            Some(h) => h,
            None => return,
        };

        if self.multi_selected.is_empty() {
            return;
        }

        let Some(dest) = rfd::FileDialog::new()
            .set_title("Export Selected Files")
            .pick_folder()
        else {
            return;
        };

        let mut ok = 0usize;
        let mut fail = 0usize;

        for &hash in &self.multi_selected {
            let filename = handler
                .filename(hash)
                .unwrap_or("unknown")
                .replace('\\', "/");

            // Recreate subfolder structure.
            let out_path = dest.join(&filename);
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            // Resolve hash → ckey → data.
            let entries = handler.search_by_hash(hash);
            let Some(entry) = entries.first() else {
                fail += 1;
                continue;
            };

            match handler.open_by_ckey(&entry.ckey) {
                Ok(data) => {
                    if std::fs::write(&out_path, &data).is_ok() {
                        ok += 1;
                    } else {
                        fail += 1;
                    }
                }
                Err(_) => {
                    fail += 1;
                }
            }
        }

        // Show result in a message box (non-blocking).
        rfd::MessageDialog::new()
            .set_title("Export Complete")
            .set_description(&format!("Exported {ok} files ({fail} failed)"))
            .set_level(rfd::MessageLevel::Info)
            .show();
    }

    /// Export all files in the currently browsed folder to a directory.
    pub fn export_folder(&self) {
        let handler = match self.handler.as_ref() {
            Some(h) => h,
            None => return,
        };

        let folder_path = match &self.browsed_folder {
            Some(p) => p.clone(),
            None => return,
        };

        let root_folder = match handler.root_folder.as_ref() {
            Some(f) => f,
            None => return,
        };

        let folder = if folder_path.is_empty() {
            root_folder
        } else {
            match root_folder.navigate(&folder_path) {
                Some(f) => f,
                None => return,
            }
        };

        let Some(dest) = rfd::FileDialog::new()
            .set_title("Export Folder")
            .pick_folder()
        else {
            return;
        };

        let mut hashes = Vec::new();
        collect_file_hashes(folder, &mut hashes);

        let mut ok = 0usize;
        let mut fail = 0usize;

        for hash in &hashes {
            let filename = handler
                .filename(*hash)
                .unwrap_or("unknown")
                .replace('\\', "/");

            let out_path = dest.join(&filename);
            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let entries = handler.search_by_hash(*hash);
            let Some(entry) = entries.first() else {
                fail += 1;
                continue;
            };

            match handler.open_by_ckey(&entry.ckey) {
                Ok(data) => {
                    if std::fs::write(&out_path, &data).is_ok() {
                        ok += 1;
                    } else {
                        fail += 1;
                    }
                }
                Err(_) => {
                    fail += 1;
                }
            }
        }

        rfd::MessageDialog::new()
            .set_title("Export Complete")
            .set_description(&format!(
                "Exported {ok} files from {folder_path} ({fail} failed)"
            ))
            .set_level(rfd::MessageLevel::Info)
            .show();
    }

    /// Export the selected file's texture as a PNG via a native save dialog.
    pub fn export_as_png(&self) {
        let Some(sel) = &self.selected else {
            return;
        };
        let Some(data) = &sel.data else {
            return;
        };

        // Only export as PNG if we can actually decode the texture.
        if !is_blp_data(data) {
            rfd::MessageDialog::new()
                .set_title("Cannot Export")
                .set_description("This texture format is not supported for PNG export yet. Use 'Export Raw' instead.")
                .set_level(rfd::MessageLevel::Warning)
                .show();
            return;
        }

        let stem = sel
            .result
            .filename
            .as_deref()
            .and_then(|n| std::path::Path::new(n).file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("export");

        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&format!("{stem}.png"))
            .add_filter("PNG image", &["png"])
            .save_file()
        {
            match rustydemon_blp2::BlpFile::from_bytes(data.clone()) {
                Ok(blp) => {
                    if let Ok((pixels, w, h)) = blp.get_pixels(0) {
                        let _ = save_rgba_as_png(&pixels, w, h, &path);
                    }
                }
                Err(_) => {}
            }
        }
    }
}

// ── eframe::App ────────────────────────────────────────────────────────────────

impl eframe::App for CascExplorerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background(ctx);
        crate::ui::draw(ctx, self);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Check if data starts with BLP magic bytes ("BLP2" or "BLP1").
fn is_blp_data(data: &[u8]) -> bool {
    data.len() >= 4 && (data[..4] == *b"BLP2" || data[..4] == *b"BLP1")
}

fn decode_blp_texture(data: &[u8], ctx: &Context) -> Option<egui::TextureHandle> {
    if !is_blp_data(data) {
        return None;
    }
    let blp = rustydemon_blp2::BlpFile::from_bytes(data.to_vec()).ok()?;
    let (pixels, w, h) = blp.get_pixels(0).ok()?;
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
    Some(ctx.load_texture("blp_preview", color_image, egui::TextureOptions::default()))
}

fn collect_file_hashes(folder: &rustydemon_lib::CascFolder, out: &mut Vec<u64>) {
    for file in folder.files.values() {
        out.push(file.hash);
    }
    for sub in folder.folders.values() {
        collect_file_hashes(sub, out);
    }
}

/// Scan common install locations for Blizzard/Steam games with CASC archives.
/// Returns a list of (display_name, path) pairs.
pub fn detect_game_installs() -> Vec<(String, std::path::PathBuf)> {
    use rustydemon_lib::CascConfig;

    let mut found = Vec::new();

    // Known Blizzard game UIDs → display names.
    let uid_names: &[(&str, &str)] = &[
        ("fenris", "Diablo IV"),
        ("wow", "World of Warcraft"),
        ("d3", "Diablo III"),
        ("hero", "Heroes of the Storm"),
        ("pro", "Overwatch"),
        ("osi", "Diablo II: Resurrected"),
        ("w3", "Warcraft III: Reforged"),
        ("s1", "StarCraft"),
        ("s2", "StarCraft II"),
        ("hs", "Hearthstone"),
        ("viper", "Call of Duty"),
        ("odin", "Call of Duty"),
        ("lazr", "Crash Bandicoot 4"),
        ("fore", "Spyro"),
        ("zeus", "Call of Duty: MW"),
        ("gryphon", "Call of Duty: BO6"),
    ];

    // Common install directories to scan (Windows + Linux/Steam Deck).
    let mut candidates: Vec<std::path::PathBuf> = vec![
        // Windows — Battle.net / Steam
        "C:/Program Files (x86)".into(),
        "C:/Program Files".into(),
        "D:/Games".into(),
        "E:/Games".into(),
        "C:/Program Files (x86)/Steam/steamapps/common".into(),
        "D:/SteamLibrary/steamapps/common".into(),
        "E:/SteamLibrary/steamapps/common".into(),
    ];

    // Linux / Steam Deck
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(format!("{home}/.local/share/Steam/steamapps/common").into());
        candidates.push(format!("{home}/.steam/steam/steamapps/common").into());
    }
    // Steam Deck SD card
    candidates.push("/run/media/mmcblk0p1/steamapps/common".into());
    candidates.push("/run/media/sdcard/steamapps/common".into());

    for base in &candidates {
        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Check if this directory has a .build.info file.
            if !path.join(".build.info").exists() {
                continue;
            }
            let products = CascConfig::detect_products(&path);
            let dir_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if products.is_empty() {
                found.push((format!("{dir_name}  —  {}", path.display()), path));
            } else {
                for product in &products {
                    let game_name = uid_names
                        .iter()
                        .find(|(uid, _)| product.starts_with(uid))
                        .map(|(_, name)| *name)
                        .unwrap_or(&dir_name);

                    // Detect launcher from path.
                    let path_str = path.to_string_lossy();
                    let is_steam = path_str.contains("Steam")
                        || path_str.contains("steam")
                        || path_str.contains("steamapps");
                    let launcher = if is_steam && path_str.contains("mmcblk0p1") || path_str.contains("sdcard") {
                        "Steam Deck SD"
                    } else if is_steam {
                        "Steam"
                    } else if path_str.contains("Public Test") || product.contains("test") {
                        "Battle.net PTR"
                    } else {
                        "Battle.net"
                    };

                    found.push((
                        format!("{game_name} [{launcher}]"),
                        path.clone(),
                    ));
                }
            }
        }
    }

    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.1 == b.1);
    found
}

fn save_rgba_as_png(
    pixels: &[u8],
    w: u32,
    h: u32,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use image::{ImageBuffer, RgbaImage};
    let img: RgbaImage =
        ImageBuffer::from_raw(w, h, pixels.to_vec()).ok_or("invalid pixel buffer dimensions")?;
    img.save(path)?;
    Ok(())
}
