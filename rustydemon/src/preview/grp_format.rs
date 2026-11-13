//! GRP sprite preview (StarCraft and StarCraft: Remastered).
//!
//! A GRP is a set of frames that share one canvas size.  Each frame is a
//! rectangle inside that canvas, stored a line at a time as runs:
//!
//! ```text
//! header   u16 frame count, u16 canvas width, u16 canvas height
//! frame    u8 x, u8 y, u8 width, u8 height, u32 offset of its lines   (× count)
//! lines    u16 offset of each line, from the start of this table      (× height)
//! a line   0x80 + n   n transparent pixels
//!          0x40 + n   the next byte, n times
//!          n          the next n bytes as they are
//! ```
//!
//! Pixels are palette indices.  The palette is not in the file: the game
//! picks one by context (the map's tileset for units, others for effects and
//! menus).

/// One frame: a rectangle of palette indices placed at `(x, y)` in the canvas.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame {
    pub x: u8,
    pub y: u8,
    pub width: u8,
    pub height: u8,
    /// `width * height` indices, row by row.  `None` is see-through.
    pub pixels: Vec<Option<u8>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Grp {
    pub width: u16,
    pub height: u16,
    pub frames: Vec<Frame>,
}

/// 256 colours, red green blue.
pub type Palette = [[u8; 3]; 256];

impl Grp {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let u16_at = |at: usize| {
            data.get(at..at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .ok_or_else(|| format!("file ends at byte {}, inside its tables", data.len()))
        };
        let count = u16_at(0)? as usize;
        let (width, height) = (u16_at(2)?, u16_at(4)?);
        let table_end = 6 + count * 8;
        if data.len() < table_end {
            return Err(format!(
                "header lists {count} frames, more than a {}-byte file can hold",
                data.len()
            ));
        }

        let entries: Vec<(u8, u8, u8, u8, usize)> = (0..count)
            .map(|i| {
                let e = &data[6 + i * 8..14 + i * 8];
                let start = u32::from_le_bytes([e[4], e[5], e[6], e[7]]) as usize;
                (e[0], e[1], e[2], e[3], start)
            })
            .collect();
        // A tiny run-encoded file can add up like plain pixels by chance, so
        // plain is only believed when the run-encoded reading does not hold.
        let plain = stored_as_plain_pixels(&entries, table_end, data.len())
            && !line_tables_hold(&entries, data);

        let mut frames = Vec::with_capacity(count);
        for (i, &(x, y, w, h, start)) in entries.iter().enumerate() {
            let area = w as usize * h as usize;
            let mut pixels = vec![None; area];
            if plain {
                // Checked in `stored_as_plain_pixels`: the frame is in the file.
                for (slot, &index) in pixels.iter_mut().zip(&data[start..start + area]) {
                    // Colour 0 is the see-through one in interface art.
                    *slot = (index != 0).then_some(index);
                }
            } else {
                for row in 0..h as usize {
                    let line = start
                        .checked_add(row * 2)
                        .and_then(|at| u16_at(at).ok())
                        .and_then(|off| data.get(start + off as usize..))
                        .ok_or_else(|| format!("frame {i}: line {row} lies outside the file"))?;
                    if w > 0 {
                        let out = &mut pixels[row * w as usize..(row + 1) * w as usize];
                        decode_line(line, out);
                    }
                }
            }
            frames.push(Frame {
                x,
                y,
                width: w,
                height: h,
                pixels,
            });
        }
        Ok(Self {
            width,
            height,
            frames,
        })
    }

    /// How many frames go on one row of the sheet.  Units are drawn facing
    /// 17 directions per animation step, so their sheets read best 17 wide:
    /// one step per row.
    pub fn columns(&self) -> usize {
        let n = self.frames.len();
        // A long sheet that is not an exact multiple is still a unit: its
        // 17-wide block is followed by a few frames of it dying.
        if n > 0 && (n.is_multiple_of(17) || n > 17 * 4) {
            17
        } else {
            (n as f64).sqrt().ceil() as usize
        }
    }

    /// The part of the canvas that `frames` draw in, as (left, top, width,
    /// height).  Canvases are sized for the largest unit that might use
    /// them, so most of each one is empty margin.
    fn drawn_area<'a>(
        &self,
        frames: impl Iterator<Item = &'a Frame>,
    ) -> Option<(usize, usize, usize, usize)> {
        let (canvas_w, canvas_h) = (self.width as usize, self.height as usize);
        let mut area: Option<(usize, usize, usize, usize)> = None; // left, top, right, bottom
        for f in frames.filter(|f| f.width > 0 && f.height > 0) {
            let (l, t) = (f.x as usize, f.y as usize);
            let (r, b) = (
                (l + f.width as usize).min(canvas_w),
                (t + f.height as usize).min(canvas_h),
            );
            if l >= r || t >= b {
                continue;
            }
            area = Some(match area {
                None => (l, t, r, b),
                Some((al, at, ar, ab)) => (al.min(l), at.min(t), ar.max(r), ab.max(b)),
            });
        }
        area.map(|(l, t, r, b)| (l, t, r - l, b - t))
    }

    /// Drop frames from the end until the sheet has at most `max_pixels`,
    /// keeping whole rows.  Returns how many frames are left.
    pub fn keep_what_fits(&mut self, max_pixels: usize) -> usize {
        let Some((_, _, cell_w, cell_h)) = self.drawn_area(self.frames.iter()) else {
            return self.frames.len();
        };
        let columns = self.columns().max(1);
        let row_pixels = columns * cell_w * cell_h;
        let rows = (max_pixels / row_pixels.max(1)).max(1);
        // The column count is settled before any frame goes, so the rows
        // that stay look the same as they would in the full sheet.
        if self.frames.len() > rows * columns {
            self.frames.truncate(rows * columns);
        }
        self.frames.len()
    }

    /// Every frame side by side as RGBA, with its width and height.  Each
    /// cell is the part of the canvas that some frame draws in.
    pub fn sheet(&self, palette: &Palette) -> (Vec<u8>, u32, u32) {
        let all: Vec<usize> = (0..self.frames.len()).collect();
        self.compose(&all, self.columns(), palette)
    }

    /// Which frames go on the poster.  For a unit: its first animation
    /// step seen from four sides, a quarter turn apart.  For anything
    /// else: its first four frames.
    pub fn poster_frames(&self) -> Vec<usize> {
        let n = self.frames.len();
        if self.columns() == 17 && n >= 17 {
            vec![0, 4, 8, 12]
        } else {
            (0..n.min(4)).collect()
        }
    }

    /// A picture for a narrow panel: [`poster_frames`](Self::poster_frames)
    /// two to a row, cut to what those frames draw in, and scaled up in
    /// whole steps (so pixels stay square and sharp) as far as
    /// [`POSTER_WIDTH`] allows, eight times at most.
    pub fn poster(&self, palette: &Palette) -> (Vec<u8>, u32, u32) {
        let (small, w, h) = self.compose(&self.poster_frames(), 2, palette);
        let (w, h) = (w as usize, h as usize);
        let scale = (POSTER_WIDTH / w.max(1)).clamp(1, 8);
        if scale == 1 {
            return (small, w as u32, h as u32);
        }
        let mut big = vec![0u8; small.len() * scale * scale];
        for y in 0..h * scale {
            for x in 0..w * scale {
                let from = ((y / scale) * w + x / scale) * 4;
                let to = (y * w * scale + x) * 4;
                big[to..to + 4].copy_from_slice(&small[from..from + 4]);
            }
        }
        (big, (w * scale) as u32, (h * scale) as u32)
    }

    /// The frames `picks`, `columns` to a row, as RGBA with its width and
    /// height.
    fn compose(&self, picks: &[usize], columns: usize, palette: &Palette) -> (Vec<u8>, u32, u32) {
        let shown = picks.iter().filter_map(|&p| self.frames.get(p));
        let Some((left, top, cell_w, cell_h)) = self.drawn_area(shown) else {
            return (vec![0; 4], 1, 1);
        };
        let columns = columns.clamp(1, picks.len());
        let rows = picks.len().div_ceil(columns);
        let (sheet_w, sheet_h) = (columns * cell_w, rows * cell_h);
        let mut rgba = vec![0u8; sheet_w * sheet_h * 4];
        for (i, frame) in picks.iter().filter_map(|&p| self.frames.get(p)).enumerate() {
            let (cell_x, cell_y) = ((i % columns) * cell_w, (i / columns) * cell_h);
            for (p, pixel) in frame.pixels.iter().enumerate() {
                let Some(index) = pixel else { continue };
                let x = frame.x as usize + p % frame.width as usize;
                let y = frame.y as usize + p / frame.width as usize;
                // A frame may claim more room than the canvas has.
                if x >= left + cell_w || y >= top + cell_h {
                    continue;
                }
                let at = ((cell_y + y - top) * sheet_w + cell_x + x - left) * 4;
                let [r, g, b] = palette[*index as usize];
                rgba[at..at + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        (rgba, sheet_w as u32, sheet_h as u32)
    }
}

/// The widest a poster is scaled up to, in pixels.  The details panel is
/// about this wide and draws a picture at its own size when it fits, so a
/// poster within it is shown pixel for pixel, never smoothed down.
pub const POSTER_WIDTH: usize = 224;

/// Which palette file to colour a sprite with, going by where it lives.
/// Doodads sit under their tileset's own folder; everything else borrows
/// Badlands, which every install has.
pub fn palette_path(sprite_path: &str) -> String {
    let parts: Vec<&str> = sprite_path.split(['/', '\\']).collect();
    let tileset = parts
        .iter()
        .position(|p| p.eq_ignore_ascii_case("tileset"))
        .and_then(|i| parts.get(i + 1))
        // The last part is the sprite itself, not a tileset folder.
        .filter(|name| !name.contains('.'))
        .copied()
        .unwrap_or("badlands");
    format!("TileSet/{tileset}.wpe")
}

/// Read a `.wpe` palette: 256 colours of red, green, blue and a spare byte.
pub fn read_palette(wpe: &[u8]) -> Option<Palette> {
    let colours = wpe.get(..1024)?.as_chunks::<4>().0;
    let mut palette = [[0u8; 3]; 256];
    for (slot, c) in palette.iter_mut().zip(colours) {
        *slot = [c[0], c[1], c[2]];
    }
    Some(palette)
}

/// Colours 8 to 15 are the team-colour slots: the game repaints them per
/// player.  `table` is the game's list of which palette colours each
/// player gets, eight per player.  Returns false, changing nothing, when
/// the table has no entry for `player`.
pub fn with_team_colour(palette: &mut Palette, table: &[u8], player: usize) -> bool {
    let Some(picks) = table.get(player * 8..player * 8 + 8) else {
        return false;
    };
    let colours: Vec<[u8; 3]> = picks.iter().map(|&i| palette[i as usize]).collect();
    palette[8..16].copy_from_slice(&colours);
    true
}

/// Shades of grey, for when no palette file can be read.
pub fn grey_palette() -> Palette {
    let mut palette = [[0u8; 3]; 256];
    for (i, slot) in palette.iter_mut().enumerate() {
        *slot = [i as u8; 3];
    }
    palette
}

/// Whether the frames are plain `width * height` pixels packed end to end,
/// the way interface art is stored, and not run-encoded lines.  True only
/// when the frames, in file order, tile the whole file after the frame
/// table exactly; a run-encoded file has line tables in the way and never
/// adds up like that.
fn stored_as_plain_pixels(
    entries: &[(u8, u8, u8, u8, usize)],
    table_end: usize,
    file_len: usize,
) -> bool {
    let mut spans: Vec<(usize, usize)> = entries
        .iter()
        .map(|&(_, _, w, h, start)| (start, w as usize * h as usize))
        .collect();
    spans.sort_unstable();
    spans.dedup(); // identical frames may share their pixels
    let mut at = table_end;
    for (start, len) in spans {
        if start != at {
            return false;
        }
        at += len;
    }
    !entries.is_empty() && at == file_len
}

/// Whether every frame starts with a believable line table: one offset per
/// line, each pointing past the table and inside the frame's own pixels'
/// worth of bytes.  Only asked of files that also add up as plain pixels.
fn line_tables_hold(entries: &[(u8, u8, u8, u8, usize)], data: &[u8]) -> bool {
    entries.iter().all(|&(_, _, w, h, start)| {
        let (table, span) = (h as usize * 2, w as usize * h as usize);
        (0..h as usize).all(|row| {
            data.get(start + row * 2..start + row * 2 + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
                .is_some_and(|off| off >= table && off < span)
        })
    })
}

/// Fill `out` (one line of a frame) from run-encoded `line`.  Stops when
/// either runs out; whatever is left of `out` stays see-through.
fn decode_line(line: &[u8], out: &mut [Option<u8>]) {
    let mut bytes = line.iter().copied();
    let mut at = 0;
    while at < out.len() {
        let Some(op) = bytes.next() else { return };
        let room = out.len() - at;
        if op >= 0x80 {
            at += (op - 0x80) as usize;
        } else if op >= 0x40 {
            let Some(colour) = bytes.next() else { return };
            let n = ((op - 0x40) as usize).min(room);
            out[at..at + n].fill(Some(colour));
            at += n;
        } else {
            for slot in out[at..].iter_mut().take(op as usize) {
                let Some(colour) = bytes.next() else { return };
                *slot = Some(colour);
            }
            at += (op as usize).min(room);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a GRP from `(x, y, width, lines)` frames, each line already
    /// run-encoded.
    pub(crate) fn grp(canvas: (u16, u16), frames: &[(u8, u8, u8, &[&[u8]])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend((frames.len() as u16).to_le_bytes());
        out.extend(canvas.0.to_le_bytes());
        out.extend(canvas.1.to_le_bytes());
        let mut body = Vec::new();
        let body_start = 6 + frames.len() * 8;
        for (x, y, width, lines) in frames {
            out.extend([*x, *y, *width, lines.len() as u8]);
            out.extend(((body_start + body.len()) as u32).to_le_bytes());
            let mut at = lines.len() * 2;
            let mut table = Vec::new();
            for line in *lines {
                table.extend((at as u16).to_le_bytes());
                at += line.len();
            }
            body.extend(table);
            for line in *lines {
                body.extend(*line);
            }
        }
        out.extend(body);
        out
    }

    const T: Option<u8> = None;

    fn palette() -> Palette {
        let mut p = [[0u8; 3]; 256];
        for (i, c) in p.iter_mut().enumerate() {
            *c = [i as u8, 100, 200];
        }
        p
    }

    #[test]
    fn decodes_all_three_kinds_of_run() {
        // Line 0: skip 2, then colour 7 twice.  Line 1: 1 2 3 as they are, skip 1.
        let data = grp(
            (8, 8),
            &[(1, 2, 4, &[&[0x82, 0x42, 7], &[3, 1, 2, 3, 0x81]])],
        );
        let g = Grp::parse(&data).unwrap();
        assert_eq!((g.width, g.height), (8, 8));
        assert_eq!(g.frames.len(), 1);
        let f = &g.frames[0];
        assert_eq!((f.x, f.y, f.width, f.height), (1, 2, 4, 2));
        assert_eq!(
            f.pixels,
            [T, T, Some(7), Some(7), Some(1), Some(2), Some(3), T]
        );
    }

    #[test]
    fn a_line_that_stops_early_is_see_through_to_its_end() {
        let data = grp((4, 1), &[(0, 0, 4, &[&[1, 9]])]);
        let g = Grp::parse(&data).unwrap();
        assert_eq!(g.frames[0].pixels, [Some(9), T, T, T]);
    }

    #[test]
    fn a_run_longer_than_the_line_is_cut_off_not_wrapped() {
        let data = grp((2, 2), &[(0, 0, 2, &[&[0x45, 3], &[1, 4]])]);
        let g = Grp::parse(&data).unwrap();
        assert_eq!(g.frames[0].pixels, [Some(3), Some(3), Some(4), T]);
    }

    /// Interface art is stored without run-encoding: each frame is just
    /// width * height indices, the frames packed end to end.
    #[test]
    fn reads_frames_stored_as_plain_pixels() {
        let mut data = Vec::new();
        data.extend(2u16.to_le_bytes());
        data.extend(4u16.to_le_bytes());
        data.extend(4u16.to_le_bytes());
        data.extend([0, 0, 2, 2]);
        data.extend(22u32.to_le_bytes());
        data.extend([1, 1, 2, 1]);
        data.extend(26u32.to_le_bytes());
        data.extend([1, 2, 3, 4]); // frame 0
        data.extend([0, 6]); // frame 1; colour 0 is see-through
        let g = Grp::parse(&data).unwrap();
        assert_eq!(g.frames[0].pixels, [Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(g.frames[1].pixels, [T, Some(6)]);
    }

    /// A run-encoded file must not be taken for plain pixels because its
    /// sizes happen to be small.
    #[test]
    fn run_encoded_frames_are_not_mistaken_for_plain_pixels() {
        let data = grp((2, 1), &[(0, 0, 2, &[&[0x42, 9]])]);
        let g = Grp::parse(&data).unwrap();
        assert_eq!(g.frames[0].pixels, [Some(9), Some(9)]);
    }

    #[test]
    fn broken_files_are_refused_not_crashed_on() {
        assert!(Grp::parse(&[]).is_err());
        assert!(Grp::parse(&[1, 0, 4, 0]).is_err());
        let good = grp((4, 1), &[(0, 0, 4, &[&[1, 9]])]);
        // Every shorter file either reads or is refused; none may panic.
        for cut in 0..good.len() {
            let _ = Grp::parse(&good[..cut]);
        }
        // A frame whose lines start past the end of the file.
        let mut bad = good.clone();
        bad[10..14].copy_from_slice(&9999u32.to_le_bytes());
        assert!(Grp::parse(&bad).is_err());
        // A header promising far more frames than the file could hold.
        let mut many = good;
        many[0..2].copy_from_slice(&60000u16.to_le_bytes());
        assert!(Grp::parse(&many).is_err());
    }

    #[test]
    fn unit_sheets_are_seventeen_frames_wide() {
        let one: (u8, u8, u8, &[&[u8]]) = (0, 0, 1, &[&[1, 5]]);
        let sheet = |n: usize| Grp::parse(&grp((1, 1), &vec![one; n])).unwrap().columns();
        assert_eq!(sheet(34), 17);
        assert_eq!(sheet(17), 17);
        // A unit with a few death frames after its 17-wide block.
        assert_eq!(sheet(296), 17);
        // Anything else is laid out roughly square.
        assert_eq!(sheet(1), 1);
        assert_eq!(sheet(4), 2);
        assert_eq!(sheet(10), 4);
    }

    #[test]
    fn the_sheet_places_each_frame_in_its_own_cell() {
        // Two frames on a 9x9 canvas, which is mostly empty margin.  Frame 0
        // has one pixel at canvas (4, 5); frame 1 has one at canvas (5, 4).
        // Cells are cut down to the 2x2 area any frame draws in.
        let data = grp((9, 9), &[(4, 5, 1, &[&[1, 50]]), (5, 4, 1, &[&[1, 60]])]);
        let g = Grp::parse(&data).unwrap();
        let (rgba, w, h) = g.sheet(&palette());
        assert_eq!((w, h), (4, 2));
        let at = |x: usize, y: usize| {
            let i = (y * w as usize + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        assert_eq!(at(0, 1), [50, 100, 200, 255]);
        assert_eq!(at(2 + 1, 0), [60, 100, 200, 255]);
        // Everything else is see-through.
        let solid = rgba.chunks(4).filter(|p| p[3] != 0).count();
        assert_eq!(solid, 2);
    }

    #[test]
    fn a_sheet_with_nothing_drawn_is_one_empty_pixel_not_a_crash() {
        let g = Grp {
            width: 8,
            height: 8,
            frames: Vec::new(),
        };
        let (rgba, w, h) = g.sheet(&palette());
        assert_eq!((w, h, rgba.len()), (1, 1, 4));
    }

    #[test]
    fn units_use_the_palette_of_the_tileset_they_sit_in() {
        assert_eq!(
            palette_path("unit/thingy/tileset/Ice/HDradarR.grp"),
            "TileSet/Ice.wpe"
        );
        assert_eq!(
            palette_path("unit\\thingy\\tileset\\jungle\\dd056.grp"),
            "TileSet/jungle.wpe"
        );
        // Anything else borrows one that every install has.
        assert_eq!(
            palette_path("unit/zerg/zergling.grp"),
            "TileSet/badlands.wpe"
        );
    }

    /// The game repaints colours 8 to 15 per player.  Its table lists, for
    /// each player in turn, the eight palette colours to use.
    #[test]
    fn team_colour_slots_take_a_players_colours() {
        let mut p = palette();
        let table: Vec<u8> = (100..132).collect(); // players 0 to 3
        assert!(with_team_colour(&mut p, &table, 1));
        // Player 1 uses table entries 8..16, which name colours 108..116.
        assert_eq!(p[8], [108, 100, 200]);
        assert_eq!(p[15], [115, 100, 200]);
        // Nothing else moves.
        assert_eq!(p[7], [7, 100, 200]);
        assert_eq!(p[16], [16, 100, 200]);
    }

    #[test]
    fn a_table_too_short_for_the_player_changes_nothing() {
        let mut p = palette();
        assert!(!with_team_colour(&mut p, &[1, 2, 3], 0));
        assert!(!with_team_colour(&mut p, &[0; 8], 1));
        assert_eq!(p, palette());
    }

    #[test]
    fn reads_a_palette_file() {
        let mut wpe = vec![0u8; 1024];
        wpe[4..8].copy_from_slice(&[10, 20, 30, 0]);
        let p = read_palette(&wpe).unwrap();
        assert_eq!(p[1], [10, 20, 30]);
        assert!(read_palette(&wpe[..100]).is_none());
    }

    #[test]
    fn a_frame_hanging_off_the_canvas_is_clipped() {
        let data = grp((2, 1), &[(1, 0, 3, &[&[3, 7, 8, 9]])]);
        let g = Grp::parse(&data).unwrap();
        let (rgba, w, h) = g.sheet(&palette());
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, [7, 100, 200, 255]);
    }

    fn frames(n: usize) -> Grp {
        let one: (u8, u8, u8, &[&[u8]]) = (0, 0, 1, &[&[1, 5]]);
        Grp::parse(&grp((1, 1), &vec![one; n])).unwrap()
    }

    /// The details panel is narrow, and a whole unit sheet in it is a grid
    /// of specks.  The picture for the screen is four frames, large.
    #[test]
    fn the_poster_of_a_unit_is_four_facings_a_quarter_turn_apart() {
        assert_eq!(frames(34).poster_frames(), [0, 4, 8, 12]);
        assert_eq!(frames(296).poster_frames(), [0, 4, 8, 12]);
    }

    #[test]
    fn the_poster_of_anything_else_is_its_first_four_frames() {
        assert_eq!(frames(3).poster_frames(), [0, 1, 2]);
        assert_eq!(frames(16).poster_frames(), [0, 1, 2, 3]);
    }

    /// A later frame (an explosion, a death) is often far larger than the
    /// unit standing still, and would shrink the poster to make room.
    #[test]
    fn the_poster_is_cut_to_the_frames_it_shows() {
        let small: (u8, u8, u8, &[&[u8]]) = (10, 10, 2, &[&[2, 5, 5]]);
        let large: (u8, u8, u8, &[&[u8]]) = (0, 0, 30, &[&[0x5e, 7]]);
        let g = Grp::parse(&grp((30, 30), &[small, small, small, small, large])).unwrap();
        // Four 2x1 cells, two to a row, at eight times: no room left for
        // the fifth frame's 30 pixels.
        let (_, w, h) = g.poster(&palette());
        assert_eq!((w, h), (32, 16));
        // The exported sheet still has room for everything.
        let (_, w, _) = g.sheet(&palette());
        assert_eq!(w, 3 * 30);
    }

    #[test]
    fn the_poster_is_scaled_up_in_whole_steps_with_hard_edges() {
        // One 2x1 frame: colour 5, then colour 6.
        let g = Grp::parse(&grp((4, 4), &[(1, 1, 2, &[&[2, 5, 6]])])).unwrap();
        let (rgba, w, h) = g.poster(&palette());
        // Small pictures stop at eight times their size.
        assert_eq!((w, h), (16, 8));
        let at = |x: usize, y: usize| rgba[(y * w as usize + x) * 4];
        assert_eq!((at(0, 0), at(7, 7)), (5, 5));
        assert_eq!((at(8, 0), at(15, 7)), (6, 6));
    }

    #[test]
    fn a_wide_poster_is_not_scaled_at_all() {
        let wide: (u8, u8, u8, &[&[u8]]) = (0, 0, 200, &[&[0x7f, 1, 0x7f, 1]]);
        let g = Grp::parse(&grp((200, 1), &[wide; 4])).unwrap();
        let (_, w, h) = g.poster(&palette());
        assert_eq!((w, h), (400, 2));
    }

    /// The panel draws a picture at its own size when it fits, so a poster
    /// that never exceeds the panel's width is never smoothed down.
    #[test]
    fn the_poster_grows_to_the_panel_width_and_no_further() {
        let cell: (u8, u8, u8, &[&[u8]]) = (0, 0, 30, &[&[0x5e, 7]]);
        let g = Grp::parse(&grp((30, 1), &[cell; 4])).unwrap();
        // Two 30-pixel cells are 60 wide; three times is 180, four would be 240.
        let (_, w, _) = g.poster(&palette());
        assert_eq!(w, 180);
    }

    #[test]
    fn a_huge_sheet_is_cut_to_whole_rows_that_fit() {
        let one: (u8, u8, u8, &[&[u8]]) = (0, 0, 10, &[&[10, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]]);
        let mut g = Grp::parse(&grp((10, 1), &vec![one; 100])).unwrap();
        // 100 frames, 17 per row, 10 pixels each.  Room for 400 pixels is two rows.
        assert_eq!(g.keep_what_fits(400), 34);
        assert_eq!(g.frames.len(), 34);
        // Plenty of room changes nothing.
        assert_eq!(g.keep_what_fits(1_000_000), 34);
    }
}
