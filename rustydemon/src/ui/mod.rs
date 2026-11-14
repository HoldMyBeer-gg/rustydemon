pub(crate) mod a11y;
mod about;
mod egg;
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

    // Primary action: ember fill, forge-colored text.
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

/// Space between the status bar's top border and its text.
const STATUS_BAR_MARGIN_Y: f32 = 4.0;

fn draw_status_bar(ui: &mut egui::Ui, app: &CascExplorerApp) {
    egui::Panel::bottom("status_bar")
        .frame(
            egui::Frame::NONE
                .fill(theme::rd::FROST_150)
                .stroke(egui::Stroke::new(1.0_f32, theme::rd::BORDER_SUBTLE))
                .inner_margin(egui::Margin::symmetric(12, STATUS_BAR_MARGIN_Y as i8)),
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
                egg::show(ui);
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

/// Paint the forge-glow vignette beneath all panels: warm ember in
/// the top-left, cool rune in the bottom-right, both at very low alpha
/// so nothing competes with actual UI content.
fn paint_forge_glow(ui: &egui::Ui) {
    let rect = ui.ctx().content_rect();
    let painter = ui.ctx().layer_painter(egui::LayerId::background());

    // Warm ember: top-left quadrant.
    let c1 = rect.min + egui::vec2(rect.width() * 0.15, -rect.height() * 0.10);
    painter.circle_filled(
        c1,
        rect.height() * 0.70,
        egui::Color32::from_rgba_premultiplied(0x1c, 0x0b, 0x02, 0x14),
    );

    // Cool rune: bottom-right quadrant.
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

    /// Text files in the repository: source, docs, scripts and packaging.
    const TEXT: &[&str] = &[
        "rs", "md", "toml", "sh", "yml", "json", "py", "wxs", "keys", "desktop", "SRCINFO",
    ];
    const TEXT_NAMES: &[&str] = &["PKGBUILD", "CREDITS", "LICENSE", ".gitignore"];

    fn is_text(path: &Path) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        name.as_deref().is_some_and(|n| TEXT_NAMES.contains(&n))
            || path
                .extension()
                .is_some_and(|e| TEXT.contains(&e.to_string_lossy().as_ref()))
    }

    /// The repository's text files. Tracked files only when git is there:
    /// an untracked folder of notes beside the checkout is not the
    /// repository's copy, and it stopped release.sh from running the suite.
    /// Without git (a source tarball), every text file under `root`.
    fn text_files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
        let tracked = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "-z"])
            .output();
        if let Ok(listing) = tracked {
            if listing.status.success() {
                for rel in listing.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
                    let path = root.join(String::from_utf8_lossy(rel).as_ref());
                    if path.is_file() && is_text(&path) {
                        out.push(path);
                    }
                }
                return;
            }
        }
        walk_text_files(root, out);
    }

    fn walk_text_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(
                    name.as_str(),
                    ".git" | "target" | "node_modules" | "bam-results"
                ) {
                    walk_text_files(&path, out);
                }
            } else if is_text(&path) {
                out.push(path);
            }
        }
    }

    /// Whether `word` appears in `line` as a whole word, ignoring case.
    fn has_word(line: &str, word: &str) -> bool {
        let line = line.to_lowercase();
        line.match_indices(word).any(|(at, _)| {
            let before = line[..at].chars().next_back();
            let after = line[at + word.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    }

    #[test]
    fn whole_words_only() {
        assert!(has_word("It works, maybe.", "maybe"));
        assert!(!has_word("maybeso", "maybe"));
        assert!(has_word("Maybe not", "maybe"));
    }

    /// Text files hold no raw control bytes.  One NUL typed into a string
    /// makes grep treat the whole file as binary and hide its matches;
    /// write such bytes as `\x00` escapes.
    #[test]
    fn no_raw_control_bytes_in_text_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut files = Vec::new();
        text_files(root, &mut files);
        let mut found = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            for (i, line) in text.lines().enumerate() {
                if line.chars().any(|c| c.is_control() && c != '\t') {
                    found.push(format!("{rel}:{}", i + 1));
                }
            }
        }
        assert!(found.is_empty(), "raw control bytes at: {found:#?}");
    }

    /// Everything in the repository reads as plain prose: no long dashes and
    /// none of the filler words below, in code, comments, docs or scripts.
    #[test]
    fn plain_punctuation_and_wording_everywhere() {
        let dash = '\u{2014}';
        // Spelled in pieces so this file does not trip its own check.
        let words = [
            ["prob", "ably"].concat(),
            ["hon", "estly"].concat(),
            ["cav", "eat"].concat(),
            ["cav", "eats"].concat(),
        ];
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut files = Vec::new();
        text_files(root, &mut files);
        let mut found = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            for (i, line) in text.lines().enumerate() {
                if line.contains(dash) {
                    found.push(format!("{rel}:{}: long dash", i + 1));
                }
                for w in &words {
                    if has_word(line, w) {
                        found.push(format!("{rel}:{}: {w}", i + 1));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "{} lines to rewrite: {found:#?}",
            found.len()
        );
    }
}
