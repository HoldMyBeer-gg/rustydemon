mod app;
mod deep_search;
mod ui;

use app::CascExplorerApp;

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rusty Demon — CASC Explorer")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(eframe::icon_data::from_png_bytes(&[]).unwrap_or_default()),
        ..Default::default()
    };

    eframe::run_native(
        "Rusty Demon — CASC Explorer",
        native_options,
        Box::new(|cc| Ok(Box::new(CascExplorerApp::new(cc)))),
    )
}
