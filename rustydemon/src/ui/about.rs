//! About modal — the Frost theme showpiece.
//!
//! Gradient header (mesh-painted), Cinzel title, ember-glow sponsor button.

use egui::{
    vec2, Align, Align2, Color32, Context, FontFamily, FontId, Layout, Mesh, OpenUrl, Rounding,
    Sense, Shape, Stroke, Ui, Vec2, Window,
};

const REPO: &str = "https://github.com/HoldMyBeer-gg/rustydemon";
const SPONSORS: &str = "https://github.com/sponsors/jabberwock";
const ISSUES: &str = "https://github.com/HoldMyBeer-gg/rustydemon/issues";

const HEADER_H: f32 = 110.0;
const HEADER_TOP: Color32 = Color32::from_rgb(38, 68, 96);
const HEADER_BOT: Color32 = Color32::from_rgb(10, 14, 22);
const EMBER: Color32 = Color32::from_rgb(217, 104, 50);
const FROST: Color32 = Color32::from_rgb(79, 195, 247);

pub fn draw(ctx: &Context, open: &mut bool) {
    if !*open {
        return;
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
        return;
    }

    let mut keep_open = true;
    Window::new("about_window")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.set_width(440.0);

            gradient_header(ui);

            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(format!("version {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(6.0);
                ui.label("A CASC archive explorer for Blizzard games.");
                ui.label("WoW · StarCraft · Diablo · Overwatch · Heroes");
            });

            ui.add_space(20.0);

            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if glow_button(ui, "♥  Support", EMBER).clicked() {
                    ctx.open_url(OpenUrl::new_tab(SPONSORS));
                }
                ui.add_space(6.0);
                if glow_button(ui, "Report a Bug", FROST).clicked() {
                    ctx.open_url(OpenUrl::new_tab(ISSUES));
                }
                ui.add_space(6.0);
                if glow_button(ui, "View Source", FROST).clicked() {
                    ctx.open_url(OpenUrl::new_tab(REPO));
                }
            });

            ui.add_space(12.0);

            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 24.0),
                Layout::right_to_left(Align::Center),
                |ui| {
                    if ui.small_button("Close").clicked() {
                        keep_open = false;
                    }
                },
            );

            ui.add_space(4.0);
        });

    if !keep_open {
        *open = false;
    }
}

/// Vertical gradient band with a Cinzel-rendered title centered on it.
fn gradient_header(ui: &mut Ui) {
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), HEADER_H));

    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), HEADER_TOP);
    mesh.colored_vertex(rect.right_top(), HEADER_TOP);
    mesh.colored_vertex(rect.left_bottom(), HEADER_BOT);
    mesh.colored_vertex(rect.right_bottom(), HEADER_BOT);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);

    let painter = ui.painter_at(rect);
    painter.add(Shape::mesh(mesh));

    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, FROST.gamma_multiply(0.6)),
    );

    let title_font = FontId::new(36.0, FontFamily::Name("display".into()));
    painter.text(
        rect.center() + vec2(0.0, -6.0),
        Align2::CENTER_CENTER,
        "RUSTYDEMON",
        title_font,
        Color32::from_gray(235),
    );

    painter.text(
        rect.center() + vec2(0.0, 24.0),
        Align2::CENTER_CENTER,
        "CASC archive explorer",
        FontId::proportional(12.0),
        Color32::from_gray(170),
    );
}

/// Button with a custom-painted background that glows in the given accent
/// color on hover/press. Replaces the default egui button so we can tint
/// per-button rather than globally.
fn glow_button(ui: &mut Ui, label: &str, accent: Color32) -> egui::Response {
    let font = FontId::proportional(14.0);
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), font.clone(), Color32::from_gray(220));
    let padding = vec2(14.0, 8.0);
    let desired = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());

    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();

    let rounding = Rounding::same(8.0);
    let painter = ui.painter();

    let (fill, stroke_color, stroke_w) = if pressed {
        (accent.gamma_multiply(0.55), accent, 1.5)
    } else if hovered {
        (accent.gamma_multiply(0.35), accent, 1.5)
    } else {
        (
            Color32::from_rgb(30, 38, 52),
            Color32::from_rgb(70, 90, 120),
            1.0,
        )
    };

    painter.rect_filled(rect, rounding, fill);
    painter.rect_stroke(rect, rounding, Stroke::new(stroke_w, stroke_color));

    painter.line_segment(
        [
            rect.left_top() + vec2(8.0, 1.0),
            rect.right_top() + vec2(-8.0, 1.0),
        ],
        Stroke::new(
            1.0,
            if hovered {
                accent.gamma_multiply(0.8)
            } else {
                Color32::from_gray(90)
            },
        ),
    );

    let text_color = if hovered {
        Color32::WHITE
    } else {
        Color32::from_gray(220)
    };
    let text_galley = painter.layout_no_wrap(label.to_owned(), font, text_color);
    let text_pos = rect.center() - text_galley.size() / 2.0;
    painter.galley(text_pos, text_galley, text_color);

    response
}
