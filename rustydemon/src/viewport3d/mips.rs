//! CPU mip chain for material textures, so a large texture drawn small
//! averages its texels instead of shimmering.

/// One mip level: RGBA bytes, width, height.
pub type Level = (Vec<u8>, u32, u32);

/// Level 0 is the texture itself; each next level averages 2x2 texels of
/// the one before, halving each side (never below 1) until 1x1.
pub fn mip_chain(rgba: &[u8], width: u32, height: u32) -> Vec<Level> {
    let mut levels = vec![(rgba.to_vec(), width, height)];
    while let Some((src, w, h)) = levels.last().filter(|l| l.1 > 1 || l.2 > 1) {
        let (w, h) = (*w as usize, *h as usize);
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut out = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                let (x0, y0) = ((2 * x).min(w - 1), (2 * y).min(h - 1));
                let (x1, y1) = ((2 * x + 1).min(w - 1), (2 * y + 1).min(h - 1));
                for c in 0..4 {
                    let at = |px: usize, py: usize| u32::from(src[(py * w + px) * 4 + c]);
                    let sum = at(x0, y0) + at(x1, y0) + at(x0, y1) + at(x1, y1);
                    out[(y * nw + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        levels.push((out, nw as u32, nh as u32));
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_texture_halves_down_to_one_texel() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let levels = mip_chain(&rgba, 4, 4);
        let sizes: Vec<(u32, u32)> = levels.iter().map(|l| (l.1, l.2)).collect();
        assert_eq!(sizes, vec![(4, 4), (2, 2), (1, 1)]);
        assert!(levels.iter().all(|l| l.0.len() == (l.1 * l.2 * 4) as usize));
    }

    #[test]
    fn each_level_averages_the_texels_above_it() {
        // 2x2: black, white / white, black  ->  1x1 mid grey.
        let rgba = [
            0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
        ];
        let levels = mip_chain(&rgba, 2, 2);
        assert_eq!(levels[1].0, vec![128, 128, 128, 255]);
    }

    #[test]
    fn odd_and_narrow_sizes_still_reach_one_texel() {
        let sizes = |w: u32, h: u32| -> Vec<(u32, u32)> {
            mip_chain(&vec![9u8; (w * h * 4) as usize], w, h)
                .iter()
                .map(|l| (l.1, l.2))
                .collect()
        };
        assert_eq!(sizes(4, 1), vec![(4, 1), (2, 1), (1, 1)]);
        assert_eq!(sizes(3, 3), vec![(3, 3), (1, 1)]);
        assert_eq!(sizes(1, 1), vec![(1, 1)]);
    }
}
