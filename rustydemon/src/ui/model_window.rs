//! The 3D model in a window of its own.
//!
//! The preview pane gives a model a strip 240 points tall.  This is a
//! separate window the desktop can place anywhere: beside the game, on a
//! second screen, maximised.  It shows whatever model is selected in the
//! main window, with the same camera.
//!
//! The renderer keeps one camera and one drawing surface per model, so a
//! model is drawn in one place at a time: while this window is open the
//! preview pane says so and leaves the drawing to it.

use std::sync::Arc;

use crate::app::CascExplorerApp;
use crate::preview::{Mesh3dCpu, PreviewOutput};
use crate::ui::theme::rd;

pub const TITLE: &str = "Rusty Demon - Model";
/// On the button that opens the window.
pub const OPEN: &str = "Open in window";
/// In the preview pane, where the model would be.
pub const MOVED: &str = "Shown in the model window.";
/// In the window, when the selection is not a model.
pub const NOTHING: &str = "Select a model in the main window.";
const HINT: &str = "Drag to rotate, Shift+drag to pan, Ctrl+drag or scroll to zoom";

/// The mesh a preview draws: the chosen variant's, or its only one.
pub fn mesh_of(preview: &PreviewOutput) -> Option<Arc<Mesh3dCpu>> {
    match &preview.variants {
        Some(variants) => Some(variants.mesh()),
        None => preview.mesh3d.clone(),
    }
}

/// The "Mesh" part of the preview pane: the model, or a note that the
/// window has it.  The button sets `window_open`.
pub fn pane(ui: &mut egui::Ui, mesh: Arc<Mesh3dCpu>, window_open: &mut bool) {
    if *window_open {
        ui.label(egui::RichText::new(MOVED).color(rd::FG_MUTED));
        return;
    }
    crate::viewport3d::paint_mesh(ui, mesh);
    if ui.button(OPEN).clicked() {
        *window_open = true;
    }
}

/// What the window shows.
fn body(ui: &mut egui::Ui, mesh: Option<Arc<Mesh3dCpu>>) {
    let frame = egui::Frame::central_panel(ui.style()).fill(rd::BG_APP);
    frame.show(ui, |ui| match mesh {
        Some(mesh) => {
            ui.label(egui::RichText::new(HINT).color(rd::FG_MUTED));
            let size = ui.available_size().max(egui::Vec2::splat(64.0));
            crate::viewport3d::paint_mesh_sized(ui, mesh, size);
        }
        None => {
            ui.label(egui::RichText::new(NOTHING).color(rd::FG_MUTED));
        }
    });
}

/// Draw the window, if it is open.  Closing it clears the flag.
pub fn show(ctx: &egui::Context, app: &mut CascExplorerApp) {
    if !app.model_window_open {
        return;
    }
    let mesh = app
        .selected
        .as_ref()
        .and_then(|s| s.preview.as_ref())
        .and_then(mesh_of);
    let builder = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size([900.0, 900.0]);
    let id = egui::ViewportId::from_hash_of("model_window");
    let closed = ctx.show_viewport_immediate(id, builder, |ui, _class| {
        body(ui, mesh.clone());
        ui.input(|i| i.viewport().close_requested())
    });
    if closed {
        app.model_window_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y::testing::nodes;
    use egui::accesskit::Role;

    fn triangle() -> Arc<Mesh3dCpu> {
        Arc::new(Mesh3dCpu {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2]; 3],
            indices: vec![0, 1, 2],
            bbox_min: [0.0; 3],
            bbox_max: [1.0, 1.0, 0.0],
            batches: Vec::new(),
            materials: Vec::new(),
        })
    }

    /// `(has a 3D canvas, the words on buttons and labels)`
    fn drawn(draw: impl FnMut(&mut egui::Ui)) -> (bool, Vec<String>) {
        let nodes = nodes(draw);
        let canvas = nodes.iter().any(|n| n.role() == Role::Canvas);
        let words = nodes
            .iter()
            .filter(|n| matches!(n.role(), Role::Button | Role::Label))
            .filter_map(|n| n.label().or_else(|| n.value()).map(str::to_owned))
            .collect();
        (canvas, words)
    }

    #[test]
    fn the_pane_draws_the_model_and_offers_the_window() {
        let (canvas, words) = drawn(|ui| pane(ui, triangle(), &mut false));
        assert!(canvas);
        assert!(words.contains(&OPEN.to_string()), "{words:?}");
        assert!(!words.contains(&MOVED.to_string()), "{words:?}");
    }

    #[test]
    fn with_the_window_open_the_pane_says_where_the_model_went() {
        let (canvas, words) = drawn(|ui| pane(ui, triangle(), &mut true));
        assert!(!canvas, "one camera per model: it is drawn in one place");
        assert!(words.contains(&MOVED.to_string()), "{words:?}");
        assert!(!words.contains(&OPEN.to_string()), "{words:?}");
    }

    #[test]
    fn the_window_draws_the_model_with_the_controls_spelled_out() {
        let (canvas, words) = drawn(|ui| body(ui, Some(triangle())));
        assert!(canvas);
        assert!(words.contains(&HINT.to_string()), "{words:?}");
    }

    #[test]
    fn the_window_says_what_to_do_when_the_selection_is_not_a_model() {
        let (canvas, words) = drawn(|ui| body(ui, None));
        assert!(!canvas);
        assert!(words.contains(&NOTHING.to_string()), "{words:?}");
    }
}
