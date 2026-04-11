mod menu;
mod preview;
mod results;
mod tree;

use crate::app::CascExplorerApp;
use egui::Context;

/// Draw the entire application UI for one frame.
pub fn draw(ctx: &Context, app: &mut CascExplorerApp) {
    ctx.set_visuals(egui::Visuals::dark());

    menu::draw_menu(ctx, app);
    draw_status_bar(ctx, app);
    draw_panels(ctx, app);
}

fn draw_panels(ctx: &Context, app: &mut CascExplorerApp) {
    egui::CentralPanel::default().show(ctx, |ui| {
        // Toolbar row.
        ui.horizontal(|ui| {
            toolbar(ui, app);
        });
        ui.separator();

        // Collect tree click before split so borrows don't overlap.
        let mut tree_click: Option<u64> = None;
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

        // Process tree click — look up the hash in the root.
        if let Some(hash) = tree_click {
            let entries = app
                .handler
                .as_ref()
                .map(|h| h.search_by_hash(hash))
                .unwrap_or_default();
            if let Some(first) = entries.into_iter().next() {
                let ctx2 = ctx.clone();
                app.select_result(first, &ctx2);
            }
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

    if ui.button("Search").clicked() {
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
            ui.label(&app.status);
        });
    });
}
