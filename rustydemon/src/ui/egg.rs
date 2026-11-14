//! An Easter egg: enter the Konami code (up up down down left right left
//! right B A, then Enter) while nothing has keyboard focus, and Hermi the
//! hermit crab scuttles along the status bar, stops, clicks a claw twice,
//! and leaves.  Hermi wears the Rusty Demon gear as a shell.
//!
//! It is painted over the bar: nothing moves, nothing is covered for long,
//! and no input is taken.

/// The keys the code is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    B,
    A,
    Start,
}

const SEQUENCE: [Key; 11] = [
    Key::Up,
    Key::Up,
    Key::Down,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::Left,
    Key::Right,
    Key::B,
    Key::A,
    Key::Start,
];

/// How far into the code the last key presses have got.
#[derive(Clone, Copy, Debug, Default)]
pub struct Code(usize);

impl Code {
    /// Take one key press (`None` for any key that is not part of the
    /// code).  True when this press completes the code.
    pub fn press(&mut self, key: Option<Key>) -> bool {
        match key {
            Some(k) if k == SEQUENCE[self.0] => {
                self.0 += 1;
                if self.0 == SEQUENCE.len() {
                    self.0 = 0;
                    return true;
                }
            }
            // A third Up in a row still leaves two Ups in hand.
            Some(Key::Up) if self.0 == 2 => {}
            Some(Key::Up) => self.0 = 1,
            _ => self.0 = 0,
        }
        false
    }
}

/// How long the whole visit lasts, in seconds.
pub const VISIT: f32 = 4.0;

/// What Hermi is doing at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// How far across the bar, 0 at the left edge to 1 at the right.
    pub across: f32,
    /// Which drawing to show.
    pub frame: Frame,
    /// Whether "peench peench" is showing.
    pub speaking: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    WalkA,
    WalkB,
    ClawOpen,
    ClawShut,
}

/// Hermi's pose `t` seconds after the code was entered; `None` once the
/// visit is over.
pub fn pose(t: f32) -> Option<Pose> {
    /// Seconds spent walking in, and again walking out.
    const WALK: f32 = 1.4;
    /// Seconds stopped in the middle: two clicks of 0.6 each.
    const STOP: f32 = VISIT - 2.0 * WALK;
    if !(0.0..VISIT).contains(&t) {
        return None;
    }
    let step = |t: f32| {
        if ((t / 0.15) as u32).is_multiple_of(2) {
            Frame::WalkA
        } else {
            Frame::WalkB
        }
    };
    Some(if t < WALK {
        Pose {
            across: (0.5 * t / WALK).min(0.5),
            frame: step(t),
            speaking: false,
        }
    } else if t < WALK + STOP {
        let click = (t - WALK) % 0.6;
        Pose {
            across: 0.5,
            frame: if click < 0.3 {
                Frame::ClawOpen
            } else {
                Frame::ClawShut
            },
            speaking: true,
        }
    } else {
        Pose {
            // Rounding must not put the first step out behind the stop.
            across: (0.5 + 0.5 * (t - WALK - STOP) / WALK).clamp(0.5, 1.0),
            frame: step(t),
            speaking: false,
        }
    })
}

/// How many art pixels a drawing is raised: every other step lifts the
/// whole crab by one, so the walk bobs.
pub fn lift(frame: Frame) -> f32 {
    if frame == Frame::WalkB {
        1.0
    } else {
        0.0
    }
}

/// Where the top-left corner of a drawing of `size` goes: feet on
/// `ground`, just out of sight to the left at `across` 0 and just out of
/// sight to the right at 1.
pub fn place(across: f32, left: f32, right: f32, ground: f32, size: egui::Vec2) -> egui::Pos2 {
    egui::pos2(
        left - size.x + across * (right - left + size.x),
        ground - size.y,
    )
}

/// What a screen reader is told while Hermi is on screen.
pub const SPOKEN: &str = "Hermi the hermit crab scuttles by and clicks a claw: peench peench.";

/// Listen for the code and, once it is entered, draw Hermi's visit.  Call
/// from inside the status bar.
pub fn show(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique("hermi_easter_egg");
    let now = ctx.input(|i| i.time);
    let (mut code, mut started): (Code, Option<f64>) =
        ctx.data(|d| d.get_temp(id)).unwrap_or_default();

    // Arrows, letters and Enter all mean something to a focused widget, so
    // only listen while nothing has the keyboard.  Keys are read, never
    // taken: whatever else they do still happens.
    if ctx.memory(|m| m.focused().is_none()) {
        let pressed: Vec<egui::Key> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        ..
                    } => Some(*key),
                    _ => None,
                })
                .collect()
        });
        for key in pressed {
            let key = match key {
                egui::Key::ArrowUp => Some(Key::Up),
                egui::Key::ArrowDown => Some(Key::Down),
                egui::Key::ArrowLeft => Some(Key::Left),
                egui::Key::ArrowRight => Some(Key::Right),
                egui::Key::B => Some(Key::B),
                egui::Key::A => Some(Key::A),
                egui::Key::Enter => Some(Key::Start),
                _ => None,
            };
            if code.press(key) {
                started = Some(now);
            }
        }
    } else {
        code = Code::default();
    }

    let visit = started.and_then(|at| pose((now - at) as f32));
    if visit.is_none() {
        started = None;
    }
    ctx.data_mut(|d| d.insert_temp(id, (code, started)));
    let Some(pose) = visit else { return };

    // One node for a screen reader; the status text itself is left alone.
    let (_, node) = ui.allocate_exact_size(egui::Vec2::ZERO, egui::Sense::hover());
    node.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, SPOKEN));

    // Painted over everything, walking along the top edge of the status
    // bar like a ledge, so the status text under it stays readable.
    const PIXEL: f32 = 2.0;
    let rows = pose.frame.rows();
    let size = egui::vec2(rows[0].len() as f32, rows.len() as f32) * PIXEL;
    let screen = ctx.content_rect();
    // The bar's drawn top border.  The panel's clip rect starts higher, in
    // the gap above the bar; the content rect starts one inner margin
    // below the border.
    let ground = ui.max_rect().top() - super::STATUS_BAR_MARGIN_Y;
    let corner = place(pose.across, screen.left(), screen.right(), ground, size);
    let (left, top) = (corner.x, corner.y - lift(pose.frame) * PIXEL);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, id));
    for (y, row) in rows.iter().enumerate() {
        for (x, letter) in row.chars().enumerate() {
            if let Some(fill) = colour(letter) {
                let min = egui::pos2(left + x as f32 * PIXEL, top + y as f32 * PIXEL);
                painter.rect_filled(
                    egui::Rect::from_min_size(min, egui::Vec2::splat(PIXEL)),
                    0.0,
                    fill,
                );
            }
        }
    }
    if pose.speaking {
        painter.text(
            egui::pos2(left + size.x + 6.0, top + size.y * 0.3),
            egui::Align2::LEFT_CENTER,
            "peench peench",
            egui::FontId::proportional(13.0),
            super::theme::rd::EMBER_600,
        );
    }
    ctx.request_repaint();
}

const WALK_A: [&str; 19] = [
    "..............................",
    "..............................",
    "..h.........h.................",
    "..hh.......hh.................",
    "...hs.....sh..................",
    "....s..ss..s..................",
    "...srrrrrrrrs.................",
    "..srRRhhhhRRrs................",
    ".ssrRkkkkkkRrss.w.w...........",
    "..rRkkoooookkRr.e.e...........",
    "..rRkooOOOookRr.c.c...........",
    ".ssRkooOOOookRsccccc...CCC....",
    "..rRkooOOOookRcCCCCCc.CC..C...",
    "..rRkkoooookkcCCCCCCCcCCk.....",
    ".ssrRkkkkkkRrcCCCCCCCcCC..C...",
    "..srRRRRRRRRrsccccccc..CCC....",
    "...srrrrrrrrs.c..c..c.........",
    "....s..ss..s..k..k..k.........",
    "..............................",
];

const WALK_B: [&str; 19] = [
    "..............................",
    "..............................",
    "..h.........h.................",
    "..hh.......hh.................",
    "...hs.....sh..................",
    "....s..ss..s..................",
    "...srrrrrrrrs.................",
    "..srRRhhhhRRrs................",
    ".ssrRkkkkkkRrss.w.w...........",
    "..rRkkoooookkRr.e.e...........",
    "..rRkooOOOookRr.c.c...........",
    ".ssRkooOOOookRsccccc...CCC....",
    "..rRkooOOOookRcCCCCCc.CC..C...",
    "..rRkkoooookkcCCCCCCCcCCk.....",
    ".ssrRkkkkkkRrcCCCCCCCcCC..C...",
    "..srRRRRRRRRrsccccccc..CCC....",
    "...srrrrrrrrs..c..c...........",
    "....s..ss..s...k..k...........",
    "..............................",
];

const UP: [&str; 19] = [
    "..............................",
    "..............................",
    "..h.........h.................",
    "..hh.......hh.........CCC.....",
    "...hs.....sh.........CC..C....",
    "....s..ss..s.........CCk......",
    "...srrrrrrrrs........CC..C....",
    "..srRRhhhhRRrs........CCC.....",
    ".ssrRkkkkkkRrss.w.w...c.......",
    "..rRkkoooookkRr.e.e...c.......",
    "..rRkooOOOookRr.c.c...c.......",
    ".ssRkooOOOookRsccccc..........",
    "..rRkooOOOookRcCCCCCc.........",
    "..rRkkoooookkcCCCCCCCc........",
    ".ssrRkkkkkkRrcCCCCCCCc........",
    "..srRRRRRRRRrsccccccc.........",
    "...srrrrrrrrs.c..c..c.........",
    "....s..ss..s..k..k..k.........",
    "..............................",
];

const SHUT: [&str; 19] = [
    "..............................",
    "..............................",
    "..h.........h.................",
    "..hh.......hh.........CCC.....",
    "...hs.....sh.........CCCCC....",
    "....s..ss..s.........CCkkk....",
    "...srrrrrrrrs........CCCCC....",
    "..srRRhhhhRRrs........CCC.....",
    ".ssrRkkkkkkRrss.w.w...c.......",
    "..rRkkoooookkRr.e.e...c.......",
    "..rRkooOOOookRr.c.c...c.......",
    ".ssRkooOOOookRsccccc..........",
    "..rRkooOOOookRcCCCCCc.........",
    "..rRkkoooookkcCCCCCCCc........",
    ".ssrRkkkkkkRrcCCCCCCCc........",
    "..srRRRRRRRRrsccccccc.........",
    "...srrrrrrrrs.c..c..c.........",
    "....s..ss..s..k..k..k.........",
    "..............................",
];

impl Frame {
    /// The drawing, one string a row; see [`colour`] for the letters.
    pub fn rows(self) -> &'static [&'static str] {
        match self {
            Frame::WalkA => &WALK_A,
            Frame::WalkB => &WALK_B,
            Frame::ClawOpen => &UP,
            Frame::ClawShut => &SHUT,
        }
    }
}

/// The colour a letter in a drawing stands for; `None` for see-through.
pub fn colour(letter: char) -> Option<egui::Color32> {
    let rgb = |r, g, b| Some(egui::Color32::from_rgb(r, g, b));
    match letter {
        'k' => rgb(38, 22, 14),    // outline
        's' => rgb(110, 52, 24),   // gear teeth, shadow
        'r' => rgb(150, 74, 32),   // rust
        'R' => rgb(205, 112, 48),  // rust, lit
        'h' => rgb(236, 160, 84),  // horn and highlight
        'o' => rgb(232, 80, 16),   // molten orb
        'O' => rgb(255, 196, 64),  // orb core
        'c' => rgb(196, 62, 38),   // crab
        'C' => rgb(240, 110, 70),  // crab, lit
        'w' => rgb(250, 250, 245), // eye
        'e' => rgb(20, 16, 16),    // pupil
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enter(code: &mut Code, keys: &[Key]) -> Vec<bool> {
        keys.iter().map(|&k| code.press(Some(k))).collect()
    }

    #[test]
    fn the_code_fires_on_its_last_key_only() {
        let mut code = Code::default();
        let fired = enter(&mut code, &SEQUENCE);
        assert_eq!(fired.iter().filter(|&&f| f).count(), 1);
        assert!(fired[SEQUENCE.len() - 1]);
        // And it can be entered again straight away.
        assert!(enter(&mut code, &SEQUENCE)[SEQUENCE.len() - 1]);
    }

    #[test]
    fn a_wrong_key_starts_the_code_over() {
        let mut code = Code::default();
        enter(&mut code, &SEQUENCE[..6]);
        assert!(!code.press(Some(Key::A)));
        // The rest of the code alone is not enough now.
        assert!(!enter(&mut code, &SEQUENCE[6..]).contains(&true));
        assert!(enter(&mut code, &SEQUENCE)[SEQUENCE.len() - 1]);
    }

    #[test]
    fn any_other_key_starts_the_code_over() {
        let mut code = Code::default();
        enter(&mut code, &SEQUENCE[..10]);
        assert!(!code.press(None));
        assert!(!code.press(Some(Key::Start)));
    }

    /// Holding or repeating Up before the code must not spoil it.
    #[test]
    fn extra_ups_at_the_start_still_count() {
        let mut code = Code::default();
        enter(&mut code, &[Key::Up, Key::Up, Key::Up]);
        assert!(enter(&mut code, &SEQUENCE[2..])[SEQUENCE.len() - 3]);
    }

    #[test]
    fn hermi_walks_in_stops_to_click_and_walks_out() {
        let start = pose(0.0).unwrap();
        assert_eq!(start.across, 0.0);
        assert!(!start.speaking);

        let middle = pose(VISIT / 2.0).unwrap();
        assert_eq!(middle.across, 0.5);
        assert!(middle.speaking);
        assert!(matches!(middle.frame, Frame::ClawOpen | Frame::ClawShut));

        let leaving = pose(VISIT - 0.1).unwrap();
        assert!(leaving.across > 0.9 && leaving.across <= 1.0);
        assert!(!leaving.speaking);

        assert!(pose(VISIT).is_none());
        assert!(pose(-1.0).is_none());
    }

    #[test]
    fn hermi_never_walks_backwards_and_clicks_twice() {
        let poses: Vec<Pose> = (0..400).filter_map(|i| pose(i as f32 / 100.0)).collect();
        assert!(poses.windows(2).all(|w| w[1].across >= w[0].across));
        // Two clicks: the claw goes from open to shut twice.
        let shuts = poses
            .windows(2)
            .filter(|w| w[0].frame == Frame::ClawOpen && w[1].frame == Frame::ClawShut)
            .count();
        assert_eq!(shuts, 2);
        // Legs move while walking.
        assert!(poses.iter().any(|p| p.frame == Frame::WalkA));
        assert!(poses.iter().any(|p| p.frame == Frame::WalkB));
    }

    /// A glide is not a scuttle: every other step the whole crab rises a
    /// pixel.
    #[test]
    fn hermi_bobs_as_it_walks() {
        assert_eq!(lift(Frame::WalkA), 0.0);
        assert_eq!(lift(Frame::WalkB), 1.0);
        assert_eq!(lift(Frame::ClawOpen), 0.0);
        assert_eq!(lift(Frame::ClawShut), 0.0);
    }

    /// Hermi walks along the top edge of the status bar, like a ledge, so
    /// the status text under it stays readable.
    #[test]
    fn hermi_stands_on_the_ground_line_and_starts_and_ends_out_of_sight() {
        let size = egui::vec2(60.0, 38.0);
        let (left, right, ground) = (0.0, 1000.0, 700.0);
        let start = place(0.0, left, right, ground, size);
        assert_eq!(start, egui::pos2(-60.0, 662.0));
        let end = place(1.0, left, right, ground, size);
        assert_eq!(end, egui::pos2(1000.0, 662.0));
        let middle = place(0.5, left, right, ground, size);
        assert_eq!(middle, egui::pos2(470.0, 662.0));
    }

    #[test]
    fn every_drawing_is_the_same_size_and_uses_known_colours() {
        let frames = [Frame::WalkA, Frame::WalkB, Frame::ClawOpen, Frame::ClawShut];
        let (w, h) = (frames[0].rows()[0].len(), frames[0].rows().len());
        for f in frames {
            assert_eq!(f.rows().len(), h, "{f:?}");
            for row in f.rows() {
                assert_eq!(row.len(), w, "{f:?}");
                assert!(
                    row.chars().all(|c| c == '.' || colour(c).is_some()),
                    "{f:?}: {row}"
                );
            }
        }
    }

    /// A key going down and coming back up.  Without the release egui
    /// takes the next press of the same key for a held-key repeat.
    fn tap(key: egui::Key) -> Vec<egui::Event> {
        [true, false]
            .map(|pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            })
            .to_vec()
    }

    /// Run frames of a bare status bar, one key press each, and return the
    /// names published on the last one.
    fn names_after(keys: &[egui::Key], also: impl Fn(&mut egui::Ui)) -> Vec<String> {
        use crate::ui::a11y::testing::frame_in;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut last = None;
        for k in keys.iter().map(|&k| Some(k)).chain([None]) {
            let input = egui::RawInput {
                events: k.map(tap).unwrap_or_default(),
                ..Default::default()
            };
            last = Some(frame_in(&ctx, input, |ui| {
                also(ui);
                show(ui);
            }));
        }
        last.unwrap()
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_owned))
            .collect()
    }

    const KONAMI: [egui::Key; 11] = {
        use egui::Key::*;
        [
            ArrowUp, ArrowUp, ArrowDown, ArrowDown, ArrowLeft, ArrowRight, ArrowLeft, ArrowRight,
            B, A, Enter,
        ]
    };

    #[test]
    fn entering_the_code_brings_hermi_out() {
        let names = names_after(&KONAMI, |_| {});
        assert!(names.iter().any(|n| n == SPOKEN), "{names:?}");
    }

    #[test]
    fn nothing_happens_without_the_whole_code() {
        let names = names_after(&KONAMI[..10], |_| {});
        assert!(!names.iter().any(|n| n == SPOKEN), "{names:?}");
    }

    /// Arrows, letters and Enter all mean something in a text field, so the
    /// code is not listened for while one has the keyboard.
    #[test]
    fn the_code_is_ignored_while_a_text_field_has_focus() {
        let names = names_after(&KONAMI, |ui| {
            let mut text = String::new();
            ui.text_edit_singleline(&mut text).request_focus();
        });
        assert!(!names.iter().any(|n| n == SPOKEN), "{names:?}");
    }
}
