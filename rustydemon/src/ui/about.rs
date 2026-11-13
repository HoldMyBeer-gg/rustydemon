//! About window, opened from Help → About.
//!
//! Drawn as an egui window inside the main window. It used to be a native
//! child viewport with its own title bar, but eframe publishes an
//! accessibility tree for the root viewport only, so nothing in a child
//! viewport could be read by a screen reader or reached by automation.

use crate::ui::a11y;
use crate::ui::theme::rd;

const TAGLINE: &str = "The most modern, efficient cross-platform CASC explorer: \
view models, inspect in-game stat powers, find hidden gems.";

const REPO_URL: &str = "https://github.com/HoldMyBeer-gg/rustydemon";
const SPONSOR_URL: &str = "https://github.com/sponsors/jabberwock";
const BUG_URL: &str = "https://github.com/HoldMyBeer-gg/rustydemon/issues/new/choose";
const COPYRIGHT: &str = "© 2026 jabberwock · AGPL-3.0 + Commons Clause";
const AUTHOR_URL: &str = "https://github.com/jabberwock";
/// Background of a link card.
const CARD_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(5, 5, 5, 5);

/// Show the About window if `open`. Mutates `open` to false when the
/// user closes the OS window or clicks Close. `logo` caches the decoded
/// logo texture across frames (lazy-loaded on first open).
pub fn show_window(
    ctx: &egui::Context,
    open: &mut bool,
    logo: &mut Option<egui::TextureHandle>,
    bounds: egui::Rect,
) -> Option<egui::Rect> {
    if !*open {
        return None;
    }

    // Lazily decode the bundled icon into a texture (once).
    if logo.is_none() {
        if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../icon.png")) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            *logo = Some(ctx.load_texture("rd_about_logo", image, egui::TextureOptions::LINEAR));
        }
    }
    let logo = logo.clone();

    // `keep_open` carries the Close button's verdict out of the closure;
    // the window's own close box writes to `open` directly.
    let mut keep_open = true;
    // Centred in the free area, and never taller than it: a window that
    // hangs off the bottom hides its own footer.
    const TITLE_BAR: f32 = 40.0;
    let max_height = (bounds.height() - TITLE_BAR - 16.0).max(120.0);
    let offset = bounds.center() - ctx.content_rect().center();
    let shown = egui::Window::new("About RustyDemon")
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_size([400.0, 600.0_f32.min(max_height)])
        .min_size([360.0, 420.0_f32.min(max_height)])
        .max_height(max_height)
        .anchor(egui::Align2::CENTER_CENTER, offset)
        .frame(egui::Frame::window(&ctx.global_style()).inner_margin(0.0))
        .show(ctx, |ui| about_body(ui, &logo, &mut keep_open));
    if !keep_open {
        *open = false;
    }
    shown.map(|s| s.response.rect)
}

/// Everything inside the About window. `open` is set to false when Close is
/// clicked.
fn about_body(ui: &mut egui::Ui, logo: &Option<egui::TextureHandle>, open: &mut bool) {
    let ctx = ui.ctx().clone();
    {
        // Footer pinned to the bottom; credits ScrollArea fills the rest.
        egui::Panel::bottom("about_footer")
            .frame(
                egui::Frame::NONE
                    .fill(rd::FROST_050)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ui, |ui| {
                // Buttons first, from the right; the copyright line wraps in
                // whatever width is left.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            *open = false;
                        }
                        if ui.button("Copy Version").clicked() {
                            let v = env!("CARGO_PKG_VERSION");
                            ctx.copy_text(format!(
                                "RustyDemon v{v} · rustydemon-lib {v} · {}",
                                std::env::consts::OS
                            ));
                        }
                        ui.with_layout(
                            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(COPYRIGHT)
                                        .size(10.5)
                                        .color(rd::FROST_600),
                                );
                            },
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(rd::FROST_100)
                    .inner_margin(egui::Margin::symmetric(28, 24)),
            )
            .show(ui, |ui| {
                // One scroll area for the whole body, so a short window can
                // still reach everything and the footer stays in view.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // ── Hero ──────────────────────────────────────────────
                        ui.vertical_centered(|ui| {
                            if let Some(tex) = logo {
                                let resp =
                                    ui.add(egui::Image::new((tex.id(), egui::vec2(88.0, 88.0))));
                                a11y::image(ui.ctx(), resp.id, "Rusty Demon logo");
                            }
                            ui.add_space(12.0);
                            a11y::decorated(
                                ui,
                                egui::RichText::new("RUSTY  DEMON")
                                    .size(28.0)
                                    .strong()
                                    .color(rd::FROST_900),
                                egui::Role::Heading,
                                "Rusty Demon",
                            );
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(TAGLINE).size(12.5).color(rd::FROST_700));
                            ui.add_space(12.0);

                            // Version pill.
                            egui::Frame::NONE
                                .stroke(egui::Stroke::new(1.0_f32, rd::FROST_400))
                                .corner_radius(egui::CornerRadius::same(255))
                                .inner_margin(egui::Margin::symmetric(12, 3))
                                .show(ui, |ui| {
                                    let v = env!("CARGO_PKG_VERSION");
                                    a11y::decorated(
                                        ui,
                                        egui::RichText::new(format!("● v{v} · lib {v}"))
                                            .monospace()
                                            .size(11.0)
                                            .color(rd::RUNE_400),
                                        egui::Role::Label,
                                        &format!("Version {v}, library {v}"),
                                    );
                                });

                            ui.add_space(14.0);
                            ui.horizontal(|ui| {
                                // Center the "Created by jabberwock" row.
                                ui.add_space((ui.available_width() - 150.0).max(0.0) / 2.0);
                                ui.label(egui::RichText::new("Created by").color(rd::FROST_700));
                                ui.hyperlink_to(
                                    egui::RichText::new("jabberwock")
                                        .color(rd::RUNE_400)
                                        .strong(),
                                    AUTHOR_URL,
                                );
                            });
                        });

                        ui.add_space(18.0);

                        // ── Link cards ────────────────────────────────────────
                        ui.columns(3, |cols| {
                            link_card(&mut cols[0], "Repository", REPO_URL, false);
                            link_card(&mut cols[1], "Sponsor", SPONSOR_URL, true);
                            link_card(&mut cols[2], "Report a Bug", BUG_URL, false);
                        });

                        ui.add_space(20.0);
                        a11y::decorated(
                            ui,
                            egui::RichText::new(
                                "A C K N O W L E D G E M E N T S   &   L I C E N S E",
                            )
                            .size(10.0)
                            .strong()
                            .color(rd::FROST_600),
                            egui::Role::Heading,
                            "Acknowledgements & License",
                        );
                        ui.add_space(6.0);

                        // ── Credits / license ──────────────────────
                        egui::Frame::NONE
                            .fill(rd::FROST_200)
                            .stroke(egui::Stroke::new(1.0_f32, rd::FROST_400))
                            .corner_radius(egui::CornerRadius::same(5))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, credits_body);
                    });
            });
    }
}

/// One link card in the 3-column row. `warm` gives the ember treatment
/// (used for Sponsor — the single warm accent).
fn link_card(ui: &mut egui::Ui, label: &str, url: &str, warm: bool) {
    let accent = if warm { rd::EMBER_600 } else { rd::RUNE_400 };
    let resp = egui::Frame::NONE
        .fill(CARD_FILL)
        .stroke(egui::Stroke::new(1.0_f32, rd::FROST_400))
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(6, 11))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                // Painted, not a label widget: the card itself is the link
                // and carries the name. A label here would be a second node
                // and the name would be read twice.
                let galley = ui.painter().layout_no_wrap(
                    label.to_owned(),
                    egui::FontId::proportional(11.5),
                    accent,
                );
                let (_, rect) = ui.allocate_space(galley.size());
                ui.painter().galley(rect.min, galley, accent);
            });
        })
        .response
        .interact(egui::Sense::click());
    // A framed label is not a control to a screen reader until it is told so.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, label));

    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            egui::CornerRadius::same(5),
            egui::Stroke::new(1.0_f32, accent),
            egui::StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::same_tab(url));
    }
}

fn credits_body(ui: &mut egui::Ui) {
    let head = |ui: &mut egui::Ui, t: &str| {
        ui.add_space(4.0);
        a11y::decorated(
            ui,
            egui::RichText::new(t)
                .size(11.0)
                .strong()
                .color(rd::RUNE_400),
            egui::Role::Heading,
            t,
        );
        ui.add_space(2.0);
    };
    let body = |ui: &mut egui::Ui, t: &str| {
        let shown = egui::RichText::new(t).size(11.5).color(rd::FROST_800);
        // The bullet is drawn, not spoken.
        match t.strip_prefix("• ") {
            Some(plain) => {
                a11y::decorated(ui, shown, egui::Role::Label, plain);
            }
            None => {
                ui.label(shown);
            }
        }
    };

    head(ui, "LICENSE");
    body(
        ui,
        "Source licensed under AGPL-3.0 with the Commons Clause: free to read, \
build, and use for personal and educational purposes. Commercial distribution \
is reserved to the maintainers under a separate proprietary license.",
    );

    head(ui, "BUILT ON THE CASC COMMUNITY");
    for line in [
        "• CascLib by Ladislav Zezula: C reference for BLTE, encoding, root manifests, MNDX/MARR.",
        "• CASC Explorer by the WoW-Tools team: original .NET GUI; RustyDemon's UI takes cues from it.",
        "• TACTLib by the Overtools team: TVFS and static-container reference.",
        "• SereniaBLPLib by Xalcon: BLP parsing & DXT decompression (rustydemon-blp2).",
        "• wowdev.wiki & the datamining community: documenting CASC/TACT and the listfiles.",
    ] {
        body(ui, line);
        ui.add_space(3.0);
    }

    head(ui, "FONTS");
    body(
        ui,
        "Cinzel Decorative, Inter, and JetBrains Mono (SIL OFL). \
OpenDyslexic bundled as an accessibility alternate.",
    );
}

#[cfg(test)]
mod a11y_tests {
    use crate::ui::a11y::testing::{named, nodes};
    use egui::accesskit::Role;

    fn about() -> Vec<(Role, String)> {
        named(&nodes(|ui| {
            let mut open = true;
            super::about_body(ui, &None, &mut open);
        }))
    }

    fn has(n: &[(Role, String)], role: Role, name: &str) -> bool {
        n.iter().any(|(r, s)| *r == role && s == name)
    }

    #[test]
    fn about_contents_are_published() {
        let n = about();
        assert!(has(&n, Role::Button, "Close"), "{n:?}");
        assert!(has(&n, Role::Button, "Copy Version"), "{n:?}");
        assert!(has(&n, Role::Heading, "Rusty Demon"), "{n:?}");
        assert!(
            has(&n, Role::Heading, "Acknowledgements & License"),
            "{n:?}"
        );
        let version = format!("Version {v}, library {v}", v = env!("CARGO_PKG_VERSION"));
        assert!(n.iter().any(|(_, s)| *s == version), "{n:?}");
    }

    #[test]
    fn link_cards_are_links() {
        let n = about();
        for name in ["Repository", "Sponsor", "Report a Bug"] {
            assert!(has(&n, Role::Link, name), "{name}: {n:?}");
        }
    }

    #[test]
    fn no_letter_spaced_names() {
        let n = about();
        assert!(
            !n.iter()
                .any(|(_, s)| s.contains("A C K") || s.contains("RUSTY  DEMON")),
            "{n:?}"
        );
    }

    #[test]
    fn each_link_card_is_announced_once() {
        let n = about();
        for name in ["Repository", "Sponsor", "Report a Bug"] {
            let count = n.iter().filter(|(_, s)| s == name).count();
            assert_eq!(count, 1, "{name}: {n:?}");
        }
    }

    #[test]
    fn decoration_glyphs_stay_out_of_names() {
        let n = about();
        assert!(
            !n.iter()
                .any(|(_, s)| s.starts_with('•') || s.starts_with('●')),
            "{n:?}"
        );
    }

    /// At the window's default width the copyright line and the two buttons
    /// do not fit on one row; the line must give way, not be drawn under them.
    #[test]
    fn footer_text_is_not_drawn_under_the_buttons() {
        use crate::ui::a11y::testing::{bounds_of, nodes_sized, overlap};
        let all = nodes_sized([400.0, 600.0], |ui| {
            let mut open = true;
            super::about_body(ui, &None, &mut open);
        });
        let text = bounds_of(&all, super::COPYRIGHT);
        for button in ["Close", "Copy Version"] {
            let b = bounds_of(&all, button);
            assert!(!overlap(text, b), "{button} {b:?} covers {text:?}");
        }
    }

    /// Draw the whole window for two frames in a screen of `screen` points
    /// whose free area is `bounds`; returns where the window ended up.
    fn window_in(screen: [f32; 2], bounds: egui::Rect) -> egui::Rect {
        use crate::ui::a11y::testing::{frame_in, sized};
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        ctx.enable_accesskit();
        let mut rect = None;
        for _ in 0..3 {
            frame_in(&ctx, sized(screen), |ui| {
                let mut open = true;
                rect = super::show_window(ui.ctx(), &mut open, &mut None, bounds);
            });
        }
        rect.expect("window shown")
    }

    /// The window is 600 points tall by default. In a main window shorter
    /// than that it used to hang off the bottom, cutting off the footer.
    #[test]
    fn the_window_fits_a_short_main_window() {
        let bounds = egui::Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(800.0, 470.0));
        let rect = window_in([800.0, 500.0], bounds);
        assert!(bounds.contains_rect(rect), "{rect:?} leaves {bounds:?}");
    }

    /// It also stays off the menu bar and status bar when there is room.
    #[test]
    fn the_window_stays_between_menu_and_status_bar() {
        let bounds = egui::Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(1100.0, 690.0));
        let rect = window_in([1100.0, 728.0], bounds);
        assert!(bounds.contains_rect(rect), "{rect:?} leaves {bounds:?}");
    }

    /// A premultiplied colour brighter than its alpha is not a faint tint;
    /// it draws as solid white.
    #[test]
    fn link_card_fill_is_a_faint_tint() {
        let [r, g, b, a] = super::CARD_FILL.to_array();
        assert!(r <= a && g <= a && b <= a, "{:?}", super::CARD_FILL);
    }
}
