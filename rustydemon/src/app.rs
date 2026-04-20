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
            build: std::sync::Arc::new(move |data, _path| {
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
    /// Index into [`crate::preview::registry`] of a plugin the user
    /// forced via the "Viewer:" dropdown.  `None` means auto-dispatch
    /// (first matching `can_preview` wins, same as before).  Cleared
    /// when the selection changes.
    pub preview_override: Option<usize>,
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
            preview_override: None,
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
    /// A text search completed.
    SearchComplete {
        results: Vec<SearchResult>,
        query: String,
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
    pub handler: Option<Arc<CascHandler>>,
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

    // ── 3D viewport spike ─────────────────────────────────────────────────────
    pub viewport3d_open: bool,

    // ── Deferred actions ──────────────────────────────────────────────────────
    /// A viewer-override change requested on the previous frame.  Applied
    /// at the top of the next `update` so the old `PreviewOutput`'s GPU
    /// texture handle isn't dropped mid-frame while egui still has paint
    /// commands referencing it — dropping a live `egui::TextureHandle`
    /// inside the frame that used it crashes wgpu at queue-submit time.
    /// `Some(None)` means "switch back to Auto"; `Some(Some(idx))` means
    /// "force plugin at registry index idx".
    pub pending_preview_override: Option<Option<usize>>,

    // ── Audio playback ────────────────────────────────────────────────────────
    /// Lazily-initialised audio player.  `None` means "not yet tried"
    /// until the first selection with an audio file, at which point we
    /// attempt `AudioPlayer::try_new()`.  If that fails (no audio
    /// device), this stays `Some(None)` as a sentinel so we don't
    /// keep retrying.
    pub audio_player: Option<Option<crate::audio::AudioPlayer>>,
    /// Audio action collected from the preview panel's Play / Pause /
    /// Stop buttons, applied during the next [`poll_background`] tick.
    pub pending_audio_action: Option<AudioAction>,
}

#[derive(Debug, Clone)]
pub enum AudioAction {
    /// Decode the given bytes with the given display label and start
    /// playback, replacing any current track.
    Play(Vec<u8>, String),
    TogglePause,
    Stop,
}

impl CascExplorerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Install GPU resources for the 3D viewport spike, if wgpu is active.
        if let Some(render_state) = cc.wgpu_render_state.as_ref() {
            crate::viewport3d::init(render_state);
        }
        // Install the RustyDemon design-system theme (frost/ember palette,
        // ember selection, forge radii).  Persists across frames, so one
        // call at startup is enough.
        crate::ui::theme::apply(&cc.egui_ctx);
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
            viewport3d_open: false,
            pending_preview_override: None,
            audio_player: None,
            pending_audio_action: None,
        }
    }

    /// Access the audio player, initialising it on first use.  Returns
    /// `None` if the machine has no working audio device.
    pub fn audio_player_mut(&mut self) -> Option<&mut crate::audio::AudioPlayer> {
        let slot = self
            .audio_player
            .get_or_insert_with(crate::audio::AudioPlayer::try_new);
        slot.as_mut()
    }

    /// Apply any queued audio action.  Called from `update()` so the
    /// preview panel can collect button clicks into an intent and let
    /// the app touch `audio_player` (which requires `&mut self`)
    /// outside of the borrow-conflicted `sel` render loop.
    pub fn apply_pending_audio(&mut self) {
        let Some(action) = self.pending_audio_action.take() else {
            return;
        };
        let Some(player) = self.audio_player_mut() else {
            self.status = "Audio playback unavailable (no output device).".into();
            return;
        };
        match action {
            AudioAction::Play(bytes, label) => {
                if let Err(e) = player.play(bytes, label.clone()) {
                    self.status = format!("Audio: {e}");
                } else {
                    self.status = format!("Playing {label}");
                }
            }
            AudioAction::TogglePause => player.toggle_pause(),
            AudioAction::Stop => player.stop(),
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
                self.handler = Some(Arc::new(handler));
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
                if let Some(handler) = self.handler.as_mut().and_then(Arc::get_mut) {
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
            Ok(BgResult::SearchComplete { results, query }) => {
                self.status = format!("{} results for {query:?}", results.len());
                self.search_results = results;
                self.bg_rx = None;
                self.loading = false;
            }
            Ok(BgResult::FileLoaded { result, data }) => {
                let mut sel = SelectedFile::new(result.clone());
                match data {
                    Ok(data) => {
                        // Dispatch to the first matching preview plugin.
                        // Plugins are registered in `crate::preview::registry()`.
                        // The sibling fetchers let multi-file formats
                        // (WMO root → groups) pull related files from the
                        // open archive by name OR by FileDataID. Modern
                        // WoW (Legion+) uses FDIDs for group references.
                        let handler_ref = self.handler.as_ref();
                        let by_name = |path: &str| -> Option<Vec<u8>> {
                            handler_ref?.open_file_by_name(path).ok()
                        };
                        let by_fdid = |id: u32| -> Option<Vec<u8>> {
                            handler_ref?.open_file_by_fdid(id).ok()
                        };
                        let siblings = crate::preview::SiblingFetcher {
                            by_name: &by_name,
                            by_fdid: &by_fdid,
                        };
                        sel.preview =
                            crate::preview::run(result.filename.as_deref(), &data, ctx, &siblings);

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
            // Lossy decode: community listfiles are mostly ASCII but
            // occasionally contain non-UTF-8 bytes (Latin-1 path fragments,
            // stray binary). Strict read_to_string fails the whole file;
            // lossy decode keeps every valid path and replaces only the
            // bad bytes inside whichever line they appear on.
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let content = String::from_utf8_lossy(&bytes);
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
    ///
    /// Auto-detects whether `search_text` is a glob pattern (contains `*`,
    /// `?`, `{`, or `[`) and dispatches accordingly: globs resolve against
    /// the virtual file tree via [`PathQuery`](rustydemon_lib::PathQuery),
    /// everything else uses the existing case-insensitive substring search
    /// over the root manifest.
    pub fn run_search(&mut self) {
        if self.handler.is_none() {
            return;
        }
        self.cancel_bg();
        self.browsed_folder = None;
        self.search_results.clear();
        self.loading = true;
        self.status = format!("Searching for {:?}…", self.search_text);

        let handler = Arc::clone(self.handler.as_ref().unwrap());
        let query = self.search_text.clone();
        let (tx, rx) = mpsc::channel();
        self.bg_rx = Some(rx);

        std::thread::spawn(move || {
            let results = handler.search_by_text(&query, 500);
            let _ = tx.send(BgResult::SearchComplete { results, query });
        });
    }

    /// Run the full global (deep) search — every entry, optionally searching
    /// inside container files.
    pub fn run_deep_search(&mut self) {
        if self.handler.is_none() {
            return;
        }
        self.cancel_bg();
        self.search_results.clear();
        self.loading = true;
        self.status = format!("Deep searching for {:?}…", self.search_text);

        let handler = Arc::clone(self.handler.as_ref().unwrap());
        let query = self.search_text.clone();
        let (tx, rx) = mpsc::channel();
        self.bg_rx = Some(rx);

        std::thread::spawn(move || {
            let results = handler.search_by_text(&query, 0);
            let _ = tx.send(BgResult::SearchComplete { results, query });
        });
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
        // Apply any viewer-override swap requested on the previous frame
        // BEFORE we start drawing.  Doing this at the top of update()
        // guarantees the old `PreviewOutput`'s texture handle is dropped
        // while no egui paint command references it — dropping mid-frame
        // crashes wgpu with "Texture has been destroyed".
        if let Some(new_override) = self.pending_preview_override.take() {
            crate::ui::preview::apply_preview_override(self, new_override, ctx);
        }
        self.apply_pending_audio();
        self.poll_background(ctx);
        crate::ui::draw(ctx, self);
        crate::viewport3d::show_window(ctx, &mut self.viewport3d_open);
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

    // Known Blizzard game UIDs → display names.  Some installs (notably
    // the Mac Battle.net Diablo III) ship with an empty `.build.info`
    // Product column, so we infer the UID from the CDN path (e.g.
    // `tpr/diablo3`) — those verbose spellings are listed alongside the
    // short ones so the menu label is right either way.
    let uid_names: &[(&str, &str)] = &[
        ("fenris", "Diablo IV"),
        ("wow", "World of Warcraft"),
        ("d3", "Diablo III"),
        ("diablo3", "Diablo III"),
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

    // Common install directories to scan (Windows + Linux/Steam Deck + macOS).
    let mut candidates: Vec<std::path::PathBuf> = vec![
        // Windows — Battle.net / Steam
        "C:/Program Files (x86)".into(),
        "C:/Program Files".into(),
        "D:/Games".into(),
        "E:/Games".into(),
        "C:/Program Files (x86)/Steam/steamapps/common".into(),
        "D:/SteamLibrary/steamapps/common".into(),
        "E:/SteamLibrary/steamapps/common".into(),
        // macOS — Battle.net installs each game as a top-level directory
        // under /Applications (e.g. `/Applications/Diablo III`).  Scanning
        // one level deep finds every Blizzard title without enumerating
        // individual `.app` bundles — the `.build.info` filter below
        // rejects anything that isn't a CASC root.
        "/Applications".into(),
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
