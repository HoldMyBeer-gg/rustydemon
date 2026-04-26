use crate::app::CascExplorerApp;
use crate::ui::theme::{self, rd};
use rustydemon_lib::SearchResult;

/// Draw the center panel — shows either search results or browsed folder contents.
/// Returns the clicked [`SearchResult`], if any.
pub fn draw_results(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<SearchResult> {
    // Handle Ctrl+A to select all visible files — but only if no text
    // widget is focused, so typing Ctrl+A in the search box still does
    // "select all text" inside that field instead of double-firing into
    // a select-all-results.
    let typing = ui.ctx().wants_keyboard_input();
    if !typing && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
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
    let mut sub_names: Vec<String> = folder.folders.keys().cloned().collect();
    sub_names.sort_unstable();

    let total_files = folder.files.len();
    const MAX_FILES: usize = 2000;
    let mut files: Vec<rustydemon_lib::CascFile> = if total_files > MAX_FILES {
        folder.files.values().take(MAX_FILES).cloned().collect()
    } else {
        folder.files.values().cloned().collect()
    };
    files.sort_unstable_by(|a, b| a.name.cmp(&b.name));

    let total = sub_names.len() + total_files;
    let sel_count = app.multi_selected.len();

    theme::panel_header(ui, "Folder", Some(&format!("{total} hits")));

    // Folder path + export controls on a second row.
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(if folder_path.is_empty() {
                "/"
            } else {
                folder_path.as_str()
            })
            .monospace()
            .color(rd::RUNE_400),
        );
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
    ui.add_space(2.0);

    let mut clicked: Option<SearchResult> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        let ctrl_held = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);

        // Subfolders (single click navigates).
        for name in &sub_names {
            let child_path = if folder_path.is_empty() {
                name.clone()
            } else {
                format!("{folder_path}/{name}")
            };
            let resp = draw_file_row(ui, name, "(folder)", "📁", false);
            if resp.clicked() {
                app.browsed_folder = Some(child_path);
            }
        }

        // Files.
        let truncated = total_files > files.len();
        for file in &files {
            let is_multi = app.multi_selected.contains(&file.hash);
            let is_preview = app
                .selected
                .as_ref()
                .map(|s| s.result.hash == file.hash)
                .unwrap_or(false);

            let icon = file_icon(&file.name);
            let meta = format!(
                "FDID {} · {} · —",
                file.file_data_id
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "—".into()),
                &format!("{:016x}", file.hash)[..12],
            );

            let resp = ui.push_id(file.hash, |ui| {
                draw_file_row(ui, &file.name, &meta, icon, is_multi || is_preview)
            });
            let resp = resp.inner;

            if resp.clicked() {
                if ctrl_held {
                    if is_multi {
                        app.multi_selected.remove(&file.hash);
                    } else {
                        app.multi_selected.insert(file.hash);
                    }
                } else {
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

    theme::panel_header(ui, "Search Results", Some(&format!("{count} hits")));

    if count > 0 || sel_count > 0 {
        ui.horizontal(|ui| {
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
        });
        ui.add_space(2.0);
    }

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
                .unwrap_or("(unknown)")
                .to_string();

            let icon = file_icon(&display_name);
            let meta = format!(
                "FDID {} · {} · —",
                result
                    .file_data_id
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "—".into()),
                &format!("{:016x}", result.hash)[..12],
            );

            let resp = ui.push_id(result.hash, |ui| {
                draw_file_row(ui, &display_name, &meta, icon, is_multi || is_preview)
            });
            let resp = resp.inner;

            if resp.clicked() {
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
        }
    });

    clicked
}

/// Two-line file row: bold filename + mono metadata line.
/// Selection paints the ember fill + 3px left stripe per the design system.
fn draw_file_row(
    ui: &mut egui::Ui,
    name: &str,
    meta: &str,
    icon: &str,
    is_sel: bool,
) -> egui::Response {
    let row_h = 40.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_h),
        egui::Sense::click(),
    );
    let painter = ui.painter();

    if is_sel {
        painter.rect_filled(rect, 0.0, theme::selected_row_fill());
        theme::row_accent_stripe(ui, rect);
    } else if resp.hovered() {
        painter.rect_filled(
            rect,
            0.0,
            egui::Color32::from_rgba_premultiplied(29, 142, 232, 20),
        );
    }
    // Bottom hairline.
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(29, 50, 71, 120)),
    );

    let x_icon = rect.left() + 12.0;
    let x_text = x_icon + 22.0;
    painter.text(
        egui::pos2(x_icon, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(14.0),
        rd::RUNE_400,
    );
    painter.text(
        egui::pos2(x_text, rect.top() + 10.0),
        egui::Align2::LEFT_TOP,
        name,
        egui::FontId::proportional(13.0),
        rd::FG_PRIMARY,
    );
    painter.text(
        egui::pos2(x_text, rect.top() + 24.0),
        egui::Align2::LEFT_TOP,
        meta,
        egui::FontId::monospace(10.5),
        rd::FG_MUTED,
    );
    resp
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
