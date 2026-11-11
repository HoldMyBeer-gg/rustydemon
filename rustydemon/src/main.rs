// On Windows, the MSVC linker defaults to the "console" subsystem, which
// makes the OS allocate a console window on launch (the stray command
// window). Switch to the GUI subsystem for release builds. Debug builds
// keep the console so stderr / eprintln diagnostics stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod deep_search;
mod launch_args;
pub mod pow_preview;
mod preview;
pub mod tex_preview;
pub mod text_preview;
mod ui;
pub mod vid_preview;
mod viewport3d;

use app::CascExplorerApp;

fn main() -> eframe::Result {
    // `rustydemon [GAME_DIR] [--listfile FILE]`, both optional.  Relative
    // paths (including `.`) are resolved against the current directory.
    let args = launch_args::parse(std::env::args().skip(1));
    let open_path = args.game_dir.map(launch_args::absolute);
    let listfile = args.listfile.map(launch_args::absolute);
    let problem = args.problem;

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::APP_TITLE)
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../icon.png"))
                    .unwrap_or_default(),
            ),
        ..Default::default()
    };

    eframe::run_native(
        app::APP_TITLE,
        native_options,
        Box::new(move |cc| {
            let mut app = CascExplorerApp::new(cc);
            app.pending_listfile = listfile;
            if let Some(path) = open_path {
                app.open_game_dir(path);
            } else if let Some(listfile) = &app.pending_listfile {
                app.status = format!(
                    "Listfile {} will load when an archive is opened.",
                    listfile.display()
                );
            }
            if let Some(problem) = problem {
                app.status = problem;
            }
            Ok(Box::new(app))
        }),
    )
}
