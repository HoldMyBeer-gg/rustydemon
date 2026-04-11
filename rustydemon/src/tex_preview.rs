//! D4 .tex texture preview — attempts to decode raw BC-compressed data.
//!
//! D4 .tex files contain raw block-compressed pixel data without a header.
//! We guess the dimensions from the file size and try multiple BC formats
//! until we get a plausible decode.

/// Attempt to decode a .tex file into RGBA pixels.
///
/// `filename` is used to hint at the format (e.g. `_normal` → BC5).
/// Returns `(rgba_pixels, width, height, format_name)` on success.
pub fn decode_tex(data: &[u8], filename: &str) -> Option<(Vec<u8>, u32, u32, &'static str)> {
    // Skip files that are too small or all zeros (encrypted).
    if data.len() < 64 {
        return None;
    }
    if data.iter().take(64).all(|&b| b == 0) {
        return None; // Likely encrypted
    }

    let lower = filename.to_lowercase();

    // Order formats based on filename hints.
    type DecodeFn = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;
    let all_formats: &[(&str, usize, DecodeFn)] = &[
        ("BC7", 1, texture2ddecoder::decode_bc7),
        ("BC5", 1, texture2ddecoder::decode_bc5),
        ("BC3", 1, texture2ddecoder::decode_bc3),
        ("BC1", 2, texture2ddecoder::decode_bc1),
        ("BC4", 2, texture2ddecoder::decode_bc4),
    ];

    // Build priority order based on filename.
    let priority: Vec<usize> = if lower.contains("_normal") {
        // Normal maps → BC5 first, then BC7
        vec![1, 0, 2, 3, 4]
    } else if lower.contains("_rough")
        || lower.contains("_metallic")
        || lower.contains("_ao")
        || lower.contains("_alpha")
        || lower.contains("_opacity")
        || lower.contains("_mask")
    {
        // Single-channel → BC4 first, then BC1
        vec![4, 3, 0, 1, 2]
    } else {
        // Color/diffuse/emissive → BC7 first
        vec![0, 2, 3, 1, 4]
    };

    for &idx in &priority {
        let &(name, size_factor, decode_fn) = &all_formats[idx];
        let pixel_count = if size_factor == 2 {
            data.len() * 2 // BC1/BC4: 0.5 bytes per pixel
        } else {
            data.len() // BC7/BC3/BC5: 1 byte per pixel
        };

        // Try multiple dimension candidates.
        for (w, h) in guess_all_dimensions(pixel_count) {
            let mut rgba_u32 = vec![0u32; w * h];
            if decode_fn(data, w, h, &mut rgba_u32).is_ok() && looks_valid(&rgba_u32, w) {
                let rgba = u32_to_rgba(&rgba_u32);
                return Some((rgba, w as u32, h as u32, name));
            }
        }
    }

    None
}

/// Return all plausible power-of-2 dimension pairs for a given pixel count.
/// Prefers square, then common aspect ratios.
fn guess_all_dimensions(pixel_count: usize) -> Vec<(usize, usize)> {
    let mut results = Vec::new();

    // Try all power-of-2 width/height combos where w*h == pixel_count.
    for w_exp in 6..14u32 {
        let w = 1usize << w_exp;
        if !pixel_count.is_multiple_of(w) {
            continue;
        }
        let h = pixel_count / w;
        if h < 64 || !h.is_power_of_two() {
            continue;
        }
        // Prefer square → put it first.
        if w == h {
            results.insert(0, (w, h));
        } else if w <= h * 2 && h <= w * 2 {
            // Reasonable aspect ratio (up to 2:1).
            results.push((w, h));
        }
    }

    results
}

/// Validate decoded image: real textures have spatial coherence (adjacent
/// pixels tend to be similar). Noise from a wrong BC format has high
/// pixel-to-pixel variation. Checks both horizontal and vertical adjacency.
fn looks_valid(pixels: &[u32], width: usize) -> bool {
    if pixels.len() < 256 || width == 0 {
        return false;
    }

    // Check that we have some distinct values (not all one color).
    let mut distinct = std::collections::HashSet::new();
    let step = (pixels.len() / 64).max(1);
    for i in (0..pixels.len()).step_by(step).take(64) {
        distinct.insert(pixels[i]);
    }
    if distinct.len() < 3 {
        return false;
    }

    let pixel_diff = |a: u32, b: u32| -> u32 {
        let dr = ((a & 0xFF) as i32 - (b & 0xFF) as i32).unsigned_abs();
        let dg = (((a >> 8) & 0xFF) as i32 - ((b >> 8) & 0xFF) as i32).unsigned_abs();
        let db = (((a >> 16) & 0xFF) as i32 - ((b >> 16) & 0xFF) as i32).unsigned_abs();
        dr + dg + db
    };

    let mut smooth = 0u32;
    let mut total = 0u32;
    let sample_step = (pixels.len() / 400).max(1);

    for i in (0..pixels.len()).step_by(sample_step).take(400) {
        // Horizontal neighbor.
        if i + 1 < pixels.len() && (i % width) + 1 < width {
            if pixel_diff(pixels[i], pixels[i + 1]) < 60 {
                smooth += 1;
            }
            total += 1;
        }
        // Vertical neighbor.
        if i + width < pixels.len() {
            if pixel_diff(pixels[i], pixels[i + width]) < 60 {
                smooth += 1;
            }
            total += 1;
        }
    }

    if total == 0 {
        return false;
    }

    // Real textures: 50-90% smooth. Noise: 5-25%.
    let ratio = smooth as f32 / total as f32;
    ratio > 0.45
}

/// Convert u32 RGBA (0xAABBGGRR) to byte array [R, G, B, A, ...].
fn u32_to_rgba(pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for &p in pixels {
        out.push((p & 0xFF) as u8); // R
        out.push(((p >> 8) & 0xFF) as u8); // G
        out.push(((p >> 16) & 0xFF) as u8); // B
        out.push(((p >> 24) & 0xFF) as u8); // A
    }
    out
}
