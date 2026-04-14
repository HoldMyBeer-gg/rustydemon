use crate::app::{detect_game_installs, CascExplorerApp};
use egui::Context;

pub fn draw_menu(ctx: &Context, app: &mut CascExplorerApp) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            // ── File ──────────────────────────────────────────────────────────
            ui.menu_button("File", |ui| {
                // Auto-detected game installs.
                let installs = detect_game_installs();
                if !installs.is_empty() {
                    ui.menu_button("Open Game", |ui| {
                        for (name, path) in &installs {
                            if ui.button(name).clicked() {
                                ui.close_menu();
                                app.open_game_dir(path.clone());
                            }
                        }
                    });
                }

                if ui.button("Open Game Directory…").clicked() {
                    ui.close_menu();
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
                        "Not needed — this game provides its own file paths.",
                    )
                } else {
                    listfile_btn
                };
                if listfile_btn.clicked() {
                    ui.close_menu();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Listfile", &["csv", "txt"])
                        .pick_file()
                    {
                        app.load_listfile(path);
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
                    ui.close_menu();
                    app.export_selected();
                }

                if ui
                    .add_enabled(
                        app.browsed_folder.is_some(),
                        egui::Button::new("Export Current Folder…"),
                    )
                    .clicked()
                {
                    ui.close_menu();
                    app.export_folder();
                }

                ui.separator();

                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            // ── Edit ──────────────────────────────────────────────────────────
            ui.menu_button("Edit", |ui| {
                if ui.button("Copy Hash").clicked() {
                    ui.close_menu();
                    if let Some(sel) = &app.selected {
                        ctx.copy_text(format!("{:016X}", sel.result.hash));
                    }
                }
                if ui.button("Copy CKey").clicked() {
                    ui.close_menu();
                    if let Some(sel) = &app.selected {
                        ctx.copy_text(sel.result.ckey.to_hex());
                    }
                }
            });

            // ── View ──────────────────────────────────────────────────────────
            ui.menu_button("View", |ui| {
                if ui.button("Expand All").clicked() {
                    ui.close_menu();
                    if let Some(handler) = &app.handler {
                        if let Some(root) = &handler.root_folder {
                            collect_folder_paths(root, "", &mut app.expanded);
                        }
                    }
                    app.expansion_dirty = true;
                }
                if ui.button("Collapse All").clicked() {
                    ui.close_menu();
                    app.expanded.clear();
                    app.expansion_dirty = true;
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
                            ui.close_menu();
                        }
                    }
                });

                let mut validate = app
                    .handler
                    .as_ref()
                    .map(|h| h.validate_hashes)
                    .unwrap_or(false);
                if ui.checkbox(&mut validate, "Validate Hashes").clicked() {
                    if let Some(h) = &mut app.handler {
                        h.validate_hashes = validate;
                    }
                }
            });

            // ── Help ──────────────────────────────────────────────────────────
            ui.menu_button("Help", |ui| {
                if ui.button("About").clicked() {
                    ui.close_menu();
                    app.status = format!(
                        "rustydemon v{}  — CASC explorer  |  rustydemon-lib v{}",
                        env!("CARGO_PKG_VERSION"),
                        env!("CARGO_PKG_VERSION"),
                    );
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
