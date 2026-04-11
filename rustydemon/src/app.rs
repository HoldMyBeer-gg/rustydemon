use std::collections::HashSet;

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

    // ── Search ────────────────────────────────────────────────────────────────
    pub search_text: String,
    pub search_results: Vec<SearchResult>,
    pub deep_search_enabled: bool,
    /// True while a search is in progress (placeholder for async later).
    #[allow(dead_code)]
    pub searching: bool,

    // ── Selection ─────────────────────────────────────────────────────────────
    pub selected: Option<SelectedFile>,

    // ── Deep-search plug-ins ──────────────────────────────────────────────────
    pub searchers: Vec<Box<dyn ContentSearcher>>,

    // ── Status bar ────────────────────────────────────────────────────────────
    pub status: String,
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
            selected: None,
            searchers: registry(),
            status: "No archive open. Use File → Open Game Directory.".into(),
        }
    }

    // ── Handler actions ────────────────────────────────────────────────────────

    pub fn open_game_dir(&mut self, path: std::path::PathBuf) {
        // Detect what products live in this directory.
        let products = CascConfig::detect_products(&path);
        self.detected_products = products.clone();

        // Pick a product: prefer the user's existing selection if it matches,
        // otherwise use the first detected one, otherwise fall back to whatever
        // is set (lets the user override via Tools > Product).
        let product = if products.iter().any(|p| p == &self.product) {
            self.product.clone()
        } else if let Some(first) = products.into_iter().next() {
            self.product = first.clone();
            first
        } else {
            // No .build.info Product column — use whatever the user set, or "wow".
            if self.product.is_empty() { self.product = "wow".into(); }
            self.product.clone()
        };

        match CascHandler::open_local(&path, &product) {
            Ok(mut h) => {
                h.set_locale(LocaleFlags::EN_US);
                let count = h.root_count();
                self.status = format!(
                    "Opened: {} (product: {})  |  {} root entries",
                    path.display(), h.config.product, count
                );
                self.handler = Some(h);
                self.search_results.clear();
                self.selected = None;
                self.expanded.clear();
            }
            Err(e) => {
                self.status = format!("Error opening {}: {e}", path.display());
            }
        }
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
        let Some(handler) = self.handler.as_ref() else { return; };
        let query = rustydemon_lib::SearchQuery::new()
            .filename(&self.search_text)
            .limit(500);
        self.search_results = handler.search(query);
        self.status = format!("{} results for {:?}", self.search_results.len(), self.search_text);
    }

    /// Run the full global (deep) search — every entry, optionally searching
    /// inside container files.
    pub fn run_deep_search(&mut self) {
        let Some(handler) = self.handler.as_ref() else { return; };
        let query = rustydemon_lib::SearchQuery::new()
            .filename(&self.search_text);
        self.search_results = handler.search(query);
        self.status = format!(
            "Deep search: {} top-level results for {:?} (deep-search into containers: {})",
            self.search_results.len(),
            self.search_text,
            if self.deep_search_enabled { "on" } else { "off" }
        );
    }

    /// Select a search result and load its raw bytes.
    pub fn select_result(&mut self, result: SearchResult, ctx: &Context) {
        let Some(handler) = self.handler.as_ref() else { return; };

        let mut sel = SelectedFile::new(result.clone());

        // Load the raw file bytes.
        let data_result = handler.open_by_ckey(&result.ckey);
        match data_result {
            Ok(data) => {
                // Check if it looks like a BLP texture.
                if result.filename.as_deref()
                    .map(|n| n.to_lowercase().ends_with(".blp"))
                    .unwrap_or(false)
                {
                    sel.texture = decode_blp_texture(&data, ctx);
                }

                // Run deep-search plug-ins if this is a container format.
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
                sel.load_error = Some(format!("{e}"));
            }
        }

        self.selected = Some(sel);
    }

    /// Export the selected file's texture as a PNG via a native save dialog.
    pub fn export_as_png(&self) {
        let Some(sel) = &self.selected else { return; };
        let Some(data) = &sel.data else { return; };

        // Determine a suggested file name.
        let stem = sel.result.filename.as_deref()
            .and_then(|n| std::path::Path::new(n).file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("export");

        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&format!("{stem}.png"))
            .add_filter("PNG image", &["png"])
            .save_file()
        {
            let filename = sel.result.filename.as_deref().unwrap_or("");
            if filename.to_lowercase().ends_with(".blp") {
                match rustydemon_blp2::BlpFile::from_bytes(data.clone()) {
                    Ok(blp) => {
                        if let Ok((pixels, w, h)) = blp.get_pixels(0) {
                            let _ = save_rgba_as_png(&pixels, w, h, &path);
                        }
                    }
                    Err(_) => {}
                }
            } else {
                let _ = std::fs::write(&path, data);
            }
        }
    }
}

// ── eframe::App ────────────────────────────────────────────────────────────────

impl eframe::App for CascExplorerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::ui::draw(ctx, self);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn decode_blp_texture(data: &[u8], ctx: &Context) -> Option<egui::TextureHandle> {
    let blp = rustydemon_blp2::BlpFile::from_bytes(data.to_vec()).ok()?;
    let (pixels, w, h) = blp.get_pixels(0).ok()?;
    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &pixels,
    );
    Some(ctx.load_texture("blp_preview", color_image, egui::TextureOptions::default()))
}

fn save_rgba_as_png(
    pixels: &[u8],
    w: u32,
    h: u32,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use image::{ImageBuffer, RgbaImage};
    let img: RgbaImage = ImageBuffer::from_raw(w, h, pixels.to_vec())
        .ok_or("invalid pixel buffer dimensions")?;
    img.save(path)?;
    Ok(())
}
