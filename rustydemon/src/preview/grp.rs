//! GRP sprite preview (StarCraft and StarCraft: Remastered): the sprite's
//! frames as one sheet, coloured with a palette from the game.
//!
//! The file format itself is read by [`super::grp_format`].

use std::sync::Arc;

use super::grp_format::{grey_palette, palette_path, read_palette, with_team_colour, Grp};
use super::{ExportAction, PreviewOutput, PreviewPlugin};

pub struct GrpPreview;

/// The game's table of player colours.
const TEAM_COLOURS: &str = "game/tunit.pcx";

/// The most pixels a sheet may have.  A sheet is kept in memory twice (for
/// the screen and for export), four bytes a pixel.
const MAX_SHEET_PIXELS: usize = 8_000_000;

impl PreviewPlugin for GrpPreview {
    fn name(&self) -> &str {
        ".grp sprite (StarCraft)"
    }

    fn can_preview(&self, filename: &str, data: &[u8]) -> bool {
        let name = filename.to_ascii_lowercase();
        // Remastered's `.dds.grp` files are packed DDS images, a different
        // format that only shares the ending.
        name.ends_with(".grp") && !name.ends_with(".dds.grp") && data.len() >= 6
    }

    fn build(
        &self,
        filename: &str,
        data: &[u8],
        ctx: &egui::Context,
        siblings: &super::SiblingFetcher<'_>,
    ) -> PreviewOutput {
        let mut out = PreviewOutput::new();
        let mut grp = match Grp::parse(data) {
            Ok(g) => g,
            Err(e) => {
                out.text = Some(format!("GRP sprite could not be read: {e}"));
                return out;
            }
        };

        let palette_file = palette_path(filename);
        let found = (siblings.by_name)(&palette_file).and_then(|wpe| read_palette(&wpe));
        let colours = match found {
            Some(_) => format!("Colours from {palette_file}."),
            None => format!("{palette_file} could not be read, so this is in shades of grey."),
        };
        let mut palette = found.unwrap_or_else(grey_palette);

        // Paint the team-colour slots as player one (red), as in a match.
        // The table is a one-row image of palette colours, eight a player.
        let team = found.is_some()
            && (siblings.by_name)(TEAM_COLOURS)
                .and_then(|pcx| super::pcx::decode_pcx_indices(&pcx).ok())
                .is_some_and(|(table, ..)| with_team_colour(&mut palette, &table, 0));

        let total = grp.frames.len();
        let shown = grp.keep_what_fits(MAX_SHEET_PIXELS);
        let (sheet_w, sheet_h) = {
            let (_, w, h) = grp.sheet(&palette);
            (w, h)
        };
        let on_poster = grp.poster_frames().len();
        let (pixels, w, h) = grp.poster(&palette);

        let mut text = format!(
            "GRP sprite  •  {total} frame{}  •  canvas {}×{}\n\
             Showing {on_poster} of them, enlarged. Export As PNG writes the whole \
             sheet: {sheet_w}×{sheet_h}, {} per row.\n{colours}\n\
             {team_note}\n\
             The game picks other palettes for menus and effects, so those \
             may differ in play.",
            if total == 1 { "" } else { "s" },
            grp.width,
            grp.height,
            grp.columns().max(1),
            team_note = if team {
                "Team colour: player one (red)."
            } else {
                "Team-colour parts are left in the palette's placeholder colours."
            },
        );
        if shown < total {
            text.push_str(&format!(
                "\n\nThe exported sheet stops at {shown} frames; all of them would be too large."
            ));
        }
        out.text = Some(text);

        let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
        // Hard edges both ways: a sprite a little wider than the panel is
        // drawn smaller, and smoothing that blurs every pixel of it.
        let hard_edges = egui::TextureOptions::NEAREST;
        out.texture = Some(ctx.load_texture("grp_preview", image, hard_edges));
        out.texture_pixels = Some((pixels, w, h));
        out.extra_exports.push(ExportAction {
            label: "Export As PNG",
            default_extension: "png",
            filter_name: "PNG image",
            build: Arc::new(move |data, _path| {
                let mut grp = Grp::parse(data)?;
                grp.keep_what_fits(MAX_SHEET_PIXELS);
                let (pixels, w, h) = grp.sheet(&palette);
                crate::preview::encode_png(&pixels, w, h)
            }),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::grp_format::tests::grp;
    use super::*;

    fn preview(name: &str, data: &[u8], palette: Option<Vec<u8>>) -> PreviewOutput {
        let by_name = |path: &str| match path {
            "TileSet/badlands.wpe" => palette.clone(),
            "game/tunit.pcx" => None,
            other => panic!("unexpected file asked for: {other}"),
        };
        let none = |_: u32| None;
        let no_info = |_: &str| None;
        let no_skins = || None;
        let no_repo = || None;
        let siblings = super::super::SiblingFetcher {
            by_name: &by_name,
            by_fdid: &none,
            texture_info: &no_info,
            monster_skins: &no_skins,
            messiah_repo: &no_repo,
        };
        GrpPreview.build(name, data, &egui::Context::default(), &siblings)
    }

    #[test]
    fn the_preview_is_the_sheet_in_the_games_colours() {
        let data = grp((4, 4), &[(1, 1, 2, &[&[2, 5, 6]])]);
        let mut wpe = vec![0u8; 1024];
        wpe[5 * 4..5 * 4 + 3].copy_from_slice(&[200, 10, 20]);
        let out = preview("unit/zerg/zergling.grp", &data, Some(wpe));
        let (rgba, w, h) = out.texture_pixels.expect("an image");
        // The picture on screen is the poster: the frame scaled up.
        assert_eq!((w, h), (16, 8));
        assert_eq!(&rgba[..4], [200, 10, 20, 255]);
        let text = out.text.unwrap();
        assert!(text.contains("1 frame"), "{text}");
        assert!(text.contains("TileSet/badlands.wpe"), "{text}");
        assert!(out.extra_exports.iter().any(|e| e.label == "Export As PNG"));
    }

    #[test]
    fn without_a_palette_file_the_preview_is_grey_and_says_so() {
        let data = grp((4, 4), &[(1, 1, 2, &[&[2, 5, 6]])]);
        let out = preview("unit/zerg/zergling.grp", &data, None);
        let (rgba, _, _) = out.texture_pixels.expect("an image");
        assert_eq!(&rgba[..4], [5, 5, 5, 255]);
        assert!(out.text.unwrap().contains("shades of grey"));
    }

    #[test]
    fn a_broken_sprite_gets_a_message_not_an_image() {
        let out = preview("unit/zerg/zergling.grp", &[1, 0, 4, 0, 4, 0], None);
        assert!(out.texture_pixels.is_none());
        assert!(out.text.unwrap().contains("GRP sprite could not be read"));
    }

    #[test]
    fn hd_containers_that_share_the_ending_are_left_to_others() {
        let data = grp((4, 4), &[(1, 1, 2, &[&[2, 5, 6]])]);
        assert!(GrpPreview.can_preview("unit/zerg/Zergling.GRP", &data));
        assert!(!GrpPreview.can_preview("SD/unit/cmdicons/cmdicons.dds.grp", &data));
        assert!(!GrpPreview.can_preview("unit/zerg/zergling.pcx", &data));
    }
}
