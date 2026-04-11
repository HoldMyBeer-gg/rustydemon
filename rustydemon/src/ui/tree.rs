use crate::app::CascExplorerApp;
use rustydemon_lib::{CascFile, CascFolder};

/// What the user clicked in the tree.
pub enum TreeClick {
    /// A file was clicked (hash).
    File(u64),
    /// A folder was clicked (path).
    Folder(String),
}

/// Draw the left-panel file tree.
/// Returns what the user clicked, if anything.
pub fn draw_tree(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<TreeClick> {
    ui.heading("File Browser");
    ui.separator();

    if app.handler.is_none() {
        ui.label("No archive open.");
        return None;
    }

    // Apply any pending programmatic expand/collapse from the View menu.
    apply_expansion_commands(ui, app);

    let mut click: Option<TreeClick> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        let id = egui::Id::new("tree_root");
        let state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        state
            .show_header(ui, |ui| {
                ui.label("Game Root");
            })
            .body(|ui| {
                click = draw_folder_recursive(ui, app, "");
            });
    });

    click
}

fn draw_folder_recursive(
    ui: &mut egui::Ui,
    app: &mut CascExplorerApp,
    path_prefix: &str,
) -> Option<TreeClick> {
    let handler = app.handler.as_ref()?;
    let root_folder = match handler.root_folder.as_ref() {
        Some(f) => f,
        None => {
            ui.label("No file names available.");
            ui.label("Use File → Load Listfile to browse.");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("WoW: download from\ngithub.com/wowdev/wow-listfile")
                    .small()
                    .color(egui::Color32::from_gray(140)),
            );
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

    let mut click: Option<TreeClick> = None;

    // ── Sub-folders ────────────────────────────────────────────────────────────
    const MAX_FOLDERS_IN_TREE: usize = 200;
    let folder_truncated = sub_names.len() > MAX_FOLDERS_IN_TREE;
    for name in sub_names.iter().take(MAX_FOLDERS_IN_TREE) {
        let child_path = if path_prefix.is_empty() {
            name.clone()
        } else {
            format!("{path_prefix}/{name}")
        };

        let default_open = app.expanded.contains(&child_path);
        let is_browsed = app.browsed_folder.as_deref() == Some(&child_path);
        let child_path_for_body = child_path.clone();

        // CollapsingHeader makes the entire row clickable.
        let resp = egui::CollapsingHeader::new(egui::RichText::new(format!("📁 {name}")).color(
            if is_browsed {
                egui::Color32::from_rgb(100, 180, 255)
            } else {
                egui::Color32::from_gray(220)
            },
        ))
        .id_salt(&child_path_for_body)
        .default_open(default_open)
        .show(ui, |ui| {
            draw_folder_recursive(ui, app, &child_path_for_body)
        });

        // When a folder is expanded (header clicked), also browse it.
        if resp.header_response.clicked() {
            click = Some(TreeClick::Folder(child_path));
        }

        if let Some(inner_click) = resp.body_returned.flatten() {
            click = click.or(Some(inner_click));
        }
    }

    if folder_truncated {
        ui.label(
            egui::RichText::new(format!(
                "  … and {} more folders",
                sub_names.len() - MAX_FOLDERS_IN_TREE
            ))
            .small()
            .color(egui::Color32::from_gray(140)),
        );
    }

    // ── Files ──────────────────────────────────────────────────────────────────
    // Cap file display in the tree to prevent UI slowdown on huge folders.
    const MAX_FILES_IN_TREE: usize = 200;
    let selected_hash = app.selected.as_ref().map(|s| s.result.hash);
    let truncated = files.len() > MAX_FILES_IN_TREE;
    for file in files.iter().take(MAX_FILES_IN_TREE) {
        let is_selected = selected_hash == Some(file.hash);
        let icon = file_icon(&file.name);
        let label = format!("  {icon} {}", file.name);
        if ui.selectable_label(is_selected, &label).clicked() {
            click = Some(TreeClick::File(file.hash));
        }
    }
    if truncated {
        ui.label(
            egui::RichText::new(format!(
                "  … and {} more (click folder to browse)",
                files.len() - MAX_FILES_IN_TREE
            ))
            .small()
            .color(egui::Color32::from_gray(140)),
        );
    }

    click
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

    let handler = match app.handler.as_ref() {
        Some(h) => h,
        None => return,
    };
    let root_folder = match handler.root_folder.as_ref() {
        Some(f) => f,
        None => return,
    };

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
    if lower.ends_with(".blp") || lower.ends_with(".tex") {
        "🖼"
    } else if lower.ends_with(".m2") || lower.ends_with(".mdx") {
        "🧊"
    } else if lower.ends_with(".pow") || lower.ends_with(".gam") {
        "⚙"
    } else if lower.ends_with(".mp3")
        || lower.ends_with(".ogg")
        || lower.ends_with(".wav")
        || lower.ends_with(".wsb")
    {
        "🎵"
    } else if lower.ends_with(".vid") {
        "🎬"
    } else {
        "📄"
    }
}
