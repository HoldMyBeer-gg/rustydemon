use crate::app::CascExplorerApp;
use crate::ui::theme::{self, rd};
use rustydemon_lib::SearchResult;

/// Draw the center panel — shows either search results or browsed folder contents.
/// Returns the clicked [`SearchResult`], if any.
pub fn draw_results(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<SearchResult> {
    // Handle Ctrl+A to select all visible files.
    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
        if let Some(folder_path) = &app.browsed_folder {
            if let Some(handler) = app.handler.as_ref() {
                if let Some(root) = handler.root_folder.as_ref() {
                    let folder = if folder_path.is_empty() {
                        Some(root)
                    } else {
                        root.navigate(folder_path)
                    };
                    if let Some(f) = folder {
                        for file in f.files.values() {
                            app.multi_selected.insert(file.hash);
                        }
                    }
                }
            }
        } else {
            for r in &app.search_results {
                app.multi_selected.insert(r.hash);
            }
        }
    }

    // If we're browsing a folder, show its contents.
    if let Some(folder_path) = &app.browsed_folder {
        return draw_folder_contents(ui, app, folder_path.clone());
    }

    // Otherwise show search results.
    draw_search_results(ui, app)
}

fn draw_folder_contents(
    ui: &mut egui::Ui,
    app: &mut CascExplorerApp,
    folder_path: String,
) -> Option<SearchResult> {
    let handler = app.handler.as_ref()?;
    let root_folder = handler.root_folder.as_ref()?;

    let folder = if folder_path.is_empty() {
        root_folder
    } else {
        root_folder.navigate(&folder_path)?
    };

    // Collect subfolders and files.
    // Only sort/collect what we can display to avoid sorting 100k+ entries.
    let mut sub_names: Vec<String> = folder.folders.keys().cloned().collect();
    sub_names.sort_unstable();

    let total_files = folder.files.len();
    const MAX_FILES: usize = 2000;
    let mut files: Vec<rustydemon_lib::CascFile> = if total_files > MAX_FILES {
        // Take an arbitrary subset rather than sorting everything.
        folder.files.values().take(MAX_FILES).cloned().collect()
    } else {
        folder.files.values().cloned().collect()
    };
    files.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    let total = sub_names.len() + total_files;
    let sel_count = app.multi_selected.len();

    // Header with export buttons.  The folder path is rune-blue
    // (technical data per the design system) and the item count is
    // muted, so the two scan separately.
    ui.horizontal(|ui| {
        ui.label(theme::engraved("Folder"));
        ui.label(
            egui::RichText::new(if folder_path.is_empty() {
                "/"
            } else {
                folder_path.as_str()
            })
            .monospace()
            .color(rd::RUNE_400),
        );
        ui.label(egui::RichText::new(format!("· {total} items")).color(rd::FG_MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if sel_count > 0 && ui.button(format!("Export {sel_count} Selected")).clicked() {
                app.export_selected();
            }
            if ui.button("Export Folder").clicked() {
                app.export_folder();
            }
            if ui.button("Select All").clicked() {
                for file in &files {
                    app.multi_selected.insert(file.hash);
                }
            }
            if sel_count > 0 && ui.button("Clear Selection").clicked() {
                app.multi_selected.clear();
            }
        });
    });
    ui.separator();

    let mut clicked: Option<SearchResult> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);

        // Show subfolders first (single click to navigate).
        for name in &sub_names {
            let child_path = if folder_path.is_empty() {
                name.clone()
            } else {
                format!("{folder_path}/{name}")
            };
            let label = format!("📁  {name}");
            if ui.selectable_label(false, &label).clicked() {
                app.browsed_folder = Some(child_path);
            }
        }

        // Show files.
        let truncated = total_files > files.len();
        for file in &files {
            let is_multi = app.multi_selected.contains(&file.hash);
            let is_preview = app
                .selected
                .as_ref()
                .map(|s| s.result.hash == file.hash)
                .unwrap_or(false);

            let icon = file_icon(&file.name);
            let label_text = format!("{icon}  {}", file.name);

            ui.push_id(file.hash, |ui| {
                let resp = ui.selectable_label(is_multi || is_preview, &label_text);

                if resp.clicked() {
                    if ctrl_held {
                        // Ctrl+click toggles multi-selection.
                        if is_multi {
                            app.multi_selected.remove(&file.hash);
                        } else {
                            app.multi_selected.insert(file.hash);
                        }
                    } else {
                        // Normal click: select for preview, clear multi-select.
                        app.multi_selected.clear();
                        app.multi_selected.insert(file.hash);

                        if let Some(h) = app.handler.as_ref() {
                            let results = h.search_by_hash(file.hash);
                            if let Some(first) = results.into_iter().next() {
                                clicked = Some(first);
                            }
                        }
                    }
                }
            });
        }

        if truncated {
            ui.label(
                egui::RichText::new(format!(
                    "… showing {} of {} files",
                    files.len(),
                    total_files
                ))
                .small()
                .color(rd::FG_MUTED),
            );
        }
    });

    clicked
}

fn draw_search_results(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<SearchResult> {
    let count = app.search_results.len();
    let sel_count = app.multi_selected.len();

    ui.horizontal(|ui| {
        ui.label(theme::engraved("Search Results"));
        if count > 0 {
            ui.label(
                egui::RichText::new(format!("· {count}"))
                    .color(rd::EMBER_600)
                    .strong(),
            );
        }

        if count > 0 || sel_count > 0 {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if sel_count > 0 {
                    if ui.button(format!("Export {sel_count} Selected")).clicked() {
                        app.export_selected();
                    }
                    if ui.button("Clear Selection").clicked() {
                        app.multi_selected.clear();
                    }
                }
                if count > 0 && ui.button("Select All").clicked() {
                    for r in &app.search_results {
                        app.multi_selected.insert(r.hash);
                    }
                }
            });
        }
    });
    ui.separator();

    if app.search_results.is_empty() {
        if app.handler.is_some() {
            ui.label("No results. Type a filename and press Search.");
        } else {
            ui.label("Open a game directory to start.");
        }
        return None;
    }

    let mut clicked: Option<SearchResult> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        let results: Vec<SearchResult> = app.search_results.clone();
        let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);

        for result in results {
            let is_multi = app.multi_selected.contains(&result.hash);
            let is_preview = app
                .selected
                .as_ref()
                .map(|s| s.result.hash == result.hash)
                .unwrap_or(false);

            let display_name = result
                .filename
                .as_deref()
                .unwrap_or("(unknown)")
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("(unknown)");

            let path_str = result.filename.as_deref().unwrap_or("");

            ui.push_id(result.hash, |ui| {
                let mut label_clicked = false;
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(4.0, 2.0))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        label_clicked = ui
                            .selectable_label(is_multi || is_preview, display_name)
                            .clicked();
                    });

                if label_clicked {
                    if ctrl_held {
                        if is_multi {
                            app.multi_selected.remove(&result.hash);
                        } else {
                            app.multi_selected.insert(result.hash);
                        }
                    } else {
                        app.multi_selected.clear();
                        app.multi_selected.insert(result.hash);
                        clicked = Some(result.clone());
                    }
                }

                if !path_str.is_empty() {
                    // Full path rendered as technical (mono + rune-blue)
                    // per the design system — this is file-identity data.
                    ui.label(
                        egui::RichText::new(format!("  {path_str}"))
                            .small()
                            .monospace()
                            .color(rd::RUNE_400),
                    );
                }
            });

            ui.separator();
        }
    });

    clicked
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
