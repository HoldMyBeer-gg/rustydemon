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
    let typing = ui.ctx().egui_wants_keyboard_input();
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

    match folder_bar(ui, &folder_path, sel_count) {
        Some(FolderAction::ExportSelected) => app.export_selected(),
        Some(FolderAction::ExportFolder) => app.export_folder(),
        Some(FolderAction::SelectAll) => {
            for file in &files {
                app.multi_selected.insert(file.hash);
            }
        }
        Some(FolderAction::ClearSelection) => app.multi_selected.clear(),
        None => {}
    }
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
                "FDID {} · {} · -",
                file.file_data_id
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "-".into()),
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

/// A button pressed in the folder view's second row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderAction {
    ExportSelected,
    ExportFolder,
    SelectAll,
    ClearSelection,
}

/// Space left of the folder path.
const LEFT_PAD: f32 = 4.0;

/// Folder path and export controls, on the row under the panel header.
fn folder_bar(ui: &mut egui::Ui, folder_path: &str, sel_count: usize) -> Option<FolderAction> {
    let mut action = None;
    let shown = if folder_path.is_empty() {
        "/"
    } else {
        folder_path
    };
    // Right to left, as drawn.
    let mut buttons: Vec<(String, FolderAction)> = Vec::new();
    if sel_count > 0 {
        buttons.push((
            format!("Export {sel_count} Selected"),
            FolderAction::ExportSelected,
        ));
    }
    buttons.push(("Export Folder".into(), FolderAction::ExportFolder));
    buttons.push(("Select All".into(), FolderAction::SelectAll));
    if sel_count > 0 {
        buttons.push(("Clear Selection".into(), FolderAction::ClearSelection));
    }

    // Is there room beside the buttons for at least the current folder?
    let wide = |text: &str, style: egui::TextStyle| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), style.resolve(ui.style()), rd::RUNE_400)
            .size()
            .x
    };
    let spacing = ui.spacing().item_spacing.x;
    let buttons_width: f32 = buttons
        .iter()
        .map(|(label, _)| {
            wide(label, egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.x + spacing
        })
        .sum();
    let least = cut_front(shown, |_| false);
    let least_width =
        wide(&least, egui::TextStyle::Monospace).min(wide(shown, egui::TextStyle::Monospace));
    let beside = ui.available_width() - LEFT_PAD - buttons_width >= least_width;

    let path = |ui: &mut egui::Ui| {
        let (galley, cut) = path_text(ui, shown, ui.available_width());
        let (rect, resp) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
        ui.painter().galley(rect.min, galley, rd::RUNE_400);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, shown));
        if cut {
            resp.on_hover_text(shown);
        }
    };

    // A path too long for its space loses its leading folders; it shows
    // whole on hover and its accessible name is always the whole path.
    // When the buttons leave no room even for the current folder, the path
    // goes on a row of its own above them.
    if !beside {
        ui.horizontal(|ui| {
            ui.add_space(LEFT_PAD);
            path(ui);
        });
    }
    ui.horizontal(|ui| {
        ui.add_space(LEFT_PAD);
        // Buttons first, from the right, so the path gets what is left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (label, what) in &buttons {
                if ui.button(label).clicked() {
                    action = Some(*what);
                }
            }
            if beside {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), path);
            }
        });
    });
    action
}

/// `path` with leading folders dropped until `fits` accepts it, marked with
/// a leading `…/`.  The last folder is where the user is, so it is the part
/// kept; if even `…/last` does not fit, that is returned for the caller to
/// cut at the end.
fn cut_front(path: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(path) {
        return path.to_owned();
    }
    let mut rest = path;
    while let Some((_, tail)) = rest.split_once('/') {
        rest = tail;
        let shown = format!("…/{rest}");
        if fits(&shown) || !rest.contains('/') {
            return shown;
        }
    }
    path.to_owned()
}

/// The folder path laid out for a row `width` wide, and whether any of it
/// had to be left out.
fn path_text(ui: &egui::Ui, path: &str, width: f32) -> (std::sync::Arc<egui::Galley>, bool) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let wide = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), rd::RUNE_400)
            .size()
            .x
    };
    let shown = cut_front(path, |text| wide(text) <= width);
    let cut = shown != path;
    let mut job = egui::text::LayoutJob::simple_singleline(shown, font.clone(), rd::RUNE_400);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    let galley = ui.painter().layout_job(job);
    let cut = cut || galley.elided;
    (galley, cut)
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
                "FDID {} · {} · -",
                result
                    .file_data_id
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| "-".into()),
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
    // Hand-painted row: describe it so it reaches the accessibility tree.
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, is_sel, name));
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
        egui::Stroke::new(
            1.0_f32,
            egui::Color32::from_rgba_premultiplied(29, 50, 71, 120),
        ),
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

pub(crate) fn file_icon(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".blp") || lower.ends_with(".tex") {
        "🖼"
    } else if lower.ends_with(".m2") || lower.ends_with(".mdx") {
        "📦"
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

#[cfg(test)]
mod a11y_tests {
    use super::*;

    #[test]
    fn file_row_is_published_with_its_name() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let out = crate::ui::a11y::testing::frame(&ctx, |ui| {
            draw_file_row(ui, "core.lua", "FDID 1 · abc · -", "📄", false);
        });
        let names: Vec<String> = out
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_owned))
            .collect();
        assert!(names.iter().any(|n| n == "core.lua"), "{names:?}");
    }
}

#[cfg(test)]
mod layout_tests {
    use crate::ui::a11y::testing::{bounds_of, nodes_sized, overlap};

    const LONG: &str = "interface/addons/blizzard_sharedxml/shared/frames/templates/mainline";

    /// A path too long for the row is cut short, not drawn under the buttons,
    /// and is still published whole.
    #[test]
    fn long_folder_path_is_not_drawn_under_the_buttons() {
        let all = nodes_sized([520.0, 300.0], |ui| {
            super::folder_bar(ui, LONG, 3);
        });
        let path = bounds_of(&all, LONG);
        for button in [
            "Clear Selection",
            "Select All",
            "Export Folder",
            "Export 3 Selected",
        ] {
            let b = bounds_of(&all, button);
            assert!(!overlap(path, b), "{button} {b:?} covers {path:?}");
        }
    }

    fn fits_in(n: usize) -> impl Fn(&str) -> bool {
        move |s: &str| s.chars().count() <= n
    }

    #[test]
    fn a_path_that_fits_is_not_cut() {
        assert_eq!(
            super::cut_front("interface/icons", fits_in(40)),
            "interface/icons"
        );
    }

    #[test]
    fn leading_folders_go_first_and_the_last_folder_stays() {
        assert_eq!(
            super::cut_front("interface/addons/blizzard_gamepad", fits_in(25)),
            "…/addons/blizzard_gamepad"
        );
        assert_eq!(
            super::cut_front("interface/addons/blizzard_gamepad", fits_in(20)),
            "…/blizzard_gamepad"
        );
    }

    #[test]
    fn a_last_folder_too_long_on_its_own_is_still_what_is_kept() {
        assert_eq!(
            super::cut_front("interface/addons/blizzard_gamepad", fits_in(5)),
            "…/blizzard_gamepad"
        );
        assert_eq!(
            super::cut_front("blizzard_gamepad", fits_in(5)),
            "blizzard_gamepad"
        );
    }

    /// In a narrow row the drawn text still ends with the current folder.
    #[test]
    fn narrow_row_keeps_the_end_of_the_path_on_screen() {
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let mut drawn = String::new();
        crate::ui::a11y::testing::frame_plain(&ctx, |ui| {
            drawn = super::path_text(ui, LONG, 160.0).0.text().to_owned();
        });
        assert!(drawn.starts_with('…'), "{drawn}");
        assert!(drawn.ends_with("mainline"), "{drawn}");
    }

    /// With four buttons in a narrow panel nothing is left beside them.
    /// The path then takes a row of its own; it must not shrink to a bare
    /// ellipsis squeezed against the first button.
    #[test]
    fn path_gets_its_own_row_when_the_buttons_leave_no_room() {
        let all = nodes_sized([430.0, 300.0], |ui| {
            super::folder_bar(ui, "creature/abyssalillidan", 1);
        });
        let path = bounds_of(&all, "creature/abyssalillidan");
        assert!(path.x1 - path.x0 > 80.0, "path drawn {path:?}");
        let button = bounds_of(&all, "Clear Selection");
        assert!(path.y1 <= button.y0, "path {path:?} not above {button:?}");
    }

    #[test]
    fn path_stays_beside_the_buttons_when_it_fits() {
        let all = nodes_sized([900.0, 300.0], |ui| {
            super::folder_bar(ui, "creature/abyssalillidan", 1);
        });
        let path = bounds_of(&all, "creature/abyssalillidan");
        let button = bounds_of(&all, "Clear Selection");
        assert!(
            path.y0 < button.y1 && button.y0 < path.y1,
            "{path:?} {button:?}"
        );
    }

    #[test]
    fn short_folder_path_is_shown_as_it_is() {
        let all = nodes_sized([900.0, 300.0], |ui| {
            super::folder_bar(ui, "interface/icons", 0);
        });
        bounds_of(&all, "interface/icons");
    }
}
