pub(crate) mod a11y;
mod about;
mod menu;
pub(crate) mod preview;
pub(crate) mod results;
pub(crate) mod theme;
pub(crate) mod tree;

use crate::app::CascExplorerApp;
use rustydemon_lib::SearchResult;

/// Draw the entire application UI for one frame.
///
/// Returns the area between the menu bar and the status bar, where floating
/// windows belong.
pub fn draw(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> egui::Rect {
    menu::draw_menu(ui, app);
    draw_status_bar(ui, app);
    let content = ui.available_rect_before_wrap();
    draw_panels(ui, app);
    about::show_window(ui.ctx(), &mut app.about_open, &mut app.about_logo, content);
    content
}

fn draw_panels(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    let ctx = &ui.ctx().clone();
    let central_frame = egui::Frame::central_panel(&ctx.global_style()).fill(theme::rd::BG_APP);
    egui::CentralPanel::default()
        .frame(central_frame)
        .show(ui, |ui| {
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
                    zoom_buttons(ui);
                });
            });
            ui.separator();

            // Collect tree click before split so borrows don't overlap.
            let mut tree_click: Option<tree::TreeClick> = None;
            let mut result_click: Option<rustydemon_lib::SearchResult> = None;

            egui::Panel::left("tree_panel")
                .resizable(true)
                .default_size(220.0)
                .min_size(140.0)
                .show(ui, |ui| {
                    tree_click = tree::draw_tree(ui, app);
                });

            egui::Panel::right("preview_panel")
                .resizable(true)
                .default_size(240.0)
                .min_size(160.0)
                .show(ui, |ui| {
                    preview::draw_preview(ui, app);
                });

            egui::CentralPanel::default().show(ui, |ui| {
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

/// The text-size buttons. Drawn right to left, so `A+` comes first.
fn zoom_buttons(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if a11y::button(ui, "A+", "Increase text size")
        .on_hover_text("Increase font size")
        .clicked()
    {
        let z = (ctx.zoom_factor() + 0.1).min(3.0);
        ctx.set_zoom_factor(z);
    }
    if a11y::button(ui, "A-", "Decrease text size")
        .on_hover_text("Decrease font size")
        .clicked()
    {
        let z = (ctx.zoom_factor() - 0.1).max(0.5);
        ctx.set_zoom_factor(z);
    }
}

/// The search field. Its hint text is not an accessible name, so it gets
/// one here.
fn search_box(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text("Search files…")
            .desired_width(300.0),
    );
    a11y::name(ui.ctx(), response.id, "Search files");
    response
}

fn toolbar(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    let response = search_box(ui, &mut app.search_text);
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        app.run_search();
    }

    // Primary action — ember fill, forge-colored text.
    let search_btn = egui::Button::new(
        egui::RichText::new("Search")
            .color(theme::rd::FG_ON_EMBER)
            .strong(),
    )
    .fill(theme::rd::EMBER_500);
    if ui.add(search_btn).clicked() {
        app.run_search();
    }

    ui.separator();

    ui.checkbox(&mut app.deep_search_enabled, "Deep search");

    if a11y::button(ui, "🔍 Find All (Deep Search)", "Find All (Deep Search)").clicked() {
        app.run_deep_search();
    }
}

fn draw_status_bar(ui: &mut egui::Ui, app: &CascExplorerApp) {
    egui::Panel::bottom("status_bar")
        .frame(
            egui::Frame::NONE
                .fill(theme::rd::FROST_150)
                .stroke(egui::Stroke::new(1.0_f32, theme::rd::BORDER_SUBTLE))
                .inner_margin(egui::Margin::symmetric(12, 4)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if app.loading {
                    ui.spinner();
                }
                // Split the status string into an ember-tinted leading verb
                // (if any) and the rest in muted body color, so live states
                // ("Opening…", "Loading…", "Searching…") read as hot while
                // idle status stays calm.
                status_line(ui, &app.status);
            });
        });
}

/// The status text: an ember-tinted leading verb (if any) and the rest in
/// muted body color, so live states ("Opening…", "Loading…") read as hot
/// while idle status stays calm.
///
/// One label with two colors, not two labels: a screen reader gets the whole
/// sentence as a single status line, and is told when it changes.
fn status_line(ui: &mut egui::Ui, status: &str) {
    let (accent, rest) = split_status_accent(status);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    if let Some(a) = accent {
        job.append(
            a,
            0.0,
            egui::TextFormat::simple(font.clone(), theme::rd::EMBER_600),
        );
    }
    job.append(
        rest,
        0.0,
        egui::TextFormat::simple(font, theme::rd::FG_SECONDARY),
    );
    let resp = a11y::decorated(ui, job, egui::Role::Status, status);
    a11y::live(ui.ctx(), resp.id);
}

/// Split a status string into an optional hot prefix (one of a short
/// allowlist of live-action verbs) and the rest, for two-tone rendering.
fn split_status_accent(status: &str) -> (Option<&str>, &str) {
    for prefix in [
        "Opening",
        "Loading",
        "Searching",
        "Deep searching",
        "Exporting",
        "Playing",
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
    let rect = ui.ctx().content_rect();
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

#[cfg(test)]
mod copy_tests {
    use std::path::Path;

    /// The part of a source line before its `//` comment, if it has one.
    fn code_part(line: &str) -> &str {
        // "://" is a URL inside a string, not the start of a comment.
        let mut from = 0;
        while let Some(at) = line[from..].find("//") {
            let at = from + at;
            if at == 0 || line.as_bytes()[at - 1] != b':' {
                return &line[..at];
            }
            from = at + 2;
        }
        line
    }

    /// Every line of Rust source under `dir` that has `needle` outside a
    /// comment, as `file:line`.
    fn lines_with(dir: &Path, needle: char, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                lines_with(&path, needle, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    if code_part(line).contains(needle) {
                        found.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
    }

    #[test]
    fn a_dash_after_a_url_is_not_mistaken_for_a_comment() {
        assert!(code_part("say(\"see https://example.com \u{2014} now\");").contains('\u{2014}'));
        assert!(!code_part("let x = 1; // old \u{2014} note").contains('\u{2014}'));
        assert!(!code_part("    /// doc \u{2014} comment").contains('\u{2014}'));
    }

    /// Text people read (labels, messages, command-line output, the README,
    /// the key file users edit) uses plain punctuation. An em dash may only
    /// appear in a source comment.
    #[test]
    fn no_em_dashes_in_text_people_read() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut found = Vec::new();
        for krate in ["rustydemon", "rustydemon-cli", "rustydemon-lib"] {
            for dir in ["src", "examples"] {
                let dir = workspace.join(krate).join(dir);
                if dir.is_dir() {
                    lines_with(&dir, '\u{2014}', &mut found);
                }
            }
        }
        for file in [
            "README.md",
            "COMMERCIAL.md",
            "rustydemon-lib/README.md",
            "rustydemon-blp2/README.md",
            "rustydemon-gr2/README.md",
            "rustydemon-lib/keys/d4.keys",
        ] {
            let text = std::fs::read_to_string(workspace.join(file)).unwrap();
            for (i, line) in text.lines().enumerate() {
                if line.contains('\u{2014}') {
                    found.push(format!("{file}:{}", i + 1));
                }
            }
        }
        assert!(found.is_empty(), "em dash outside a comment: {found:#?}");
    }
}
