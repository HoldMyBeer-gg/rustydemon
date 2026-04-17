mod app;
mod audio;
mod deep_search;
pub mod pow_preview;
mod preview;
pub mod tex_preview;
pub mod text_preview;
mod ui;
pub mod vid_preview;
mod viewport3d;

use std::path::PathBuf;

use app::CascExplorerApp;

fn main() -> eframe::Result {
    // Optional CLI argument: path to a game directory to open on launch.
    // Supports `rustydemon /path/to/game` or `rustydemon .` for cwd.
    let open_path: Option<PathBuf> =
        std::env::args()
            .nth(1)
            .filter(|a| !a.starts_with('-'))
            .map(|a| {
                let p = PathBuf::from(&a);
                // Resolve `.` and relative paths to absolute.
                std::fs::canonicalize(&p).unwrap_or(p)
            });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rusty Demon — CASC Explorer")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../icon.png"))
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };

    eframe::run_native(
        "Rusty Demon — CASC Explorer",
        native_options,
        Box::new(move |cc| {
            let mut app = CascExplorerApp::new(cc);
            if let Some(path) = open_path {
                app.open_game_dir(path);
            }
            Ok(Box::new(app))
        }),
    )
}
