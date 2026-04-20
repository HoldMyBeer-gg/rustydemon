//! RustyDemon design-system → egui mapping.
//!
//! Translates the tokens in `RustyDemon Design System/colors_and_type.css`
//! into a single `apply(ctx)` call.  Cold gunmetal surfaces, rune-blue for
//! technical data, ember fills for selection / focus / active state.
//!
//! Fonts: the design system names Cinzel Decorative / Inter / JetBrains
//! Mono as the display/body/mono faces, but none of those are shipped
//! locally (only OpenDyslexic, an a11y alternate, is on disk).  We keep
//! eframe's `default_fonts` for now and only reshape colors, strokes,
//! radii, and the type scale.  Dropping the three Google Fonts TTFs into
//! `assets/fonts/` and wiring them through `FontDefinitions` is a clean
//! follow-up — no other theme code needs to change.

use egui::{Color32, Context, FontId, Rounding, Stroke, Style, TextStyle, Visuals};

/// RustyDemon design tokens, transcribed from
/// `RustyDemon Design System/colors_and_type.css`.  Keep in sync if the
/// design system changes — token names match the CSS custom properties.
#[allow(dead_code)]
pub mod rd {
    use egui::Color32;

    // ── Frost / steel (cold neutrals) ───────────────────────────────
    pub const FROST_000: Color32 = Color32::from_rgb(0x05, 0x08, 0x0c);
    pub const FROST_050: Color32 = Color32::from_rgb(0x0a, 0x0e, 0x14);
    pub const FROST_100: Color32 = Color32::from_rgb(0x10, 0x16, 0x1f);
    pub const FROST_200: Color32 = Color32::from_rgb(0x16, 0x1d, 0x28);
    pub const FROST_300: Color32 = Color32::from_rgb(0x1d, 0x26, 0x33);
    pub const FROST_400: Color32 = Color32::from_rgb(0x2a, 0x35, 0x42);
    pub const FROST_500: Color32 = Color32::from_rgb(0x3a, 0x47, 0x57);
    pub const FROST_600: Color32 = Color32::from_rgb(0x5a, 0x6a, 0x7e);
    pub const FROST_700: Color32 = Color32::from_rgb(0x8a, 0x9a, 0xaf);
    pub const FROST_800: Color32 = Color32::from_rgb(0xc2, 0xcb, 0xd6);
    pub const FROST_900: Color32 = Color32::from_rgb(0xe6, 0xec, 0xf3);

    // ── Rune-blue (cold accent — technical data) ────────────────────
    pub const RUNE_300: Color32 = Color32::from_rgb(0x6f, 0xc8, 0xff);
    pub const RUNE_400: Color32 = Color32::from_rgb(0x3a, 0xa7, 0xff);
    pub const RUNE_500: Color32 = Color32::from_rgb(0x1d, 0x8e, 0xe8);

    // ── Ember / rust (brand heart) ──────────────────────────────────
    pub const EMBER_400: Color32 = Color32::from_rgb(0xa8, 0x40, 0x0f);
    pub const EMBER_500: Color32 = Color32::from_rgb(0xd8, 0x5a, 0x12);
    pub const EMBER_600: Color32 = Color32::from_rgb(0xf0, 0x78, 0x20);
    pub const EMBER_700: Color32 = Color32::from_rgb(0xff, 0xa0, 0x48);
    pub const EMBER_800: Color32 = Color32::from_rgb(0xff, 0xd2, 0x8a);
    pub const FG_ON_EMBER: Color32 = Color32::from_rgb(0x1a, 0x0a, 0x04);

    // ── Semantic aliases ────────────────────────────────────────────
    pub const BG_APP: Color32 = FROST_050;
    pub const BG_PANEL: Color32 = FROST_100;
    pub const BG_RAISED: Color32 = FROST_200;
    pub const BG_HOVER: Color32 = FROST_300;
    pub const BG_INSET: Color32 = FROST_000;

    pub const FG_PRIMARY: Color32 = FROST_900;
    pub const FG_SECONDARY: Color32 = FROST_700;
    pub const FG_MUTED: Color32 = FROST_600;

    pub const BORDER_SUBTLE: Color32 = FROST_300;
    pub const BORDER_DEFAULT: Color32 = FROST_400;
    pub const BORDER_STRONG: Color32 = FROST_500;

    pub const DANGER: Color32 = Color32::from_rgb(0xd0, 0x45, 0x3d);
    pub const WARNING: Color32 = Color32::from_rgb(0xe2, 0xa4, 0x3a);
    pub const SUCCESS: Color32 = Color32::from_rgb(0x4a, 0xa5, 0x64);
}

/// Install the RustyDemon visual theme on `ctx`.  Safe to call once at
/// startup — takes immediate effect and persists across frames.
pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();

    visuals.dark_mode = true;
    visuals.override_text_color = Some(rd::FG_PRIMARY);
    visuals.panel_fill = rd::BG_PANEL;
    visuals.window_fill = rd::BG_PANEL;
    visuals.faint_bg_color = rd::BG_RAISED;
    visuals.extreme_bg_color = rd::BG_INSET;
    visuals.code_bg_color = rd::BG_INSET;
    visuals.warn_fg_color = rd::WARNING;
    visuals.error_fg_color = rd::DANGER;
    visuals.hyperlink_color = rd::RUNE_400;

    // Forge-colored selection, never OS blue.  Premultiplied so the
    // fill reads as a warm tint rather than an opaque slab over text.
    visuals.selection.bg_fill = Color32::from_rgba_premultiplied(0x6e, 0x2e, 0x09, 0x80);
    visuals.selection.stroke = Stroke::new(1.0, rd::EMBER_600);

    // Radii: 4px for controls, 6px for windows/menus.  The design
    // system reserves 10px+ for major panels — egui panels ignore
    // Rounding entirely, so the effective ceiling is 6px here.
    let r_sm = Rounding::same(4.0);
    let r_md = Rounding::same(6.0);

    // Non-interactive surfaces (separators, labels, frames).
    visuals.widgets.noninteractive.bg_fill = rd::BG_PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = rd::BG_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, rd::BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, rd::FG_SECONDARY);
    visuals.widgets.noninteractive.rounding = r_sm;
    visuals.widgets.noninteractive.expansion = 0.0;

    // Inactive / default interactive (buttons at rest).
    visuals.widgets.inactive.bg_fill = rd::BG_RAISED;
    visuals.widgets.inactive.weak_bg_fill = rd::BG_RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, rd::BORDER_DEFAULT);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, rd::FG_PRIMARY);
    visuals.widgets.inactive.rounding = r_sm;
    visuals.widgets.inactive.expansion = 0.0;

    // Hover: ember border takes over — the glow IS the affordance.
    visuals.widgets.hovered.bg_fill = rd::BG_HOVER;
    visuals.widgets.hovered.weak_bg_fill = rd::BG_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, rd::EMBER_600);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, rd::FG_PRIMARY);
    visuals.widgets.hovered.rounding = r_sm;
    visuals.widgets.hovered.expansion = 1.0;

    // Active / pressed: ember fill + near-black text.
    visuals.widgets.active.bg_fill = rd::EMBER_500;
    visuals.widgets.active.weak_bg_fill = rd::EMBER_500;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, rd::EMBER_600);
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, rd::FG_ON_EMBER);
    visuals.widgets.active.rounding = r_sm;
    visuals.widgets.active.expansion = 0.0;

    // Open (combobox, collapsing header unfolded).
    visuals.widgets.open.bg_fill = rd::BG_HOVER;
    visuals.widgets.open.weak_bg_fill = rd::BG_HOVER;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, rd::BORDER_STRONG);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, rd::FG_PRIMARY);
    visuals.widgets.open.rounding = r_sm;
    visuals.widgets.open.expansion = 0.0;

    // Windows, menus, popovers.
    visuals.window_rounding = r_md;
    visuals.menu_rounding = r_sm;
    visuals.window_stroke = Stroke::new(1.0, rd::BORDER_DEFAULT);
    visuals.window_shadow.color = Color32::from_rgba_premultiplied(0, 0, 0, 160);
    visuals.popup_shadow.color = Color32::from_rgba_premultiplied(0, 0, 0, 160);

    visuals.slider_trailing_fill = true;
    visuals.striped = false;
    visuals.indent_has_left_vline = false;

    ctx.set_visuals(visuals);

    // Type scale (px sizes from the design system: 11/13/14/16/20).
    let mut style: Style = (*ctx.style()).clone();
    use egui::FontFamily::{Monospace, Proportional};
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(20.0, Proportional));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, Proportional));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::new(13.0, Proportional));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::new(11.0, Proportional));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(13.0, Monospace));

    // 4-px spacing grid, tighter than egui's defaults.
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.menu_margin = egui::Margin::same(6.0);

    ctx.set_style(style);
}

/// Format a section masthead as ENGRAVED UPPERCASE with wide tracking —
/// the design system reserves this for brand/masthead moments.  egui
/// doesn't support CSS letter-spacing; we approximate by spacing the
/// chars with thin U+2009 gaps.
pub fn engraved(text: &str) -> egui::RichText {
    let spaced: String = text
        .to_uppercase()
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\u{2009}");
    egui::RichText::new(spaced)
        .color(rd::FROST_700)
        .size(13.0)
        .strong()
}
