//! About window — a real native child viewport (NOT an inner egui::Window).
//!
//! Opened from Help → About. Uses `ctx.show_viewport_immediate`, so it's
//! a true OS-level window with its own title bar, taskbar entry, and close
//! button — not a floating panel inside the main window.
//!
//! Wiring (see patch notes):
//!   • add `pub about_open: bool` and `pub about_logo: Option<egui::TextureHandle>`
//!     to `CascExplorerApp` (init both to false / None)
//!   • add `mod about;` to `src/ui/mod.rs`
//!   • Help → About: set `app.about_open = true;` (replace the status-text line)
//!   • in `ui::draw`, after the panels, call:
//!         about::show_window(ctx, &mut app.about_open, &mut app.about_logo);

use crate::ui::theme::rd;

const TAGLINE: &str = "The most modern, efficient cross-platform CASC explorer — \
view models, inspect in-game stat powers, find hidden gems.";

const REPO_URL: &str = "https://github.com/HoldMyBeer-gg/rustydemon";
const SPONSOR_URL: &str = "https://github.com/sponsors/jabberwock";
const BUG_URL: &str = "https://github.com/HoldMyBeer-gg/rustydemon/issues/new/choose";
const AUTHOR_URL: &str = "https://github.com/jabberwock";

/// Show the About window if `open`. Mutates `open` to false when the
/// user closes the OS window or clicks Close. `logo` caches the decoded
/// logo texture across frames (lazy-loaded on first open).
pub fn show_window(ctx: &egui::Context, open: &mut bool, logo: &mut Option<egui::TextureHandle>) {
    if !*open {
        return;
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

    let viewport_id = egui::ViewportId::from_hash_of("rd_about_window");
    let builder = egui::ViewportBuilder::default()
        .with_title("About RustyDemon")
        .with_inner_size([400.0, 600.0])
        .with_min_inner_size([360.0, 420.0])
        .with_resizable(true);

    ctx.show_viewport_immediate(viewport_id, builder, move |ctx, _class| {
        // Footer pinned to the bottom; credits ScrollArea fills the rest.
        egui::TopBottomPanel::bottom("about_footer")
            .frame(
                egui::Frame::none()
                    .fill(rd::FROST_050)
                    .inner_margin(egui::Margin::symmetric(20.0, 12.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("© 2026 jabberwock · AGPL-3.0 + Commons Clause")
                            .size(10.5)
                            .color(rd::FROST_600),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Copy Version").clicked() {
                            let v = env!("CARGO_PKG_VERSION");
                            ctx.copy_text(format!(
                                "RustyDemon v{v} · rustydemon-lib {v} · {}",
                                std::env::consts::OS
                            ));
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(rd::FROST_100)
                    .inner_margin(egui::Margin::symmetric(28.0, 24.0)),
            )
            .show(ctx, |ui| {
                // ── Hero ──────────────────────────────────────────────
                ui.vertical_centered(|ui| {
                    if let Some(tex) = &logo {
                        ui.add(egui::Image::new((tex.id(), egui::vec2(88.0, 88.0))));
                    }
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("RUSTY  DEMON")
                            .size(28.0)
                            .strong()
                            .color(rd::FROST_900),
                    );
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(TAGLINE).size(12.5).color(rd::FROST_700));
                    ui.add_space(12.0);

                    // Version pill.
                    egui::Frame::none()
                        .stroke(egui::Stroke::new(1.0_f32, rd::FROST_400))
                        .rounding(egui::Rounding::same(999.0))
                        .inner_margin(egui::Margin::symmetric(12.0, 3.0))
                        .show(ui, |ui| {
                            let v = env!("CARGO_PKG_VERSION");
                            ui.label(
                                egui::RichText::new(format!("● v{v} · lib {v}"))
                                    .monospace()
                                    .size(11.0)
                                    .color(rd::RUNE_400),
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
                ui.label(
                    egui::RichText::new("A C K N O W L E D G E M E N T S   &   L I C E N S E")
                        .size(10.0)
                        .strong()
                        .color(rd::FROST_600),
                );
                ui.add_space(6.0);

                // ── Scrollable credits / license ──────────────────────
                egui::Frame::none()
                    .fill(rd::FROST_200)
                    .stroke(egui::Stroke::new(1.0_f32, rd::FROST_400))
                    .rounding(egui::Rounding::same(5.0))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                credits_body(ui);
                            });
                    });
            });

        // Honour the OS close button.
        if ctx.input(|i| i.viewport().close_requested()) {
            *open = false;
        }
    });

    // If the immediate viewport set close_requested, `*open` is already
    // false; nothing else to do.
}

/// One link card in the 3-column row. `warm` gives the ember treatment
/// (used for Sponsor — the single warm accent).
fn link_card(ui: &mut egui::Ui, label: &str, url: &str, warm: bool) {
    let accent = if warm { rd::EMBER_600 } else { rd::RUNE_400 };
    let resp = egui::Frame::none()
        .fill(egui::Color32::from_rgba_premultiplied(255, 255, 255, 5))
        .stroke(egui::Stroke::new(1.0_f32, rd::FROST_400))
        .rounding(egui::Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(6.0, 11.0))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(label).size(11.5).color(accent).strong());
            });
        })
        .response
        .interact(egui::Sense::click());

    if resp.hovered() {
        ui.painter().rect_stroke(
            resp.rect,
            egui::Rounding::same(5.0),
            egui::Stroke::new(1.0_f32, accent),
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
        ui.label(
            egui::RichText::new(t)
                .size(11.0)
                .strong()
                .color(rd::RUNE_400),
        );
        ui.add_space(2.0);
    };
    let body = |ui: &mut egui::Ui, t: &str| {
        ui.label(egui::RichText::new(t).size(11.5).color(rd::FROST_800));
    };

    head(ui, "LICENSE");
    body(
        ui,
        "Source licensed under AGPL-3.0 with the Commons Clause — free to read, \
build, and use for personal and educational purposes. Commercial distribution \
is reserved to the maintainers under a separate proprietary license.",
    );

    head(ui, "BUILT ON THE CASC COMMUNITY");
    for line in [
        "• CascLib by Ladislav Zezula — C reference for BLTE, encoding, root manifests, MNDX/MARR.",
        "• CASC Explorer by the WoW-Tools team — original .NET GUI; RustyDemon's UI takes cues from it.",
        "• TACTLib by the Overtools team — TVFS and static-container reference.",
        "• SereniaBLPLib by Xalcon — BLP parsing & DXT decompression (rustydemon-blp2).",
        "• wowdev.wiki & the datamining community — documenting CASC/TACT and the listfiles.",
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
