use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use egui::Context;
use rustydemon_lib::{CascConfig, CascHandler, LocaleFlags, PreparedLoad, SearchResult};

use crate::deep_search::{registry, ContentMatch, ContentSearcher};

/// Re-render a PCX selection using an external palette, replacing the
/// plugin-built texture in place. Updates `texture`, `texture_pixels`, and
/// rewrites the Export-As-PNG action to bake the same palette.
pub fn apply_pcx_palette_override(
    sel: &mut SelectedFile,
    data: &[u8],
    palette: &[u8],
    ctx: &Context,
) {
    let Ok((pixels, w, h)) = crate::preview::pcx::decode_pcx_with_palette(data, Some(palette))
    else {
        return;
    };
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
    if let Some(preview) = sel.preview.as_mut() {
        preview.texture =
            Some(ctx.load_texture("pcx_preview", color_image, egui::TextureOptions::default()));
        preview.texture_pixels = Some((pixels, w, h));
        // Swap the Export As PNG action for one that bakes in the palette.
        let pal_vec: Vec<u8> = palette.to_vec();
        let pal_arc = std::sync::Arc::new(pal_vec);
        preview.extra_exports.clear();
        let pal_clone = pal_arc.clone();
        preview.extra_exports.push(crate::preview::ExportAction {
            label: "Export As PNG",
            default_extension: "png",
            filter_name: "PNG image",
            build: std::sync::Arc::new(move |data| {
                let (pixels, w, h) =
                    crate::preview::pcx::decode_pcx_with_palette(data, Some(pal_clone.as_slice()))
                        .map_err(|e| format!("pcx decode: {e}"))?;
                crate::preview::encode_png(&pixels, w, h)
            }),
        });
    }
}

// ── Selected file ──────────────────────────────────────────────────────────────

/// State for whichever file is shown in the right preview panel.
pub struct SelectedFile {
    pub result: SearchResult,
    /// Decoded bytes (lazily loaded on selection).
    pub data: Option<Vec<u8>>,
    /// Error string if loading failed.
    pub load_error: Option<String>,
    /// Output produced by the first matching [`PreviewPlugin`].  When
    /// `None`, the panel falls back to a hex dump.
    pub preview: Option<crate::preview::PreviewOutput>,
    /// Deep-search hits inside this container file.
    pub content_matches: Vec<ContentMatch>,
}

impl SelectedFile {
    pub fn new(result: SearchResult) -> Self {
        Self {
            result,
            data: None,
            load_error: None,
            preview: None,
            content_matches: vec![],
        }
    }
}

// ── Background task results ────────────────────────────────────────────────────

#[allow(clippy::large_enum_variant)]
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
    /// A listfile was parsed and the tree was built in the background.
    ListfileLoaded {
        filenames: std::collections::HashMap<u64, String>,
        tree: rustydemon_lib::CascFolder,
        path: std::path::PathBuf,
    },
    /// Listfile loading failed.
    ListfileError(String),
    /// A batch export finished.
    ExportComplete {
        ok: usize,
        fail: usize,
        label: String,
    },
}

/// One file prepared for background export.
struct ExportItem {
    filename: String,
    prepared: PreparedLoad,
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

    // ── PCX palette override ──────────────────────────────────────────────────
    /// 768-byte RGB palette loaded by the user to re-render palettized assets
    /// (SC1 PCX files that reference an external `.pal`/`.wpe` file).
    pub pcx_palette: Option<Vec<u8>>,
    /// Display name of the currently loaded palette (filename only).
    pub pcx_palette_name: Option<String>,

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
            pcx_palette: None,
            pcx_palette_name: None,
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
                let fnames = handler.filename_count();
                let root_ty = handler.root_type_name();
                let idx = handler.local_index_count();
                let enc = handler.encoding_count();
                self.status = format!(
                    "Opened: {} ({})  |  root={root_ty}  entries={count}  names={fnames}  idx={idx}  enc={enc}",
                    path.display(),
                    handler.config.product,
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
            Ok(BgResult::ListfileLoaded {
                filenames,
                tree,
                path,
            }) => {
                if let Some(handler) = self.handler.as_mut() {
                    handler.apply_listfile(filenames, tree);
                    self.status = format!("Listfile loaded: {}", path.display());
                }
                self.bg_rx = None;
                self.loading = false;
            }
            Ok(BgResult::ListfileError(e)) => {
                self.status = e;
                self.bg_rx = None;
                self.loading = false;
            }
            Ok(BgResult::ExportComplete { ok, fail, label }) => {
                self.status = format!("Exported {ok} files from {label} ({fail} failed)");
                self.bg_rx = None;
                self.loading = false;
            }
            Ok(BgResult::FileLoaded { result, data }) => {
                let mut sel = SelectedFile::new(result.clone());
                match data {
                    Ok(data) => {
                        // Dispatch to the first matching preview plugin.
                        // Plugins are registered in `crate::preview::registry()`.
                        sel.preview = crate::preview::run(result.filename.as_deref(), &data, ctx);

                        // If a PCX palette override is active, re-render the
                        // texture with it so SC1 assets that rely on external
                        // .pal/.wpe files display correctly.
                        let is_pcx = result
                            .filename
                            .as_deref()
                            .map(|n| n.to_ascii_lowercase().ends_with(".pcx"))
                            .unwrap_or(false);
                        if is_pcx {
                            if let Some(pal) = self.pcx_palette.as_deref() {
                                apply_pcx_palette_override(&mut sel, &data, pal, ctx);
                            }
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
        if self.handler.is_none() {
            self.status = "Open a game directory first.".into();
            return;
        }
        self.cancel_bg();
        self.status = format!("Loading listfile {}…", path.display());
        self.loading = true;

        // Snapshot the fdid→hash map so the bg thread can resolve hashes
        // without borrowing the handler.
        let fdid_hashes = self.handler.as_ref().unwrap().fdid_hash_snapshot();

        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        self.bg_rx = Some(rx);

        std::thread::spawn(move || {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let (filenames, tree) =
                        rustydemon_lib::prepare_listfile(&content, &fdid_hashes);
                    let _ = tx.send(BgResult::ListfileLoaded {
                        filenames,
                        tree,
                        path,
                    });
                }
                Err(e) => {
                    let _ = tx.send(BgResult::ListfileError(format!(
                        "Failed to read listfile: {e}"
                    )));
                }
            }
        });
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

    /// Select a search result and load its raw bytes in the background.
    pub fn select_result(&mut self, result: SearchResult, _ctx: &Context) {
        if self.handler.is_none() {
            return;
        }

        // Cancel any previous file load.
        self.cancel_bg();

        let handler = self.handler.as_ref().unwrap();
        let ckey = result.ckey;
        let (tx, rx) = mpsc::channel();
        self.bg_rx = Some(rx);
        self.loading = true;

        if handler.is_static_container() {
            // Static containers: load synchronously (fast path, no .idx).
            let data = handler.open_by_ckey(&ckey).map_err(|e| format!("{e}"));
            let _ = tx.send(BgResult::FileLoaded { result, data });
            return;
        }

        // Fast hash lookups stay on the UI thread; the heavy I/O + BLTE
        // decompression runs on a background thread.
        let prepared = match handler.prepare_load(&ckey) {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(BgResult::FileLoaded {
                    result,
                    data: Err(format!("{e}")),
                });
                return;
            }
        };

        let cancel = self.cancel.clone();

        std::thread::spawn(move || {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let data = prepared.execute().map_err(|e| format!("{e}"));
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(BgResult::FileLoaded { result, data });
            }
        });
    }

    /// Export all multi-selected files to a directory (background thread).
    pub fn export_selected(&mut self) {
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

        // Prepare all work items on the UI thread (fast hash lookups).
        let items =
            self.prepare_export_items(handler, self.multi_selected.iter().copied().collect());
        let total = items.len();

        self.cancel_bg();
        self.status = format!("Exporting {total} files…");
        self.loading = true;

        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        self.bg_rx = Some(rx);

        std::thread::spawn(move || {
            let (ok, fail) = run_export(&items, &dest, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(BgResult::ExportComplete {
                    ok,
                    fail,
                    label: format!("{total} selected files"),
                });
            }
        });
    }

    /// Export all files in the currently browsed folder (background thread).
    pub fn export_folder(&mut self) {
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
        let total = hashes.len();

        let items = self.prepare_export_items(handler, hashes);

        self.cancel_bg();
        self.status = format!("Exporting {total} files from {folder_path}…");
        self.loading = true;

        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        self.bg_rx = Some(rx);

        std::thread::spawn(move || {
            let (ok, fail) = run_export(&items, &dest, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(BgResult::ExportComplete {
                    ok,
                    fail,
                    label: folder_path,
                });
            }
        });
    }

    /// Prepare export work items: resolve hashes to filenames + PreparedLoads.
    fn prepare_export_items(&self, handler: &CascHandler, hashes: Vec<u64>) -> Vec<ExportItem> {
        let mut items = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let filename = handler
                .filename(hash)
                .unwrap_or("unknown")
                .replace('\\', "/");

            let entries = handler.search_by_hash(hash);
            let Some(entry) = entries.first() else {
                continue;
            };

            if let Ok(prepared) = handler.prepare_load(&entry.ckey) {
                items.push(ExportItem { filename, prepared });
            }
        }
        items
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

/// Execute a batch of prepared exports on a background thread.
fn run_export(items: &[ExportItem], dest: &std::path::Path, cancel: &AtomicBool) -> (usize, usize) {
    let mut ok = 0usize;
    let mut fail = 0usize;

    for item in items {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let out_path = dest.join(&item.filename);
        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match read_and_write_export(&item.prepared, &out_path) {
            Ok(()) => ok += 1,
            Err(_) => fail += 1,
        }
    }

    (ok, fail)
}

/// Read a file from CASC archives and write it to disk.
fn read_and_write_export(
    prepared: &PreparedLoad,
    out_path: &std::path::Path,
) -> Result<(), String> {
    let data = prepared.execute_ref().map_err(|e| format!("{e}"))?;
    std::fs::write(out_path, &data).map_err(|e| format!("{e}"))
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
        ("sc1", "StarCraft: Remastered"),
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
                    let launcher = if is_steam && path_str.contains("mmcblk0p1")
                        || path_str.contains("sdcard")
                    {
                        "Steam Deck SD"
                    } else if is_steam {
                        "Steam"
                    } else if path_str.contains("Public Test") || product.contains("test") {
                        "Battle.net PTR"
                    } else {
                        "Battle.net"
                    };

                    found.push((format!("{game_name} [{launcher}]"), path.clone()));
                }
            }
        }
    }

    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.1 == b.1);
    found
}
