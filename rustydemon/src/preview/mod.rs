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
pub mod m2;
pub mod model3d;
pub mod model_d2r;
pub mod pcx;
pub mod pow;
pub mod tex;
pub mod text;
pub mod texture;
pub mod vid;

use std::sync::Arc;

/// Closure type used by [`ExportAction::build`] to transform the original
/// file bytes into the exported representation (e.g. PNG, BK2).
///
/// The second argument is the output path chosen by the user. Most
/// exporters ignore it, but multi-file exports (OBJ+MTL+textures)
/// use it to derive sibling file paths.
pub type ExportBuilder =
    Arc<dyn Fn(&[u8], &std::path::Path) -> Result<Vec<u8>, String> + Send + Sync>;

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
/// Geometry + UVs + per-batch material indices + decoded textures.
pub struct Mesh3dCpu {
    pub positions: Vec<[f32; 3]>,
    /// One UV per vertex, laid out parallel to `positions`. Filled with
    /// zeros if the source format doesn't carry texture coordinates.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// Index ranges for separately-textured draws. Each batch covers
    /// `indices[start_index .. start_index + index_count]` and references
    /// a material slot by index into `materials` (or `u32::MAX` for
    /// "no material — use fallback texture").
    pub batches: Vec<MeshBatch>,
    /// Decoded materials in the order referenced by `MeshBatch::material_id`.
    /// Empty when the source had no material info (e.g. a single group
    /// file viewed without its root); the renderer falls back to flat
    /// per-batch hash colours in that case.
    pub materials: Vec<MeshMaterial>,
}

#[derive(Clone, Copy)]
pub struct MeshBatch {
    pub start_index: u32,
    pub index_count: u32,
    pub material_id: u32,
}

/// CPU-side decoded texture for one material. RGBA8, ready to upload.
/// `None` rgba means "decode failed, use fallback".
pub struct MeshMaterial {
    pub rgba: Option<Vec<u8>>,
    pub width: u32,
    pub height: u32,
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

/// Bundle of closures plugins can call to fetch sibling files from the
/// open archive. Two lookup paths because modern WoW (Legion+) uses
/// FileDataIDs rather than virtual paths for things like WMO group
/// references; older formats still resolve by name.
pub struct SiblingFetcher<'a> {
    pub by_name: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    pub by_fdid: &'a dyn Fn(u32) -> Option<Vec<u8>>,
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
    /// `fetch_sibling` lets the plugin pull related files out of the
    /// open archive — e.g. a WMO root loading its group files. Most
    /// plugins ignore it.
    fn build(
        &self,
        filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        siblings: &SiblingFetcher<'_>,
    ) -> PreviewOutput;
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
        // D2R `<DE(` container — magic-sniffed, so it sits above the
        // extension-only `.tex` plugin to claim D2R textures before a
        // name collision could matter.
        Box::new(texture::TextureDePreview),
        Box::new(tex::TexPreview),
        // Structured-data format summaries.
        Box::new(model3d::Model3dPreview),
        // M2: 3D viewport + BLP textures via TXID FDIDs.
        Box::new(m2::M2Preview),
        // D2R .model (Granny3D): 3D viewport + texture materials.
        Box::new(model_d2r::ModelD2rPreview),
        Box::new(pow::PowPreview),
        Box::new(vid::VidPreview),
        Box::new(audio::AudioPreview),
        // Generic text detector runs last so specific formats win.
        Box::new(text::TextPreview),
    ]
}

/// Run all registered plugins against a loaded file and return the first
/// matching preview, or `None` if no plugin can handle it.
pub fn run(
    filename: Option<&str>,
    data: &[u8],
    ctx: &egui::Context,
    siblings: &SiblingFetcher<'_>,
) -> Option<PreviewOutput> {
    let name = filename.unwrap_or("");
    for plugin in registry() {
        if plugin.can_preview(name, data) {
            return Some(plugin.build(name, data, ctx, siblings));
        }
    }
    None
}

/// Force a specific plugin to handle a file regardless of its
/// `can_preview` vote.  Used by the "Viewer:" override dropdown in the
/// preview panel to try an unrelated decoder on a mystery format.
///
/// Plugins already fail soft when handed unrelated bytes (they fill in
/// a human-readable `text` blob and return), so forcing a mismatch is
/// safe — you either see a decoded image or a "could not decode"
/// message, never a panic.
pub fn run_with_override(
    plugin_index: usize,
    filename: Option<&str>,
    data: &[u8],
    ctx: &egui::Context,
    siblings: &SiblingFetcher<'_>,
) -> Option<PreviewOutput> {
    let name = filename.unwrap_or("");
    let registry = registry();
    let plugin = registry.get(plugin_index)?;
    Some(plugin.build(name, data, ctx, siblings))
}

/// Names of every registered plugin, in the same order as
/// [`registry`].  The preview panel uses this to populate its
/// "Viewer:" override dropdown.
pub fn plugin_names() -> Vec<String> {
    registry().iter().map(|p| p.name().to_owned()).collect()
}

/// Encode a `Mesh3dCpu` as Wavefront OBJ.  Used by model preview
/// plugins for their "Export As OBJ" buttons.
pub fn encode_obj(mesh: &Mesh3dCpu) -> Vec<u8> {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "# rustydemon OBJ export");
    for p in &mesh.positions {
        let _ = writeln!(s, "v {:.6} {:.6} {:.6}", p[0], p[1], p[2]);
    }
    for uv in &mesh.uvs {
        let _ = writeln!(s, "vt {:.6} {:.6}", uv[0], uv[1]);
    }
    let has_uvs = !mesh.uvs.is_empty();
    for tri in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] + 1, tri[1] + 1, tri[2] + 1);
        if has_uvs {
            let _ = writeln!(s, "f {a}/{a} {b}/{b} {c}/{c}");
        } else {
            let _ = writeln!(s, "f {a} {b} {c}");
        }
    }
    s.into_bytes()
}

/// Encode a `Mesh3dCpu` as OBJ + MTL + texture PNGs.
///
/// Writes the MTL and texture PNGs as siblings of `obj_path`. Returns
/// the OBJ file bytes (the caller writes those to `obj_path`).
pub fn encode_obj_with_materials(
    mesh: &Mesh3dCpu,
    obj_path: &std::path::Path,
) -> Result<Vec<u8>, String> {
    use std::fmt::Write;

    let stem = obj_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model");
    let parent = obj_path.parent().unwrap_or(std::path::Path::new("."));
    let mtl_name = format!("{stem}.mtl");

    // ── Write texture PNGs and build the MTL ─────────────────────────
    let mut mtl = String::new();
    let _ = writeln!(mtl, "# rustydemon material export");
    for (i, mat) in mesh.materials.iter().enumerate() {
        let mat_name = format!("material_{i}");
        let _ = writeln!(mtl, "\nnewmtl {mat_name}");
        if let Some(rgba) = &mat.rgba {
            let tex_name = format!("{stem}_tex{i}.png");
            let tex_path = parent.join(&tex_name);
            let png = encode_png(rgba, mat.width, mat.height)
                .map_err(|e| format!("texture {i} PNG encode: {e}"))?;
            std::fs::write(&tex_path, &png)
                .map_err(|e| format!("write {}: {e}", tex_path.display()))?;
            let _ = writeln!(mtl, "map_Kd {tex_name}");
        }
    }

    // Write the MTL file.
    let mtl_path = parent.join(&mtl_name);
    std::fs::write(&mtl_path, mtl.as_bytes())
        .map_err(|e| format!("write {}: {e}", mtl_path.display()))?;

    // ── Build the OBJ ────────────────────────────────────────────────
    let mut s = String::new();
    let _ = writeln!(s, "# rustydemon OBJ export");
    let _ = writeln!(s, "mtllib {mtl_name}");
    for p in &mesh.positions {
        let _ = writeln!(s, "v {:.6} {:.6} {:.6}", p[0], p[1], p[2]);
    }
    for uv in &mesh.uvs {
        let _ = writeln!(s, "vt {:.6} {:.6}", uv[0], uv[1]);
    }
    let has_uvs = !mesh.uvs.is_empty();

    // Group faces by batch so each batch gets its material assignment.
    for (bi, batch) in mesh.batches.iter().enumerate() {
        let mat_name = if (batch.material_id as usize) < mesh.materials.len() {
            format!("material_{}", batch.material_id)
        } else {
            format!("material_{bi}")
        };
        let _ = writeln!(s, "usemtl {mat_name}");
        let start = batch.start_index as usize;
        let end = start + batch.index_count as usize;
        let tri_indices = &mesh.indices[start..end];
        for tri in tri_indices.chunks_exact(3) {
            let (a, b, c) = (tri[0] + 1, tri[1] + 1, tri[2] + 1);
            if has_uvs {
                let _ = writeln!(s, "f {a}/{a} {b}/{b} {c}/{c}");
            } else {
                let _ = writeln!(s, "f {a} {b} {c}");
            }
        }
    }

    Ok(s.into_bytes())
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
