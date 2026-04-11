use rustydemon_lib::SearchResult;
use crate::app::CascExplorerApp;

/// Draw the center search-results panel.
/// Returns the clicked [`SearchResult`], if any.
pub fn draw_results(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<SearchResult> {
    let count = app.search_results.len();
    let header = if count > 0 {
        format!("Search Results ({count})")
    } else {
        "Search Results".to_string()
    };

    ui.heading(&header);
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
        // Clone the results to avoid borrow conflicts.
        let results: Vec<SearchResult> = app.search_results.clone();
        let selected_hash = app.selected.as_ref().map(|s| s.result.hash);

        for result in results {
            let is_selected = selected_hash == Some(result.hash);
            let display_name = result
                .filename
                .as_deref()
                .unwrap_or("(unknown)")
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("(unknown)");

            let path_str = result.filename.as_deref().unwrap_or("");

            ui.push_id(result.hash, |ui| {
                let resp = egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(4.0, 2.0))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        let _ = ui.selectable_label(is_selected, display_name);
                    });

                if resp.response.clicked() {
                    clicked = Some(result.clone());
                }

                // Show the path as a dimmed sub-label.
                if !path_str.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("  Path: {path_str}"))
                            .small()
                            .color(egui::Color32::from_gray(130)),
                    );
                } else if let Some(id) = result.file_data_id {
                    ui.label(
                        egui::RichText::new(format!("  FileDataId: {id}"))
                            .small()
                            .color(egui::Color32::from_gray(130)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!("  {:016X}", result.hash))
                            .small()
                            .color(egui::Color32::from_gray(130)),
                    );
                }
            });

            ui.separator();
        }
    });

    clicked
}
