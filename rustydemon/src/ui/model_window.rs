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
/// The name of the set search box.
pub const SEARCH: &str = "Armour set";
/// On the button that removes what is worn.
pub const TAKE_OFF: &str = "Take off";
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

/// A set the search box suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub items: Vec<u32>,
}

/// What the user asked for in the armour row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dress {
    Wear(Suggestion),
    Undress,
}

/// The armour row: a box to type a set's name into, the sets that match as
/// buttons under it, and what is worn now with a button to take it off.
///
/// `problem` replaces the row when the game's set list cannot be used.
fn armour(
    ui: &mut egui::Ui,
    typed: &mut String,
    suggestions: &[Suggestion],
    wearing: Option<&str>,
    problem: Option<&str>,
) -> Option<Dress> {
    if let Some(problem) = problem {
        ui.label(
            egui::RichText::new(format!("Armour sets are not available: {problem}"))
                .color(rd::FG_MUTED),
        );
        return None;
    }
    let mut asked = None;
    ui.horizontal(|ui| {
        let search = ui.add(
            egui::TextEdit::singleline(typed)
                .hint_text("Armour set, e.g. Lawbringer")
                .desired_width(260.0),
        );
        crate::ui::a11y::name(ui.ctx(), search.id, SEARCH);
        if let Some(name) = wearing {
            ui.label(format!("Wearing: {name}"));
            if ui.button(TAKE_OFF).clicked() {
                asked = Some(Dress::Undress);
            }
        }
    });
    if !suggestions.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for set in suggestions {
                let label = format!("{} ({} pieces)", set.name, set.items.len());
                if ui.button(label).clicked() {
                    asked = Some(Dress::Wear(set.clone()));
                }
            }
        });
    }
    asked
}

/// The armour row's state for one frame.
struct Armour<'a> {
    typed: &'a mut String,
    suggestions: &'a [Suggestion],
    wearing: Option<&'a str>,
    problem: Option<&'a str>,
}

/// What the window shows.  Returns what was asked for in the armour row.
fn body(ui: &mut egui::Ui, mesh: Option<Arc<Mesh3dCpu>>, row: Armour<'_>) -> Option<Dress> {
    let frame = egui::Frame::central_panel(ui.style()).fill(rd::BG_APP);
    frame
        .show(ui, |ui| match mesh {
            Some(mesh) => {
                let asked = armour(ui, row.typed, row.suggestions, row.wearing, row.problem);
                ui.label(egui::RichText::new(HINT).color(rd::FG_MUTED));
                let size = ui.available_size().max(egui::Vec2::splat(64.0));
                crate::viewport3d::paint_mesh_sized(ui, mesh, size);
                asked
            }
            None => {
                ui.label(egui::RichText::new(NOTHING).color(rd::FG_MUTED));
                None
            }
        })
        .inner
}

/// How many sets the search box offers at once.
const SUGGESTIONS: usize = 8;

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
    // The set list is read only once something is typed: other games
    // have none, and it costs a moment the first time.
    let mut suggestions = Vec::new();
    let mut problem = None;
    if !app.set_query.trim().is_empty() {
        if let Some(handler) = app.handler.as_ref() {
            let sets = crate::preview::character::tables(&app.character_looks, handler)
                .and_then(|t| t.sets.as_ref().map_err(String::clone));
            match sets {
                Ok(sets) => {
                    let found =
                        crate::preview::item_sets::suggest(sets, &app.set_query, SUGGESTIONS);
                    suggestions = found
                        .into_iter()
                        .map(|s| Suggestion {
                            name: s.name.clone(),
                            items: s.items.clone(),
                        })
                        .collect();
                }
                Err(e) => problem = Some(e),
            }
        }
    }
    let mut typed = std::mem::take(&mut app.set_query);
    let wearing = app.worn_name.clone();

    let builder = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size([900.0, 900.0]);
    let id = egui::ViewportId::from_hash_of("model_window");
    let (asked, closed) = ctx.show_viewport_immediate(id, builder, |ui, _class| {
        let row = Armour {
            typed: &mut typed,
            suggestions: &suggestions,
            wearing: wearing.as_deref(),
            problem: problem.as_deref(),
        };
        let asked = body(ui, mesh.clone(), row);
        (asked, ui.input(|i| i.viewport().close_requested()))
    });
    app.set_query = typed;
    match asked {
        Some(Dress::Wear(set)) => {
            app.worn_items = set.items;
            app.worn_name = Some(set.name);
            app.set_query.clear();
        }
        Some(Dress::Undress) => {
            app.worn_items.clear();
            app.worn_name = None;
        }
        None => {}
    }
    if asked_any(&app.worn_items, &mut app.worn_drawn) {
        // Build the preview again with the new outfit.  Done at the top of
        // the next frame, where it is safe to drop the old textures.
        let current = app.selected.as_ref().and_then(|s| s.preview_override);
        app.pending_preview_override = Some(current);
        ctx.request_repaint();
    }
    if closed {
        app.model_window_open = false;
    }
}

/// Whether what is worn differs from what the preview was last built with;
/// remembers the new outfit when it does.
fn asked_any(worn: &[u32], drawn: &mut Vec<u32>) -> bool {
    if worn == drawn.as_slice() {
        return false;
    }
    *drawn = worn.to_vec();
    true
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
        let (canvas, words) = drawn(|ui| {
            body(ui, Some(triangle()), no_armour(&mut String::new()));
        });
        assert!(canvas);
        assert!(words.contains(&HINT.to_string()), "{words:?}");
    }

    fn no_armour(typed: &mut String) -> Armour<'_> {
        Armour {
            typed,
            suggestions: &[],
            wearing: None,
            problem: None,
        }
    }

    #[test]
    fn the_preview_is_rebuilt_only_when_the_outfit_changes() {
        let mut drawn = Vec::new();
        assert!(!asked_any(&[], &mut drawn), "nothing worn, nothing drawn");
        assert!(asked_any(&[1, 2], &mut drawn));
        assert!(!asked_any(&[1, 2], &mut drawn), "already drawn with these");
        assert!(asked_any(&[], &mut drawn), "taking it off is a change");
    }

    fn lawbringer() -> Suggestion {
        Suggestion {
            name: "Lawbringer Armor".into(),
            items: vec![16853, 16854],
        }
    }

    #[test]
    fn the_armour_row_has_a_named_box_and_a_button_per_match() {
        let found = [lawbringer()];
        let nodes = nodes(|ui| {
            armour(ui, &mut "law".to_string(), &found, None, None);
        });
        let named = |role, name: &str| {
            nodes
                .iter()
                .any(|n| n.role() == role && n.label() == Some(name))
        };
        assert!(
            named(Role::TextInput, SEARCH),
            "the box says what it is for"
        );
        assert!(named(Role::Button, "Lawbringer Armor (2 pieces)"));
        assert!(!named(Role::Button, TAKE_OFF), "nothing is worn yet");
    }

    #[test]
    fn what_is_worn_is_named_and_can_be_taken_off() {
        let (_, words) = drawn(|ui| {
            armour(ui, &mut String::new(), &[], Some("Lawbringer Armor"), None);
        });
        assert!(
            words.contains(&"Wearing: Lawbringer Armor".to_string()),
            "{words:?}"
        );
        assert!(words.contains(&TAKE_OFF.to_string()), "{words:?}");
    }

    #[test]
    fn a_set_list_that_cannot_be_used_says_why() {
        let (_, words) = drawn(|ui| {
            armour(
                ui,
                &mut String::new(),
                &[],
                None,
                Some("ItemSet could not be read"),
            );
        });
        assert!(
            words
                .iter()
                .any(|w| w.contains("ItemSet could not be read")),
            "{words:?}"
        );
    }

    #[test]
    fn the_window_says_what_to_do_when_the_selection_is_not_a_model() {
        let (canvas, words) = drawn(|ui| {
            body(ui, None, no_armour(&mut String::new()));
        });
        assert!(!canvas);
        assert!(words.contains(&NOTHING.to_string()), "{words:?}");
    }
}
