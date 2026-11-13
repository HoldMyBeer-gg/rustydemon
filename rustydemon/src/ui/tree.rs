use crate::app::CascExplorerApp;
use crate::ui::theme::{self, rd};
use rustydemon_lib::{CascFile, CascFolder};

/// Most rows drawn per folder, for sub-folders and for files separately.
///
/// The tree draws every row of every open folder each frame, and some WoW
/// folders hold tens of thousands of files, so each list is cut off here.
/// Whatever is cut gets an [`overflow_row`] so the gap is never silent.
const TREE_ROW_CAP: usize = 200;

/// How many of `total` rows the cap leaves out.
fn hidden_rows(total: usize) -> usize {
    total.saturating_sub(TREE_ROW_CAP)
}

/// `12345` -> `"12,345"`.
fn with_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Text for the row that stands in for `hidden` undrawn rows of `folder`.
fn overflow_label(hidden: usize, noun: &str, folder: &str) -> String {
    let (plural, pronoun) = if hidden == 1 {
        ("", "it")
    } else {
        ("s", "them")
    };
    format!(
        "… {} more {noun}{plural}. Click {folder} to browse {pronoun}",
        with_thousands(hidden)
    )
}

/// A muted, clickable row saying that `hidden` rows of `folder` were not
/// drawn.  It carries its own accessible name so screen readers and UI
/// automation can tell the list is incomplete.
fn overflow_row(
    ui: &mut egui::Ui,
    depth: i32,
    hidden: usize,
    noun: &str,
    folder: &str,
) -> egui::Response {
    let label = overflow_label(hidden, noun, folder);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    let color = if resp.hovered() {
        rd::FG_PRIMARY
    } else {
        rd::FG_MUTED
    };
    // Lines up with the names of the rows above it (indent + caret + icon).
    let x = rect.left() + 6.0 + depth as f32 * 12.0 + 30.0;
    ui.painter().text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &label,
        egui::FontId::proportional(12.0),
        color,
    );
    resp
}

pub enum TreeClick {
    File(u64),
    Folder(String),
}

pub fn draw_tree(ui: &mut egui::Ui, app: &mut CascExplorerApp) -> Option<TreeClick> {
    let game_label = app.game_label().unwrap_or_else(|| "—".to_string());
    theme::panel_header(ui, "File Browser", Some(&game_label));

    if app.handler.is_none() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("No archive open.").color(rd::FG_MUTED));
        return None;
    }

    // Handler is open, but the root manifest didn't ship with file paths
    // (MFST WoW, etc.) and no listfile has been applied yet — guide the
    // user to the next step instead of leaving the panel mute.
    let needs_listfile = app
        .handler
        .as_ref()
        .map(|h| h.root_folder.is_none())
        .unwrap_or(false);
    if needs_listfile {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Archive opened, but no file paths are loaded.")
                .color(rd::FG_MUTED),
        );
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("Use File → Load Listfile… to populate the tree.")
                .color(rd::FG_MUTED)
                .small(),
        );
        return None;
    }

    apply_expansion_commands(ui, app);
    let mut click: Option<TreeClick> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        // "Game Root" pseudo-row at depth 0.
        click = draw_folder_rows(ui, app, "", 0, "Game Root", true);
    });
    click
}

fn draw_folder_rows(
    ui: &mut egui::Ui,
    app: &mut CascExplorerApp,
    path: &str,
    depth: i32,
    display_name: &str,
    force_open: bool,
) -> Option<TreeClick> {
    // Snapshot folder contents up-front so the borrow on `app.handler`
    // is released before we recurse (which needs `&mut app`).
    let (sub_names, files): (Vec<String>, Vec<CascFile>) = {
        let handler = app.handler.as_ref()?;
        let root = handler.root_folder.as_ref()?;
        let folder: &CascFolder = if path.is_empty() {
            root
        } else {
            root.navigate(path)?
        };
        let mut sn: Vec<String> = folder.folders.keys().cloned().collect();
        sn.sort_unstable();
        let mut fs: Vec<CascFile> = folder.files.values().cloned().collect();
        fs.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        (sn, fs)
    };

    let is_open = force_open || app.expanded.contains(path);
    let is_browsed = app.browsed_folder.as_deref() == Some(path);

    let mut click: Option<TreeClick> = None;

    // The folder header row itself (skip for "" root — shown as "Game Root" label only at depth 0).
    if !path.is_empty() || depth == 0 {
        let row = tree_row(
            ui,
            depth,
            Some(is_open),
            "📁",
            display_name,
            is_browsed,
            false,
        );
        // Caret toggles expansion (root is force-open and ignores toggles).
        if row.caret_clicked && !force_open {
            if is_open {
                app.expanded.remove(path);
            } else {
                app.expanded.insert(path.to_string());
            }
        }
        // Body click browses the folder.
        if row.body.clicked() {
            click = Some(TreeClick::Folder(path.to_string()));
        }
    }

    // Re-read after possible toggle so the new state takes effect this frame.
    let is_open = force_open || app.expanded.contains(path);
    if !is_open {
        return click;
    }

    // Sub-folders.
    for name in sub_names.iter().take(TREE_ROW_CAP) {
        let child = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        if let Some(c) = draw_folder_rows(ui, app, &child, depth + 1, name, false) {
            click = Some(c);
        }
    }

    let hidden_folders = hidden_rows(sub_names.len());
    if hidden_folders > 0
        && overflow_row(ui, depth + 1, hidden_folders, "folder", display_name).clicked()
    {
        click = Some(TreeClick::Folder(path.to_string()));
    }

    // Files.
    let selected_hash = app.selected.as_ref().map(|s| s.result.hash);
    for file in files.iter().take(TREE_ROW_CAP) {
        let is_sel = selected_hash == Some(file.hash);
        let row = tree_row(
            ui,
            depth + 1,
            None,
            file_icon(&file.name),
            &file.name,
            false,
            is_sel,
        );
        if row.body.clicked() {
            click = Some(TreeClick::File(file.hash));
        }
    }
    let hidden_files = hidden_rows(files.len());
    if hidden_files > 0 && overflow_row(ui, depth + 1, hidden_files, "file", display_name).clicked()
    {
        click = Some(TreeClick::Folder(path.to_string()));
    }

    click
}

/// Response from a tree row — body click vs caret click are separate.
struct TreeRowResponse {
    body: egui::Response,
    caret_clicked: bool,
}

/// Render one tree row. `caret` = Some(is_open) draws a rotating `▶`;
/// None is a file row.
fn tree_row(
    ui: &mut egui::Ui,
    depth: i32,
    caret: Option<bool>,
    icon: &str,
    name: &str,
    is_browsed: bool,
    is_sel: bool,
) -> TreeRowResponse {
    let indent = 6.0 + depth as f32 * 12.0;
    let row_height = 20.0;
    // Allocate the full row hover-only; we install two click hit-zones below.
    let (rect, _hover) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::hover(),
    );

    // Carve out caret + body hit-zones. The caret zone is the leading ~24px
    // (indent + 12px caret column) so a click there toggles expansion
    // without browsing; everything to the right browses on click.
    let caret_w = if caret.is_some() { indent + 18.0 } else { 0.0 };
    let caret_rect = egui::Rect::from_min_size(rect.min, egui::vec2(caret_w, rect.height()));
    let body_rect =
        egui::Rect::from_min_max(egui::pos2(rect.left() + caret_w, rect.top()), rect.max);

    let caret_id = ui
        .scope_id()
        .with(("caret", rect.min.x.to_bits(), rect.min.y.to_bits()));
    let body_id = ui
        .scope_id()
        .with(("body", rect.min.x.to_bits(), rect.min.y.to_bits()));
    let caret_resp = if caret.is_some() {
        Some(ui.interact(caret_rect, caret_id, egui::Sense::click()))
    } else {
        None
    };
    let body_resp = ui.interact(body_rect, body_id, egui::Sense::click());

    // The row is painted by hand below, so egui knows nothing about it
    // unless we describe it.  Without this the whole file browser is
    // invisible to screen readers and UI automation.
    body_resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, is_sel || is_browsed, name)
    });
    if let (Some(resp), Some(open)) = (&caret_resp, caret) {
        let verb = if open { "Collapse" } else { "Expand" };
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{verb} {name}"))
        });
    }

    // Hover paints the *body* zone — caret hover is implicit feedback only.
    let resp = &body_resp;

    let painter = ui.painter();
    // Hover / selection backgrounds.
    if is_sel {
        painter.rect_filled(rect, 2.0, theme::selected_row_fill());
        theme::row_accent_stripe(ui, rect);
    } else if resp.hovered() {
        painter.rect_filled(
            rect,
            2.0,
            egui::Color32::from_rgba_premultiplied(29, 142, 232, 26),
        );
    }

    let text_color = if is_sel {
        egui::Color32::WHITE
    } else if is_browsed {
        rd::RUNE_400
    } else {
        rd::FG_PRIMARY
    };

    let mut x = rect.left() + indent;
    let y = rect.center().y;

    // Caret. Highlights when its hit-zone is hovered.
    if let Some(open) = caret {
        let ch = if open { CARET_OPEN } else { CARET_CLOSED };
        let caret_color = if caret_resp.as_ref().map(|r| r.hovered()).unwrap_or(false) {
            rd::FG_PRIMARY
        } else {
            rd::FG_MUTED
        };
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            ch,
            caret_font(),
            caret_color,
        );
        x += 12.0;
    } else {
        x += 12.0;
    }

    // Icon.
    painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(13.0),
        rd::RUNE_400,
    );
    x += 18.0;

    // Name.
    painter.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(13.0),
        text_color,
    );

    TreeRowResponse {
        body: body_resp,
        caret_clicked: caret_resp.map(|r| r.clicked()).unwrap_or(false),
    }
}

fn apply_expansion_commands(_ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    // With the custom row-based tree, expansion state lives entirely in
    // app.expanded — no egui CollapsingState sync needed.
    app.expansion_dirty = false;
}

/// The expand/collapse carets and the font they are drawn in.
pub(crate) const CARET_OPEN: &str = "▾";
pub(crate) const CARET_CLOSED: &str = "▸";
pub(crate) fn caret_font() -> egui::FontId {
    // Monospace: neither Inter nor the emoji fonts have these two triangles,
    // so in the proportional family they came out as empty boxes.
    egui::FontId::monospace(11.0)
}

pub(crate) fn file_icon(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".blp") || lower.ends_with(".tex") {
        "🖼"
    } else if lower.ends_with(".m2") || lower.ends_with(".mdx") {
        "📦"
    } else if lower.ends_with(".pow") || lower.ends_with(".gam") {
        "⚙"
    } else if lower.ends_with(".mp3")
        || lower.ends_with(".ogg")
        || lower.ends_with(".wav")
        || lower.ends_with(".wsb")
    {
        "🎵"
    } else if lower.ends_with(".vid") {
        "🎬"
    } else {
        "📄"
    }
}

#[cfg(test)]
mod a11y_tests {
    use super::*;

    /// Names of every accessibility node one headless frame publishes.
    fn published_names(add: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        let mut add = add;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let out = crate::ui::a11y::testing::frame(&ctx, |ui| add(ui));
        out.nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_owned))
            .collect()
    }

    /// Hand-painted rows must still describe themselves, or screen readers
    /// and UI automation see an empty file browser.
    #[test]
    fn folder_row_is_published_with_its_name() {
        let names = published_names(|ui| {
            tree_row(ui, 1, Some(false), "📁", "interface", false, false);
        });
        assert!(names.iter().any(|n| n == "interface"), "{names:?}");
    }

    #[test]
    fn folder_caret_says_what_it_will_do() {
        let closed = published_names(|ui| {
            tree_row(ui, 1, Some(false), "📁", "interface", false, false);
        });
        assert!(closed.iter().any(|n| n == "Expand interface"), "{closed:?}");

        let open = published_names(|ui| {
            tree_row(ui, 1, Some(true), "📁", "interface", false, false);
        });
        assert!(open.iter().any(|n| n == "Collapse interface"), "{open:?}");
    }

    #[test]
    fn file_row_is_published_with_its_name() {
        let names = published_names(|ui| {
            tree_row(ui, 2, None, "📄", "bindings.xml", false, true);
        });
        assert!(names.iter().any(|n| n == "bindings.xml"), "{names:?}");
    }
}

#[cfg(test)]
mod overflow_tests {
    use super::*;

    #[test]
    fn nothing_is_hidden_at_or_under_the_cap() {
        assert_eq!(hidden_rows(0), 0);
        assert_eq!(hidden_rows(TREE_ROW_CAP), 0);
    }

    #[test]
    fn rows_past_the_cap_are_counted() {
        assert_eq!(hidden_rows(TREE_ROW_CAP + 1), 1);
        assert_eq!(hidden_rows(79_465), 79_465 - TREE_ROW_CAP);
    }

    #[test]
    fn label_says_how_many_and_what_to_do() {
        assert_eq!(
            overflow_label(79_265, "file", "bakednpctextures"),
            "… 79,265 more files. Click bakednpctextures to browse them"
        );
        assert_eq!(
            overflow_label(1, "folder", "spells"),
            "… 1 more folder. Click spells to browse it"
        );
    }

    /// The notice has to reach the accessibility tree, or automation and
    /// screen readers still can't tell that rows were left out.
    #[test]
    fn overflow_row_is_published() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let out = crate::ui::a11y::testing::frame(&ctx, |ui| {
            overflow_row(ui, 2, 54_489, "file", "head");
        });
        let names: Vec<String> = out
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_owned))
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n == "… 54,489 more files. Click head to browse them"),
            "{names:?}"
        );
    }
}
