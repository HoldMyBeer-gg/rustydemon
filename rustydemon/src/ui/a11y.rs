//! Accessible names and roles for widgets whose visible text is not what a
//! screen reader should say.
//!
//! egui publishes a widget's visible text as its accessible name. That is
//! wrong where the text is decoration (letter-spaced headings, glyph
//! prefixes, values cut short with an ellipsis) and missing where there is
//! no text at all (text boxes, dropdowns, images). These helpers set the
//! name on the widget's AccessKit node after egui has built it.

use egui::accesskit::{Live, Role};

/// Replace the accessible name of the widget with id `id`.
pub fn name(ctx: &egui::Context, id: egui::Id, name: &str) {
    ctx.accesskit_node_builder(id, |node| node.set_label(name));
}

/// Draw `shown` and publish `spoken` for it, as a single node of `role`.
///
/// For text whose on-screen form is decoration: letter-spaced headings, a
/// value cut short with an ellipsis, a bullet or glyph prefix. An egui label
/// would publish the drawn characters themselves (as its value, and again as
/// text runs), so the text is painted instead and the one node says the
/// plain words.
pub fn decorated(
    ui: &mut egui::Ui,
    shown: impl Into<egui::WidgetText>,
    role: Role,
    spoken: &str,
) -> egui::Response {
    let galley = shown.into().into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        ui.available_width(),
        egui::TextStyle::Body,
    );
    let (rect, resp) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .galley(rect.min, galley, ui.visuals().text_color());
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(role, true, spoken));
    resp
}

/// Publish the widget as an image described by `name`.
pub fn image(ctx: &egui::Context, id: egui::Id, name: &str) {
    ctx.accesskit_node_builder(id, |node| {
        node.set_role(Role::Image);
        node.set_label(name);
    });
}

/// Mark a node as a status line: a screen reader announces a change to it
/// when it is next idle, without moving focus there.
pub fn live(ctx: &egui::Context, id: egui::Id) {
    ctx.accesskit_node_builder(id, |node| node.set_live(Live::Polite));
}

/// A button whose visible text carries a glyph or shorthand (`A+`,
/// `▶ Play`) and whose spoken name is plain words.
pub fn button(
    ui: &mut egui::Ui,
    visible: impl Into<egui::WidgetText>,
    spoken: &str,
) -> egui::Response {
    button_enabled(ui, true, visible, spoken)
}

/// [`button`], greyed out when `enabled` is false.
pub fn button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    visible: impl Into<egui::WidgetText>,
    spoken: &str,
) -> egui::Response {
    let resp = ui.add_enabled(enabled, egui::Button::new(visible));
    name(ui.ctx(), resp.id, spoken);
    resp
}

#[cfg(test)]
pub(crate) mod testing {
    use egui::accesskit::{Node, Role};

    /// Draw with AccessKit on, under the app's theme, and return every node.
    ///
    /// Two frames: some containers (`Grid`) only measure on their first
    /// frame and publish nothing until the second.
    pub fn nodes(mut draw: impl FnMut(&mut egui::Ui)) -> Vec<Node> {
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        ctx.enable_accesskit();
        frame(&ctx, &mut draw);
        frame(&ctx, &mut draw)
            .nodes
            .into_iter()
            .map(|(_, n)| n)
            .collect()
    }

    /// One frame of `draw`, returning the accessibility tree it published.
    pub fn frame(
        ctx: &egui::Context,
        draw: impl FnMut(&mut egui::Ui),
    ) -> egui::accesskit::TreeUpdate {
        let mut out = ctx.run_ui(egui::RawInput::default(), draw);
        // egui panics if texture changes are dropped without being applied.
        out.textures_delta.clear();
        out.platform_output
            .accesskit_update
            .expect("accesskit enabled")
    }

    /// One frame of `draw` with no accessibility output wanted.
    pub fn frame_plain(ctx: &egui::Context, draw: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(egui::RawInput::default(), draw);
        out.textures_delta.clear();
    }

    /// `(role, text)` of every node a screen reader has words for: its
    /// label, or for static text (which egui publishes as a value) its value.
    pub fn named(nodes: &[Node]) -> Vec<(Role, String)> {
        nodes
            .iter()
            .filter_map(|n| {
                n.label()
                    .or_else(|| n.value())
                    .map(|text| (n.role(), text.to_owned()))
            })
            .collect()
    }

    /// Roles of the nodes that have no name.
    pub fn unnamed_roles(nodes: &[Node]) -> Vec<Role> {
        nodes
            .iter()
            .filter(|n| n.label().is_none_or(|s| s.trim().is_empty()))
            .map(|n| n.role())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{named, nodes, unnamed_roles};
    use egui::accesskit::{Live, Role};

    fn has(named: &[(Role, String)], role: Role, name: &str) -> bool {
        named.iter().any(|(r, n)| *r == role && n == name)
    }

    #[test]
    fn section_label_is_a_heading_with_plain_words() {
        let n = named(&nodes(|ui| {
            crate::ui::theme::section_label(ui, "Texture · 64×64");
        }));
        assert!(has(&n, Role::Heading, "Texture · 64×64"), "{n:?}");
        assert!(
            !n.iter().any(|(_, name)| name.contains('\u{2009}')),
            "letter-spacing leaked into a name: {n:?}"
        );
    }

    #[test]
    fn panel_header_is_a_heading_with_plain_words() {
        let n = named(&nodes(|ui| {
            crate::ui::theme::panel_header(ui, "File Browser", Some("wowf"));
        }));
        assert!(has(&n, Role::Heading, "File Browser"), "{n:?}");
        assert!(
            !n.iter().any(|(_, name)| name.contains('\u{2009}')),
            "{n:?}"
        );
    }

    #[test]
    fn meta_row_publishes_the_whole_value_when_it_is_cut_short_on_screen() {
        let long = "inv_misc_questionmark_with_a_very_long_name.blp";
        let n = named(&nodes(|ui| {
            egui::Grid::new("g").show(ui, |ui| {
                crate::ui::preview::meta_row(ui, "Name:", long);
            });
        }));
        assert!(n.iter().any(|(_, name)| name == long), "{n:?}");
        assert!(!n.iter().any(|(_, name)| name.ends_with('…')), "{n:?}");
    }

    #[test]
    fn meta_row_survives_a_long_value_with_multibyte_characters() {
        // 30 two-byte characters: cutting at byte 24 is fine, but an odd
        // offset would land inside a character and panic.
        let long = format!("a{}", "é".repeat(30));
        let n = named(&nodes(|ui| {
            egui::Grid::new("g").show(ui, |ui| {
                crate::ui::preview::meta_row(ui, "Name:", &long);
            });
        }));
        assert!(n.iter().any(|(_, name)| *name == long), "{n:?}");
    }

    #[test]
    fn search_box_has_a_name() {
        let mut text = String::new();
        let all = nodes(|ui| {
            crate::ui::search_box(ui, &mut text);
        });
        assert!(
            has(&named(&all), Role::TextInput, "Search files"),
            "{:?}",
            named(&all)
        );
    }

    #[test]
    fn zoom_buttons_say_what_they_do() {
        let n = named(&nodes(|ui| {
            crate::ui::zoom_buttons(ui);
        }));
        assert!(has(&n, Role::Button, "Increase text size"), "{n:?}");
        assert!(has(&n, Role::Button, "Decrease text size"), "{n:?}");
    }

    #[test]
    fn status_line_is_one_polite_live_region_with_the_whole_text() {
        let all = nodes(|ui| {
            crate::ui::status_line(ui, "Loading listfile /tmp/list.csv…");
        });
        let status: Vec<_> = all.iter().filter(|n| n.role() == Role::Status).collect();
        assert_eq!(status.len(), 1, "{:?}", named(&all));
        assert_eq!(status[0].label(), Some("Loading listfile /tmp/list.csv…"));
        assert_eq!(status[0].live(), Some(Live::Polite));
        // The accent word must not also be published on its own.
        assert!(
            !named(&all).iter().any(|(_, n)| n == "Loading"),
            "{:?}",
            named(&all)
        );
    }

    #[test]
    fn viewer_dropdown_has_a_name() {
        let names = vec!["BLP".to_string(), "Text".to_string()];
        let n = named(&nodes(|ui| {
            crate::ui::preview::viewer_combo(ui, None, &names);
        }));
        assert!(has(&n, Role::ComboBox, "Viewer"), "{n:?}");
    }

    #[test]
    fn preview_text_has_a_name() {
        let all = nodes(|ui| {
            crate::ui::preview::text_block(ui, "## Title: x", "File contents");
        });
        assert!(
            named(&all).iter().any(|(_, n)| n == "File contents"),
            "{:?}",
            named(&all)
        );
        assert!(unnamed_roles(&all)
            .iter()
            .all(|r| !matches!(r, Role::TextInput | Role::MultilineTextInput)));
    }

    #[test]
    fn texture_preview_is_an_image_with_its_size() {
        let n = named(&nodes(|ui| {
            let tex = ui.ctx().load_texture(
                "t",
                egui::ColorImage::filled([64, 32], egui::Color32::RED),
                egui::TextureOptions::LINEAR,
            );
            crate::ui::preview::texture_image(ui, &tex);
        }));
        assert!(
            has(&n, Role::Image, "Texture preview, 64 by 32 pixels"),
            "{n:?}"
        );
    }

    #[test]
    fn glyph_button_speaks_plain_words() {
        let n = named(&nodes(|ui| {
            super::button(ui, "▶ Play", "Play");
        }));
        assert!(has(&n, Role::Button, "Play"), "{n:?}");
        assert!(!n.iter().any(|(_, name)| name.contains('▶')), "{n:?}");
    }

    /// Text a widget shows is also its accessible *value*. A renamed label
    /// must not keep the on-screen decoration there.
    #[test]
    fn renamed_text_does_not_keep_the_decorated_value() {
        let all = nodes(|ui| {
            crate::ui::theme::section_label(ui, "Viewer");
            egui::Grid::new("g").show(ui, |ui| {
                crate::ui::preview::meta_row(ui, "Name:", "a_very_long_file_name_indeed_yes.blp");
            });
        });
        for n in &all {
            let v = n.value().unwrap_or_default();
            assert!(!v.contains('\u{2009}') && !v.ends_with('…'), "value {v:?}");
        }
    }

    #[test]
    fn empty_game_pill_says_what_it_means() {
        let n = named(&nodes(|ui| {
            crate::ui::theme::panel_header(ui, "File Browser", Some("—"));
        }));
        assert!(n.iter().any(|(_, s)| s == "No game loaded"), "{n:?}");
        assert!(!n.iter().any(|(_, s)| s == "—"), "{n:?}");
    }
}
