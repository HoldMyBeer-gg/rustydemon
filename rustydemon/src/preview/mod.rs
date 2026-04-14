//! Preview plug-in architecture.
//!
//! Each file format that wants a richer view than the default hex dump
//! implements [`PreviewPlugin`].  Plugins are consulted in priority order
//! by [`poll_background`](crate::app::CascExplorerApp::poll_background);
//! the first one whose [`can_preview`](PreviewPlugin::can_preview) returns
//! `true` takes ownership of the selection.
//!
//! This mirrors the existing [`deep_search`](crate::deep_search) plug-in
//! pattern so users can port one new format by dropping a module under
//! `preview/` and adding one line to [`registry`].

pub mod audio;
pub mod blp;
pub mod model3d;
pub mod pcx;
pub mod pow;
pub mod tex;
pub mod text;
pub mod vid;

use std::sync::Arc;

/// Closure type used by [`ExportAction::build`] to transform the original
/// file bytes into the exported representation (e.g. PNG, BK2).
pub type ExportBuilder = Arc<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync>;

/// Output produced by a preview plugin for one selected file.
///
/// All fields are optional — a minimalist plugin might only fill in
/// `text`, while a texture plugin fills in `texture` + `texture_pixels`
/// and adds an "Export As PNG" action.  The UI renders whichever fields
/// are present, in a fixed order, so plugins never have to think about
/// layout.
pub struct PreviewOutput {
    /// A human-readable text block (shown in a monospace scroll view).
    /// Used for text-like files and format summaries.
    pub text: Option<String>,
    /// GPU-side texture handle for inline image rendering.
    pub texture: Option<egui::TextureHandle>,
    /// CPU-side RGBA pixels + `(width, height)` kept so texture-based
    /// formats can be re-exported to PNG without re-decoding.
    pub texture_pixels: Option<(Vec<u8>, u32, u32)>,
    /// Extra export buttons beyond the baseline `Export Raw`.
    pub extra_exports: Vec<ExportAction>,
    /// Indexed CPU-side mesh for the wgpu 3D viewport. When present, the
    /// preview pane allocates an inline 3D rect and feeds this to
    /// [`crate::viewport3d::paint_mesh`].
    pub mesh3d: Option<Arc<Mesh3dCpu>>,
}

/// CPU-side indexed mesh handed from a preview plugin to the 3D viewport.
/// Geometry + per-batch material ids; actual material/texture lookup
/// against a root file is a later phase.
pub struct Mesh3dCpu {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Index ranges for separately-coloured draws. Each batch covers
    /// `indices[start_index .. start_index + index_count]` and tags it
    /// with a stable `material_id` the renderer hashes into a colour.
    pub batches: Vec<MeshBatch>,
}

#[derive(Clone, Copy)]
pub struct MeshBatch {
    pub start_index: u32,
    pub index_count: u32,
    pub material_id: u32,
}

impl PreviewOutput {
    /// Construct an empty output that a plugin can fill in.
    pub fn new() -> Self {
        Self {
            text: None,
            texture: None,
            texture_pixels: None,
            extra_exports: vec![],
            mesh3d: None,
        }
    }

    /// Plugins call this to register an extra export button.
    #[allow(dead_code)] // Convenience builder; not all plugins use it.
    pub fn add_export(mut self, action: ExportAction) -> Self {
        self.extra_exports.push(action);
        self
    }
}

/// A single export-button row registered by a plugin.
///
/// The UI wires the button label to a file dialog with the given
/// filter and then calls `build` to produce the bytes to write.
#[derive(Clone)]
pub struct ExportAction {
    pub label: &'static str,
    pub default_extension: &'static str,
    pub filter_name: &'static str,
    /// Closure that takes the original file bytes and returns the
    /// transformed bytes to write.  For lossless exports (`Export Raw`)
    /// this is the identity function; for texture exports it re-encodes
    /// to PNG; for `.vid` → `.bk2` it strips the 128-byte header.
    pub build: ExportBuilder,
}

/// Plug-in interface for format-specific preview panels.
///
/// Implementers go under `src/preview/` and register themselves in
/// [`registry`].  Keep `can_preview` cheap — it runs on every file load.
pub trait PreviewPlugin: Send + Sync {
    /// Human-readable name (e.g. `".blp texture"`).  Used in debug output
    /// and future plugin-management UI.
    #[allow(dead_code)]
    fn name(&self) -> &str;

    /// Return `true` if this plugin should handle the given file.
    ///
    /// Plugins may check the filename, the data, or both.  Returning
    /// `false` lets lower-priority plugins (or the hex-dump fallback)
    /// take over.
    fn can_preview(&self, filename: &str, data: &[u8]) -> bool;

    /// Build the preview output.  Called only after `can_preview`
    /// returns `true` for the same `(filename, data)` pair.
    ///
    /// `ctx` is provided for plugins that need to upload GPU textures.
    fn build(&self, filename: &str, data: &[u8], ctx: &egui::Context) -> PreviewOutput;
}

/// All registered preview plug-ins, in priority order.
///
/// The first plugin whose `can_preview` returns `true` wins.  Add a new
/// plugin here after dropping its module into `src/preview/`.
pub fn registry() -> Vec<Box<dyn PreviewPlugin>> {
    vec![
        // Texture-bearing formats come first — they short-circuit before
        // the text heuristic could false-positive on a binary header.
        Box::new(blp::BlpPreview),
        Box::new(pcx::PcxPreview),
        Box::new(tex::TexPreview),
        // Structured-data format summaries.
        Box::new(model3d::Model3dPreview),
        Box::new(pow::PowPreview),
        Box::new(vid::VidPreview),
        Box::new(audio::AudioPreview),
        // Generic text detector runs last so specific formats win.
        Box::new(text::TextPreview),
    ]
}

/// Run all registered plugins against a loaded file and return the first
/// matching preview, or `None` if no plugin can handle it.
pub fn run(filename: Option<&str>, data: &[u8], ctx: &egui::Context) -> Option<PreviewOutput> {
    let name = filename.unwrap_or("");
    for plugin in registry() {
        if plugin.can_preview(name, data) {
            return Some(plugin.build(name, data, ctx));
        }
    }
    None
}

/// Encode an 8-bit RGBA pixel buffer to a PNG byte stream.  Used by
/// texture-format plugins from their `Export As PNG` closures.
pub fn encode_png(pixels: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    use image::{codecs::png::PngEncoder, ImageEncoder};
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(pixels, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out)
}
