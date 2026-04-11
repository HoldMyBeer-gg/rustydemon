use rustydemon_lib::{CascFolder, CascFile};
use crate::app::CascExplorerApp;

/// Draw the left-panel file tree.
/// Returns the hash of a file the user clicked.
pub fn draw_tree(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<u64> {
    ui.heading("Search Archives");
    ui.separator();

    if app.handler.is_none() {
        ui.label("No archive open.");
        return None;
    }

    // Apply any pending programmatic expand/collapse from the View menu.
    apply_expansion_commands(ui, app);

    let mut clicked_hash: Option<u64> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        let id = egui::Id::new("tree_root");
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(), id, true,
        );
        state.show_header(ui, |ui| {
            ui.label("🗂 Game Root");
        }).body(|ui| {
            clicked_hash = draw_folder_recursive(ui, app, "");
        });
    });

    clicked_hash
}

fn draw_folder_recursive(
    ui: &mut egui::Ui,
    app: &mut CascExplorerApp,
    path_prefix: &str,
) -> Option<u64> {
    let handler = app.handler.as_ref()?;
    let root_folder = match handler.root_folder.as_ref() {
        Some(f) => f,
        None => {
            ui.label("(load a listfile to browse)");
            return None;
        }
    };

    let folder: &CascFolder = if path_prefix.is_empty() {
        root_folder
    } else {
        match root_folder.navigate(path_prefix) {
            Some(f) => f,
            None => return None,
        }
    };

    // Clone folder contents to release the borrow before calling back into app.
    let sub_names: Vec<String> = {
        let mut v: Vec<String> = folder.folders.keys().cloned().collect();
        v.sort_unstable();
        v
    };
    let files: Vec<CascFile> = {
        let mut v: Vec<CascFile> = folder.files.values().cloned().collect();
        v.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        v
    };

    let mut clicked_hash: Option<u64> = None;

    // ── Sub-folders ────────────────────────────────────────────────────────────
    for name in &sub_names {
        let child_path = if path_prefix.is_empty() {
            name.clone()
        } else {
            format!("{path_prefix}/{name}")
        };

        let id = egui::Id::new(&child_path);
        // Default closed except when programmatically expanded.
        let default_open = app.expanded.contains(&child_path);
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(), id, default_open,
        );
        let child_path_for_body = child_path.clone();
        let body = state.show_header(ui, |ui| {
            ui.label(format!("📁 {name}"));
        }).body(|ui| {
            draw_folder_recursive(ui, app, &child_path_for_body)
        });
        if let Some(body_inner) = body.2 {
            if let Some(hash) = body_inner.inner {
                clicked_hash = clicked_hash.or(Some(hash));
            }
        }
    }

    // ── Files ──────────────────────────────────────────────────────────────────
    let selected_hash = app.selected.as_ref().map(|s| s.result.hash);
    for file in &files {
        let is_selected = selected_hash == Some(file.hash);
        let icon = file_icon(&file.name);
        let label = format!("  {icon} {}", file.name);
        if ui.selectable_label(is_selected, &label).clicked() {
            clicked_hash = Some(file.hash);
        }
    }

    clicked_hash
}

/// Apply programmatic open/close commands stored in `app.expanded`.
///
/// The View menu writes to `app.expanded`; here we push those states into
/// egui's CollapsingState memory so the tree reflects them.
fn apply_expansion_commands(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    if !app.expansion_dirty {
        return;
    }
    app.expansion_dirty = false;

    let handler = match app.handler.as_ref() { Some(h) => h, None => return };
    let root_folder = match handler.root_folder.as_ref() { Some(f) => f, None => return };

    // Walk every folder and set the CollapsingState according to app.expanded.
    set_folder_states(ui.ctx(), root_folder, "", &app.expanded);
}

fn set_folder_states(
    ctx: &egui::Context,
    folder: &CascFolder,
    prefix: &str,
    expanded: &std::collections::HashSet<String>,
) {
    for (name, sub) in &folder.folders {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let is_open = expanded.contains(&path);
        let id = egui::Id::new(&path);
        egui::collapsing_header::CollapsingState::load_with_default_open(ctx, id, false)
            .set_open(is_open);
        set_folder_states(ctx, sub, &path, expanded);
    }
}

fn file_icon(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".blp") { "🖼" }
    else if lower.ends_with(".m2") || lower.ends_with(".mdx") { "🧊" }
    else if lower.ends_with(".pow") || lower.ends_with(".gam") { "⚙" }
    else if lower.ends_with(".mp3") || lower.ends_with(".ogg") || lower.ends_with(".wav") { "🎵" }
    else { "📄" }
}
