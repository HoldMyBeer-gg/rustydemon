mod menu;
pub(crate) mod preview;
mod results;
pub(crate) mod theme;
mod tree;

use crate::app::CascExplorerApp;
use egui::Context;
use rustydemon_lib::SearchResult;

/// Draw the entire application UI for one frame.
pub fn draw(ctx: &Context, app: &mut CascExplorerApp) {
    menu::draw_menu(ctx, app);
    draw_status_bar(ctx, app);
    draw_panels(ctx, app);
}

fn draw_panels(ctx: &Context, app: &mut CascExplorerApp) {
    let central_frame = egui::Frame::central_panel(&ctx.style()).fill(theme::rd::BG_APP);
    egui::CentralPanel::default()
        .frame(central_frame)
        .show(ctx, |ui| {
            // Forge-glow vignette: a warm radial in the top-left + a cool
            // one in the bottom-right, painted under the toolbar so it
            // reads as "something is hot off-screen", not a full gradient
            // wash.  Matches `body.rd-forge-bg` in the design system.
            paint_forge_glow(ui);

            // Toolbar row.
            ui.horizontal(|ui| {
                toolbar(ui, app);
                // Zoom buttons pinned to the far right.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("A+")
                        .on_hover_text("Increase font size")
                        .clicked()
                    {
                        let z = (ctx.zoom_factor() + 0.1).min(3.0);
                        ctx.set_zoom_factor(z);
                    }
                    if ui
                        .button("A-")
                        .on_hover_text("Decrease font size")
                        .clicked()
                    {
                        let z = (ctx.zoom_factor() - 0.1).max(0.5);
                        ctx.set_zoom_factor(z);
                    }
                });
            });
            ui.separator();

        // Collect tree click before split so borrows don't overlap.
        let mut tree_click: Option<tree::TreeClick> = None;
        let mut result_click: Option<rustydemon_lib::SearchResult> = None;

        egui::SidePanel::left("tree_panel")
            .resizable(true)
            .default_width(220.0)
            .min_width(140.0)
            .show_inside(ui, |ui| {
                tree_click = tree::draw_tree(ui, app);
            });

        egui::SidePanel::right("preview_panel")
            .resizable(true)
            .default_width(240.0)
            .min_width(160.0)
            .show_inside(ui, |ui| {
                preview::draw_preview(ui, app);
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            result_click = results::draw_results(ui, app);
        });

        // Process tree clicks.
        match tree_click {
            Some(tree::TreeClick::File(hash)) => {
                let handler = app.handler.as_ref();
                let entries = handler.map(|h| h.search_by_hash(hash)).unwrap_or_default();
                if let Some(first) = entries.into_iter().next() {
                    let ctx2 = ctx.clone();
                    app.select_result(first, &ctx2);
                } else {
                    // File is in the tree but has no root entry (no CKey to load).
                    let filename = handler.and_then(|h| h.filename_for_hash(hash));
                    let mut sel = crate::app::SelectedFile::new(SearchResult {
                        hash,
                        filename,
                        file_data_id: None,
                        locale: rustydemon_lib::LocaleFlags::ALL,
                        content: rustydemon_lib::ContentFlags::NONE,
                        ckey: rustydemon_lib::Md5Hash::default(),
                    });
                    sel.load_error =
                        Some("No root entry found for this file (CKey unavailable).".into());
                    app.selected = Some(sel);
                }
            }
            Some(tree::TreeClick::Folder(path)) => {
                app.browsed_folder = Some(path);
                app.search_results.clear();
            }
            None => {}
        }

        // Process results panel click.
        if let Some(res) = result_click {
            let ctx2 = ctx.clone();
            app.select_result(res, &ctx2);
        }
    });
}

fn toolbar(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    let response = ui.add(
        egui::TextEdit::singleline(&mut app.search_text)
            .hint_text("Search files…")
            .desired_width(300.0),
    );
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        app.run_search();
    }

    // Primary action — ember fill, forge-colored text.
    let search_btn = egui::Button::new(
        egui::RichText::new("Search").color(theme::rd::FG_ON_EMBER).strong(),
    )
    .fill(theme::rd::EMBER_500);
    if ui.add(search_btn).clicked() {
        app.run_search();
    }

    ui.separator();

    ui.checkbox(&mut app.deep_search_enabled, "Deep search");

    if ui.button("🔍 Find All (Deep Search)").clicked() {
        app.run_deep_search();
    }
}

fn draw_status_bar(ctx: &Context, app: &CascExplorerApp) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            if app.loading {
                ui.spinner();
            }
            // Split the status string into an ember-tinted leading verb
            // (if any) and the rest in muted body color, so live states
            // ("Opening…", "Loading…", "Searching…") read as hot while
            // idle status stays calm.
            let (accent, rest) = split_status_accent(&app.status);
            if let Some(a) = accent {
                ui.label(
                    egui::RichText::new(a)
                        .color(theme::rd::EMBER_600)
                        .strong(),
                );
            }
            ui.label(egui::RichText::new(rest).color(theme::rd::FG_SECONDARY));
        });
    });
}

/// Split a status string into an optional hot prefix (one of a short
/// allowlist of live-action verbs) and the rest, for two-tone rendering.
fn split_status_accent(status: &str) -> (Option<&str>, &str) {
    for prefix in [
        "Opening", "Loading", "Searching", "Deep searching", "Exporting", "Playing",
    ] {
        if let Some(rest) = status.strip_prefix(prefix) {
            return (Some(prefix), rest);
        }
    }
    (None, status)
}

/// Paint the forge-glow vignette beneath all panels — warm ember in
/// the top-left, cool rune in the bottom-right, both at very low alpha
/// so nothing competes with actual UI content.
fn paint_forge_glow(ui: &egui::Ui) {
    let rect = ui.ctx().screen_rect();
    let painter = ui.ctx().layer_painter(egui::LayerId::background());

    // Warm ember — top-left quadrant.
    let c1 = rect.min + egui::vec2(rect.width() * 0.15, -rect.height() * 0.10);
    painter.circle_filled(
        c1,
        rect.height() * 0.70,
        egui::Color32::from_rgba_premultiplied(0x1c, 0x0b, 0x02, 0x14),
    );

    // Cool rune — bottom-right quadrant.
    let c2 = rect.min + egui::vec2(rect.width() * 0.90, rect.height() * 1.10);
    painter.circle_filled(
        c2,
        rect.height() * 0.65,
        egui::Color32::from_rgba_premultiplied(0x04, 0x0a, 0x14, 0x14),
    );
}
