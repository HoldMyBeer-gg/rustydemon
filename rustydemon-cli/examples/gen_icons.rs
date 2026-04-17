//! Generate app icons at all required sizes from the base_icon.png.
//!
//! Usage: cargo run --example gen_icons -- <base_icon.png> <output_dir>

use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: gen_icons <base_icon.png> <output_dir>");
        std::process::exit(1);
    }
    let src_path = &args[1];
    let out_dir = PathBuf::from(&args[2]);

    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let img = image::open(src_path).expect("open base icon");
    eprintln!(
        "Base image: {}x{} {:?}",
        img.width(),
        img.height(),
        img.color()
    );

    // Crop to the largest centered square.
    let cropped = crop_center_square(&img);
    eprintln!("Cropped to {}x{} square", cropped.width(), cropped.height());

    // Add ~12% padding so the gear has breathing room in masked contexts.
    let padded = add_padding(&cropped, 0.12);
    eprintln!(
        "Padded to {}x{} (12% margin)",
        padded.width(),
        padded.height()
    );

    // Save the padded master at 1024.
    let master = padded.resize_exact(1024, 1024, FilterType::Lanczos3);
    master
        .save(out_dir.join("icon_1024.png"))
        .expect("save 1024");
    eprintln!("Saved icon_1024.png");

    // ── Standard square PNGs ─────────────────────────────────────────────
    let square_sizes = [512, 256, 128, 64, 48, 32, 16];
    for &sz in &square_sizes {
        let resized = padded.resize_exact(sz, sz, FilterType::Lanczos3);
        let name = format!("icon_{sz}.png");
        resized.save(out_dir.join(&name)).expect(&name);
        eprintln!("Saved {name}");
    }

    // ── Windows-specific sizes ───────────────────────────────────────────
    // 70px (small tile), 150px (medium tile), 44px (taskbar)
    for &sz in &[70, 150, 44] {
        let resized = padded.resize_exact(sz, sz, FilterType::Lanczos3);
        let name = format!("icon_{sz}.png");
        resized.save(out_dir.join(&name)).expect(&name);
        eprintln!("Saved {name}");
    }

    // Wide tile 310x150
    let wide = make_wide_tile(&padded, 310, 150);
    wide.save(out_dir.join("wide_tile_310x150.png"))
        .expect("wide tile");
    eprintln!("Saved wide_tile_310x150.png");

    // ── Windows .ico (multi-resolution) ──────────────────────────────────
    let ico_sizes = [256, 128, 64, 48, 32, 16];
    write_ico(&padded, &ico_sizes, &out_dir.join("rustydemon.ico"));
    eprintln!("Saved rustydemon.ico");

    // ── macOS .icns sizes (plain square PNGs, icns packing needs a
    //    separate tool — but having the right sizes is 90% of the work) ──
    for &sz in &[1024, 512, 256, 128, 32, 16] {
        let resized = padded.resize_exact(sz, sz, FilterType::Lanczos3);
        let name = format!("icns_{sz}.png");
        resized.save(out_dir.join(&name)).expect(&name);
    }
    // @2x retina variants
    for &(logical, physical) in &[(512, 1024), (256, 512), (128, 256), (16, 32)] {
        let resized = padded.resize_exact(physical, physical, FilterType::Lanczos3);
        let name = format!("icns_{logical}@2x.png");
        resized.save(out_dir.join(&name)).expect(&name);
    }
    eprintln!("Saved macOS icns PNGs");

    // ── iOS / iPadOS ─────────────────────────────────────────────────────
    // These need NO transparency for App Store, but we provide transparent
    // versions — Apple composites the rounded rect mask at display time.
    for &sz in &[180, 167, 152, 120, 87, 76, 58, 40, 29] {
        let resized = padded.resize_exact(sz, sz, FilterType::Lanczos3);
        let name = format!("ios_{sz}.png");
        resized.save(out_dir.join(&name)).expect(&name);
    }
    // 1024 no-transparency version for App Store
    let opaque = make_opaque(&master, [30, 32, 38, 255]);
    opaque
        .save(out_dir.join("ios_appstore_1024.png"))
        .expect("ios appstore");
    eprintln!("Saved iOS icons");

    // ── Android adaptive icon ────────────────────────────────────────────
    // Foreground: 108dp (with safe zone — icon should fit in center 72dp)
    // Background: 108dp solid dark
    let dp_map = [("xxxhdpi", 4), ("xxhdpi", 3), ("xhdpi", 2), ("hdpi", 1)];
    for &(dpi, scale) in &dp_map {
        let fg_sz = 108 * scale;
        // Foreground: padded icon centered in 108dp canvas
        let fg = make_android_foreground(&padded, fg_sz as u32);
        let name = format!("android_fg_{dpi}_{fg_sz}.png");
        fg.save(out_dir.join(&name)).expect(&name);

        // Background: solid dark
        let bg = RgbaImage::from_pixel(fg_sz as u32, fg_sz as u32, image::Rgba([30, 32, 38, 255]));
        let name = format!("android_bg_{dpi}_{fg_sz}.png");
        DynamicImage::ImageRgba8(bg)
            .save(out_dir.join(&name))
            .expect(&name);
    }
    // 512px Play Store icon
    let play = padded.resize_exact(512, 512, FilterType::Lanczos3);
    let play_opaque = make_opaque(&play, [30, 32, 38, 255]);
    play_opaque
        .save(out_dir.join("android_playstore_512.png"))
        .expect("play store");
    eprintln!("Saved Android icons");

    eprintln!("\nDone! All icons in {}", out_dir.display());
}

/// Crop the largest centered square from an image.
fn crop_center_square(img: &DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    img.crop_imm(x, y, side, side)
}

/// Add transparent padding around the image (fraction of the resulting size).
fn add_padding(img: &DynamicImage, fraction: f32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let pad = (w.max(h) as f32 * fraction) as u32;
    let new_w = w + pad * 2;
    let new_h = h + pad * 2;
    let mut canvas = RgbaImage::new(new_w, new_h);
    image::imageops::overlay(&mut canvas, &img.to_rgba8(), pad as i64, pad as i64);
    DynamicImage::ImageRgba8(canvas)
}

/// Create a wide tile: icon centered on a dark background.
fn make_wide_tile(icon: &DynamicImage, w: u32, h: u32) -> DynamicImage {
    let mut canvas = RgbaImage::from_pixel(w, h, image::Rgba([30, 32, 38, 255]));
    let icon_h = (h as f32 * 0.8) as u32;
    let small = icon.resize(icon_h, icon_h, FilterType::Lanczos3);
    let x = ((w - small.width()) / 2) as i64;
    let y = ((h - small.height()) / 2) as i64;
    image::imageops::overlay(&mut canvas, &small.to_rgba8(), x, y);
    DynamicImage::ImageRgba8(canvas)
}

/// Composite the icon onto a solid background (removes transparency).
fn make_opaque(img: &DynamicImage, bg: [u8; 4]) -> DynamicImage {
    let (w, h) = img.dimensions();
    let mut canvas = RgbaImage::from_pixel(w, h, image::Rgba(bg));
    image::imageops::overlay(&mut canvas, &img.to_rgba8(), 0, 0);
    DynamicImage::ImageRgba8(canvas)
}

/// Android adaptive foreground: icon centered in the canvas with the
/// safe zone respected (icon fills ~67% of the canvas).
fn make_android_foreground(icon: &DynamicImage, canvas_size: u32) -> DynamicImage {
    let icon_size = (canvas_size as f32 * 0.667) as u32;
    let small = icon.resize_exact(icon_size, icon_size, FilterType::Lanczos3);
    let mut canvas = RgbaImage::new(canvas_size, canvas_size);
    let offset = ((canvas_size - icon_size) / 2) as i64;
    image::imageops::overlay(&mut canvas, &small.to_rgba8(), offset, offset);
    DynamicImage::ImageRgba8(canvas)
}

/// Write a Windows .ico file containing multiple resolutions.
fn write_ico(src: &DynamicImage, sizes: &[u32], path: &Path) {
    // ICO format: header + directory entries + PNG data blocks.
    let mut entries: Vec<(u32, Vec<u8>)> = Vec::new();
    for &sz in sizes {
        let resized = src.resize_exact(sz, sz, FilterType::Lanczos3);
        let mut png_data = Vec::new();
        resized
            .write_to(&mut std::io::Cursor::new(&mut png_data), ImageFormat::Png)
            .expect("encode PNG for ICO");
        entries.push((sz, png_data));
    }

    let mut out = Vec::new();
    // ICO header: reserved(2) + type=1(2) + count(2)
    out.extend_from_slice(&[0, 0]); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = ICO
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    // Directory entries (16 bytes each), then data.
    let header_size = 6 + entries.len() * 16;
    let mut data_offset = header_size as u32;
    for (sz, png) in &entries {
        let w = if *sz >= 256 { 0u8 } else { *sz as u8 };
        let h = w;
        out.push(w); // width (0 = 256)
        out.push(h); // height
        out.push(0); // color palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes()); // data size
        out.extend_from_slice(&data_offset.to_le_bytes()); // offset
        data_offset += png.len() as u32;
    }
    for (_, png) in &entries {
        out.extend_from_slice(png);
    }

    std::fs::write(path, &out).expect("write ICO");
}
