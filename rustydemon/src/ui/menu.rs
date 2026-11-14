use std::sync::Arc;

use crate::app::CascExplorerApp;

/// Ctrl+O on Linux/Windows, ⌘+O on macOS: egui's `COMMAND` modifier maps
/// to whichever the host platform uses.
const OPEN_SHORTCUT: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O);

/// File → Open Game Directory…, with its shortcut drawn on the right.
///
/// egui publishes the shortcut as part of the button's name ("Open Game
/// Directory… Ctrl+O"). The name is set back to the label alone, and the
/// shortcut is published where assistive technology looks for one.
fn open_dir_button(ui: &mut egui::Ui) -> egui::Response {
    const LABEL: &str = "Open Game Directory…";
    let shortcut = ui.ctx().format_shortcut(&OPEN_SHORTCUT);
    let resp = ui.add(egui::Button::new(LABEL).shortcut_text(shortcut.clone()));
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_label(LABEL);
        node.set_keyboard_shortcut(shortcut);
    });
    resp
}

/// What an Edit menu item asks to be copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Copy {
    Hash,
    CKey,
}

/// The Edit menu's items. Both copy from the selected file.
fn edit_items(ui: &mut egui::Ui, has_selection: bool) -> Option<Copy> {
    let mut item = |label: &str| {
        ui.add_enabled(has_selection, egui::Button::new(label))
            .clicked()
    };
    if item("Copy Hash") {
        return Some(Copy::Hash);
    }
    if item("Copy CKey") {
        return Some(Copy::CKey);
    }
    None
}

pub fn draw_menu(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    let ctx = &ui.ctx().clone();
    // Global Ctrl/⌘+O: pop the same folder picker as File → Open Game Directory.
    // Guarded against text-field focus so typing the letter in the search bar
    // doesn't trigger a dialog.
    let typing = ctx.egui_wants_keyboard_input();
    if !typing && ctx.input_mut(|i| i.consume_shortcut(&OPEN_SHORTCUT)) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            app.open_game_dir(path);
        }
    }

    egui::Panel::top("menu_bar")
        .frame(
            egui::Frame::NONE
                .fill(crate::ui::theme::rd::FROST_150)
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    crate::ui::theme::rd::BORDER_SUBTLE,
                ))
                .inner_margin(egui::Margin::symmetric(12, 4)),
        )
        .show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // ── File ──────────────────────────────────────────────────────────
                ui.menu_button("File", |ui| {
                    // Auto-detected game installs.
                    let repaint = ui.ctx().clone();
                    let search = app
                        .installs
                        .get_or_insert_with(|| {
                            crate::installs::Search::start(crate::installs::detect, move || {
                                repaint.request_repaint()
                            })
                        })
                        .clone();
                    match search.found() {
                        None => {
                            ui.add_enabled(false, egui::Button::new("Looking for games…"));
                        }
                        Some([]) => {}
                        Some(installs) => {
                            ui.menu_button("Open Game", |ui| {
                                for (name, path) in installs {
                                    if ui.button(name).clicked() {
                                        ui.close();
                                        app.open_game_dir(path.clone());
                                    }
                                }
                            });
                        }
                    }

                    if open_dir_button(ui).clicked() {
                        ui.close();
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            app.open_game_dir(path);
                        }
                    }

                    let has_builtin = app
                        .handler
                        .as_ref()
                        .map(|h| h.has_builtin_paths())
                        .unwrap_or(false);
                    let listfile_enabled = app.handler.is_some() && !has_builtin;
                    let listfile_btn =
                        ui.add_enabled(listfile_enabled, egui::Button::new("Load Listfile…"));
                    let listfile_btn = if has_builtin {
                        listfile_btn.on_disabled_hover_text(
                            "Not needed: this game provides its own file paths.",
                        )
                    } else {
                        listfile_btn
                    };
                    if listfile_btn.clicked() {
                        ui.close();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Listfile", &["csv", "txt"])
                            .pick_file()
                        {
                            app.load_listfile(path);
                        }
                    }

                    // Always enabled: keys apply to the shared runtime table,
                    // so they can be loaded before or after opening an archive.
                    let keys_btn = ui.button("Load TACT Keys…").on_hover_text(
                        rustydemon_lib::key_service::user_key_file()
                            .map(|p| {
                                format!(
                                    "Decryption keys for encrypted files.\n\
                                     Loaded automatically at startup from:\n{}",
                                    p.display()
                                )
                            })
                            .unwrap_or_else(|| "Decryption keys for encrypted files.".into()),
                    );
                    if keys_btn.clicked() {
                        ui.close();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("TACT keys", &["keys", "txt"])
                            .pick_file()
                        {
                            app.load_tact_keys(path);
                        }
                    }

                    ui.separator();

                    let sel_count = app.multi_selected.len();
                    let has_selection = sel_count > 0;

                    if ui
                        .add_enabled(
                            has_selection,
                            egui::Button::new(format!("Export {sel_count} Selected…")),
                        )
                        .clicked()
                    {
                        ui.close();
                        app.export_selected();
                    }

                    if ui
                        .add_enabled(
                            app.browsed_folder.is_some(),
                            egui::Button::new("Export Current Folder…"),
                        )
                        .clicked()
                    {
                        ui.close();
                        app.export_folder();
                    }

                    ui.separator();

                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                // ── Edit ──────────────────────────────────────────────────────────
                ui.menu_button("Edit", |ui| {
                    let copy = edit_items(ui, app.selected.is_some());
                    if let (Some(copy), Some(sel)) = (copy, &app.selected) {
                        ui.close();
                        ctx.copy_text(match copy {
                            Copy::Hash => format!("{:016X}", sel.result.hash),
                            Copy::CKey => sel.result.ckey.to_hex(),
                        });
                    }
                });

                // ── View ──────────────────────────────────────────────────────────
                ui.menu_button("View", |ui| {
                    if ui.button("Expand All").clicked() {
                        ui.close();
                        if let Some(handler) = &app.handler {
                            if let Some(root) = &handler.root_folder {
                                collect_folder_paths(root, "", &mut app.expanded);
                            }
                        }
                        app.expansion_dirty = true;
                    }
                    if ui.button("Collapse All").clicked() {
                        ui.close();
                        app.expanded.clear();
                        app.expansion_dirty = true;
                    }
                    ui.separator();
                    ui.checkbox(&mut app.viewport3d_open, "3D Test Viewport");
                    let mut dys = app.dyslexia_friendly;
                    if ui.checkbox(&mut dys, "Dyslexia-friendly type").changed() {
                        app.dyslexia_friendly = dys;
                        crate::ui::theme::set_fonts(ctx, dys);
                    }
                });

                // ── Tools ─────────────────────────────────────────────────────────
                ui.menu_button("Tools", |ui| {
                    // Show detected products first, then a fixed fallback list.
                    let detected = app.detected_products.clone();
                    let fallback = [
                        "fenris",
                        "wow",
                        "wow_classic",
                        "hs",
                        "s2",
                        "hero",
                        "pro",
                        "w3",
                        "osi",
                        "d3",
                    ];
                    let products: Vec<&str> = if detected.is_empty() {
                        fallback.to_vec()
                    } else {
                        detected.iter().map(|s| s.as_str()).collect()
                    };

                    ui.menu_button("Product", |ui| {
                        for product in &products {
                            if ui
                                .selectable_label(&app.product == product, *product)
                                .clicked()
                            {
                                app.product = product.to_string();
                                ui.close();
                            }
                        }
                    });

                    let mut validate = app
                        .handler
                        .as_ref()
                        .map(|h| h.validate_hashes)
                        .unwrap_or(false);
                    if ui.checkbox(&mut validate, "Validate Hashes").clicked() {
                        if let Some(h) = app.handler.as_mut().and_then(Arc::get_mut) {
                            h.validate_hashes = validate;
                        }
                    }
                });

                // ── Help ──────────────────────────────────────────────────────────
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        ui.close();
                        app.about_open = true;
                    }
                });
            });
        });
}

fn collect_folder_paths(
    folder: &rustydemon_lib::CascFolder,
    prefix: &str,
    out: &mut std::collections::HashSet<String>,
) {
    for (name, sub) in &folder.folders {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        out.insert(path.clone());
        collect_folder_paths(sub, &path, out);
    }
}

#[cfg(test)]
mod a11y_tests {
    use crate::ui::a11y::testing::nodes;
    use egui::accesskit::Role;

    /// The shortcut is drawn beside the label, but it is not part of the
    /// item's name: a screen reader should say "Open Game Directory" and
    /// offer the shortcut separately.
    #[test]
    fn open_item_name_does_not_include_its_shortcut() {
        let all = nodes(|ui| {
            super::open_dir_button(ui);
        });
        let item = all
            .iter()
            .find(|n| n.role() == Role::Button)
            .expect("the menu item");
        assert_eq!(item.label(), Some("Open Game Directory…"));
        let shortcut = item.keyboard_shortcut().expect("a published shortcut");
        assert!(shortcut.ends_with('O'), "{shortcut}");
    }

    fn edit_item_disabled(has_selection: bool, name: &str) -> bool {
        nodes(|ui| {
            super::edit_items(ui, has_selection);
        })
        .iter()
        .find(|n| n.label() == Some(name))
        .unwrap_or_else(|| panic!("no {name}"))
        .is_disabled()
    }

    #[test]
    fn copy_items_are_disabled_with_nothing_selected() {
        assert!(edit_item_disabled(false, "Copy Hash"));
        assert!(edit_item_disabled(false, "Copy CKey"));
    }

    #[test]
    fn copy_items_are_enabled_with_a_file_selected() {
        assert!(!edit_item_disabled(true, "Copy Hash"));
        assert!(!edit_item_disabled(true, "Copy CKey"));
    }
}
