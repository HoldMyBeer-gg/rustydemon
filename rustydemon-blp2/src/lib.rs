//! # rustydemon-blp2
//!
//! A Rust library for reading BLP texture files (BLP0, BLP1, BLP2).
//!
//! BLP is a texture format used in several popular games. This crate parses all
//! three format versions and all standard
//! color encodings, returning decoded pixels as raw RGBA bytes.
//!
//! ## Quick start
//!
//! ```no_run
//! use rustydemon_blp2::BlpFile;
//!
//! let blp = BlpFile::open("texture.blp")?;
//! println!("{}×{}, {} mip level(s)", blp.width, blp.height, blp.mipmap_count());
//!
//! // Decode the base (largest) mipmap — always RGBA, 4 bytes per pixel.
//! let (pixels, w, h) = blp.get_pixels(0)?;
//! assert_eq!(pixels.len(), (w * h * 4) as usize);
//! # Ok::<(), rustydemon_blp2::BlpError>(())
//! ```
//!
//! ## Format support
//!
//! | Format | Encoding | Alpha modes |
//! |--------|----------|-------------|
//! | BLP0 / BLP1 | Palette, DXT1/3/5, ARGB8888, JPEG | 0 / 1 / 4 / 8-bit |
//! | BLP2 | Palette, DXT1/3/5, ARGB8888, JPEG | 0 / 1 / 4 / 8-bit |
//!
//! ## Output format
//!
//! [`BlpFile::get_pixels`] always returns **RGBA** (red first, alpha last),
//! 4 bytes per pixel, in row-major order. This differs from the on-disk
//! ARGB8888 representation (which is BGRA) — the swap is applied automatically.
//!
//! ## Mipmaps
//!
//! BLP files can store up to 16 mipmap levels. Level 0 is always the largest
//! (base) image. Each successive level is half the dimensions of the previous
//! one, clamped to a minimum of 1×1. Requesting a level beyond the available
//! range silently clamps to the last valid level.

pub mod error;

mod blp;
mod dxt;

pub use blp::{BlpFile, ColorEncoding, PixelFormat};
pub use error::BlpError;
