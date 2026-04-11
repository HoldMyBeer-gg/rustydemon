use std::io::{self, Read};

use crate::dxt::{self, DxtFlags};
use crate::error::BlpError;

// ── Enumerations ──────────────────────────────────────────────────────────────

/// How the pixel data inside a BLP file is encoded.
///
/// This is stored as a single byte in BLP2 headers and as a 4-byte integer
/// in BLP0/BLP1 headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ColorEncoding {
    /// JPEG-compressed data. A shared JPEG header precedes the per-mipmap data.
    Jpeg        = 0,
    /// Indexed-color palette with up to 256 entries. Alpha is stored separately
    /// after the pixel-index bytes, in 0-, 1-, 4-, or 8-bit-per-pixel form.
    Palette     = 1,
    /// DirectX texture compression (DXT1, DXT3, or DXT5), selected by
    /// [`PixelFormat`] and [`BlpFile::alpha_size`].
    Dxt         = 2,
    /// Uncompressed 32-bit pixels stored on-disk as BGRA. [`BlpFile::get_pixels`]
    /// swaps the channels to RGBA before returning.
    Argb8888    = 3,
    /// Identical to [`Argb8888`](ColorEncoding::Argb8888); present in some older files.
    Argb8888Dup = 4,
}

impl TryFrom<u8> for ColorEncoding {
    type Error = BlpError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::Jpeg),
            1 => Ok(Self::Palette),
            2 => Ok(Self::Dxt),
            3 => Ok(Self::Argb8888),
            4 => Ok(Self::Argb8888Dup),
            _ => Err(BlpError::UnsupportedEncoding(v)),
        }
    }
}

/// The sub-format used when [`ColorEncoding`] is [`Dxt`](ColorEncoding::Dxt).
///
/// Not all variants are used by every game; BLP files in the wild primarily
/// use `Dxt1`, `Dxt3`, and `Dxt5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PixelFormat {
    /// DXT1 (BC1) — 4 bits/pixel, 0- or 1-bit alpha.
    Dxt1        = 0,
    /// DXT3 (BC2) — 8 bits/pixel, explicit 4-bit alpha.
    Dxt3        = 1,
    /// Uncompressed 32-bit ARGB (rarely used as a `PixelFormat` value).
    Argb8888    = 2,
    /// 16-bit ARGB (1-bit alpha, 5-bit per RGB channel).
    Argb1555    = 3,
    /// 16-bit ARGB (4-bit per channel).
    Argb4444    = 4,
    /// 16-bit RGB (5-6-5, no alpha).
    Rgb565      = 5,
    /// 8-bit alpha-only channel.
    A8          = 6,
    /// DXT5 (BC3) — 8 bits/pixel, interpolated 8-bit alpha.
    Dxt5        = 7,
    /// Unspecified / unknown format.
    Unspecified = 8,
    /// 16-bit ARGB (2-bit alpha, 5-bit per RGB channel).
    Argb2565    = 9,
    /// BC5 (two-channel compression, used for normal maps).
    Bc5         = 11,
}

impl TryFrom<u8> for PixelFormat {
    type Error = ();
    fn try_from(v: u8) -> Result<Self, ()> {
        match v {
            0  => Ok(Self::Dxt1),
            1  => Ok(Self::Dxt3),
            2  => Ok(Self::Argb8888),
            3  => Ok(Self::Argb1555),
            4  => Ok(Self::Argb4444),
            5  => Ok(Self::Rgb565),
            6  => Ok(Self::A8),
            7  => Ok(Self::Dxt5),
            8  => Ok(Self::Unspecified),
            9  => Ok(Self::Argb2565),
            11 => Ok(Self::Bc5),
            _  => Err(()),
        }
    }
}

// ── Palette entry ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default)]
struct Rgba8 { r: u8, g: u8, b: u8, a: u8 }

// ── Limits ────────────────────────────────────────────────────────────────────

/// Maximum decoded RGBA byte count we will attempt to allocate (~256 MiB).
///
/// A malicious file could declare enormous dimensions; this ceiling prevents
/// unbounded heap allocation before the mip data bounds check would catch it.
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;

/// Maximum accepted size for the shared JPEG header block.
///
/// Legitimate JPEG headers are a few hundred bytes at most. Values above this
/// threshold indicate a malformed or malicious file.
const MAX_JPEG_HEADER: usize = 64 * 1024;

// ── BlpFile ───────────────────────────────────────────────────────────────────

/// A parsed BLP texture file.
///
/// Load with [`BlpFile::open`] (from disk) or [`BlpFile::from_bytes`] (from
/// an in-memory buffer), then call [`BlpFile::get_pixels`] to decode a mipmap
/// level into raw RGBA bytes.
///
/// The entire file is kept in memory so that any mipmap level can be decoded
/// on demand without re-opening the file.
///
/// # Example
///
/// ```no_run
/// use rustydemon_blp2::BlpFile;
///
/// let blp = BlpFile::open("icon.blp")?;
///
/// for level in 0..blp.mipmap_count() as u32 {
///     let (pixels, w, h) = blp.get_pixels(level)?;
///     println!("mip {level}: {w}×{h} ({} bytes)", pixels.len());
/// }
/// # Ok::<(), rustydemon_blp2::BlpError>(())
/// ```
pub struct BlpFile {
    /// How the pixel data is encoded on disk.
    pub color_encoding:   ColorEncoding,
    /// Bits of alpha precision stored per pixel.
    ///
    /// * `0` — no alpha (all pixels are fully opaque)
    /// * `1` — 1-bit alpha (transparent or opaque)
    /// * `4` — 4-bit alpha (16 levels)
    /// * `8` — 8-bit alpha (256 levels)
    pub alpha_size:       u8,
    /// DXT sub-format, relevant only when
    /// `color_encoding == ColorEncoding::Dxt`.
    pub preferred_format: PixelFormat,
    /// Width of the base (level-0) mipmap in pixels.
    pub width:            u32,
    /// Height of the base (level-0) mipmap in pixels.
    pub height:           u32,
    mip_offsets:          [u32; 16],
    mip_sizes:            [u32; 16],
    palette:              [Rgba8; 256],
    jpeg_header:          Vec<u8>,
    data:                 Vec<u8>,
}

// ── Construction ──────────────────────────────────────────────────────────────

impl BlpFile {
    /// Load and parse a BLP file from disk.
    ///
    /// The entire file is read into memory. Use [`from_bytes`](Self::from_bytes)
    /// if you already have the data in a buffer (e.g. extracted from a CASC archive).
    ///
    /// # Errors
    ///
    /// Returns [`BlpError::Io`] if the file cannot be read, or any parse error
    /// documented on [`from_bytes`](Self::from_bytes).
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, BlpError> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// Parse a BLP file from an in-memory byte buffer.
    ///
    /// The buffer is consumed and stored internally so that mipmap data can be
    /// decoded later without additional allocations.
    ///
    /// # Errors
    ///
    /// | Error | Cause |
    /// |-------|-------|
    /// | [`BlpError::Io`] | Buffer is too short to contain a valid header |
    /// | [`BlpError::InvalidMagic`] | First 4 bytes are not `BLP0`, `BLP1`, or `BLP2` |
    /// | [`BlpError::InvalidFormatVersion`] | BLP2 format version field ≠ 1 |
    /// | [`BlpError::UnsupportedEncoding`] | Color encoding byte is unrecognised |
    /// | [`BlpError::DataTooShort`] | JPEG header size field exceeds 64 KiB |
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, BlpError> {
        parse_header(io::Cursor::new(data))
    }
}

// ── Queries ───────────────────────────────────────────────────────────────────

impl BlpFile {
    /// The number of mipmap levels stored in this file (0–16).
    ///
    /// Counted as the number of leading non-zero entries in the mipmap offset
    /// table. A value of `0` means the file contains no usable image data.
    pub fn mipmap_count(&self) -> usize {
        self.mip_offsets.iter().take_while(|&&o| o != 0).count()
    }

    /// Decode a mipmap level and return `(pixels, width, height)`.
    ///
    /// `pixels` is a `Vec<u8>` containing raw **RGBA** data (red first, alpha
    /// last), 4 bytes per pixel, in row-major left-to-right top-to-bottom order.
    /// Its length is always `width * height * 4`.
    ///
    /// `mipmap_level` is **clamped** to the available range: requesting level 99
    /// on a file with 3 mip levels returns level 2. Level 0 is always the largest
    /// (base) image.
    ///
    /// # Errors
    ///
    /// | Error | Cause |
    /// |-------|-------|
    /// | [`BlpError::NoMipmaps`] | `mipmap_count() == 0` |
    /// | [`BlpError::ImageTooLarge`] | `width × height × 4` overflows or exceeds 256 MiB |
    /// | [`BlpError::OutOfBounds`] | Mipmap offset/size points outside the file buffer |
    /// | [`BlpError::DataTooShort`] | Mipmap slice is too small for the declared dimensions |
    /// | [`BlpError::JpegDecode`] | JPEG data is invalid or corrupt |
    pub fn get_pixels(&self, mipmap_level: u32) -> Result<(Vec<u8>, u32, u32), BlpError> {
        let count = self.mipmap_count();
        if count == 0 {
            return Err(BlpError::NoMipmaps);
        }

        let level = (mipmap_level as usize).min(count - 1);
        let scale = 1u32 << level;
        let w     = (self.width  / scale).max(1);
        let h     = (self.height / scale).max(1);

        // Reject before any allocation attempt: w * h * 4 must not overflow
        // and must not exceed our self-imposed ceiling.
        let byte_count = (w as usize)
            .checked_mul(h as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(BlpError::ImageTooLarge)?;
        if byte_count > MAX_IMAGE_BYTES {
            return Err(BlpError::ImageTooLarge);
        }

        let raw    = self.mip_data(level)?;
        let pixels = self.decode(w, h, raw)?;

        Ok((pixels, w, h))
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

impl BlpFile {
    fn mip_data(&self, level: usize) -> Result<&[u8], BlpError> {
        let off  = self.mip_offsets[level] as usize;
        let size = self.mip_sizes[level]   as usize;
        // checked_add prevents wrapping overflow before the slice bound check.
        let end  = off.checked_add(size).ok_or(BlpError::OutOfBounds)?;
        self.data.get(off..end).ok_or(BlpError::OutOfBounds)
    }

    fn decode(&self, w: u32, h: u32, data: &[u8]) -> Result<Vec<u8>, BlpError> {
        match self.color_encoding {
            ColorEncoding::Jpeg                                   => self.decode_jpeg(data),
            ColorEncoding::Palette                                => self.decode_palette(w, h, data),
            ColorEncoding::Dxt                                    => self.decode_dxt(w, h, data),
            ColorEncoding::Argb8888 | ColorEncoding::Argb8888Dup => Ok(bgra_to_rgba(data)),
        }
    }

    // ── JPEG ──────────────────────────────────────────────────────────────────

    fn decode_jpeg(&self, data: &[u8]) -> Result<Vec<u8>, BlpError> {
        // BLP JPEG files share a single header across all mipmap levels.
        // Prepend it before handing the data to the decoder.
        let combined: Vec<u8> = if self.jpeg_header.is_empty() {
            data.to_vec()
        } else {
            let mut v = Vec::with_capacity(self.jpeg_header.len() + data.len());
            v.extend_from_slice(&self.jpeg_header);
            v.extend_from_slice(data);
            v
        };

        let img = image::load_from_memory(&combined)
            .map_err(|e| BlpError::JpegDecode(e.to_string()))?;
        Ok(img.to_rgba8().into_raw())
    }

    // ── Palette ───────────────────────────────────────────────────────────────

    fn decode_palette(&self, w: u32, h: u32, data: &[u8]) -> Result<Vec<u8>, BlpError> {
        let n_pixels = (w as usize)
            .checked_mul(h as usize)
            .ok_or(BlpError::ImageTooLarge)?;

        // Data layout: [palette_index × n_pixels] [alpha_data]
        if data.len() < n_pixels {
            return Err(BlpError::DataTooShort);
        }

        // Validate that the alpha region is also present before the pixel loop.
        if self.alpha_size != 0 {
            let alpha_bytes: usize = match self.alpha_size {
                1 => (n_pixels + 7) / 8,  // 1 bit/pixel, rounded up
                4 => (n_pixels + 1) / 2,  // 4 bits/pixel, rounded up
                8 => n_pixels,             // 1 byte/pixel
                _ => 0,
            };
            let required = n_pixels.checked_add(alpha_bytes).ok_or(BlpError::DataTooShort)?;
            if data.len() < required {
                return Err(BlpError::DataTooShort);
            }
        }

        let mut out = vec![0u8; n_pixels * 4];
        for i in 0..n_pixels {
            // data[i] is a u8 (0–255) and palette has exactly 256 entries — always in bounds.
            let c = self.palette[data[i] as usize];
            out[i * 4]     = c.r;
            out[i * 4 + 1] = c.g;
            out[i * 4 + 2] = c.b;
            out[i * 4 + 3] = palette_alpha(data, i, n_pixels, self.alpha_size);
        }

        Ok(out)
    }

    // ── DXT ───────────────────────────────────────────────────────────────────

    fn decode_dxt(&self, w: u32, h: u32, data: &[u8]) -> Result<Vec<u8>, BlpError> {
        // DXT variant selection mirrors the original SereniaBLPLib logic:
        //   alpha_size > 1 + preferred_format == Dxt5  →  DXT5
        //   alpha_size > 1 + anything else             →  DXT3
        //   alpha_size == 0 or 1                       →  DXT1
        let flag = if self.alpha_size > 1 {
            if self.preferred_format == PixelFormat::Dxt5 { DxtFlags::Dxt5 } else { DxtFlags::Dxt3 }
        } else {
            DxtFlags::Dxt1
        };

        dxt::decompress_image(w, h, data, flag).ok_or(BlpError::ImageTooLarge)
    }
}

// ── Header parser ─────────────────────────────────────────────────────────────

/// Reads the BLP header from `cur`, then returns a [`BlpFile`] that retains
/// the cursor's inner `Vec<u8>` for on-demand mipmap decoding.
fn parse_header(mut cur: io::Cursor<Vec<u8>>) -> Result<BlpFile, BlpError> {
    const MAGIC_BLP0: u32 = 0x30504c42; // b"BLP0"
    const MAGIC_BLP1: u32 = 0x31504c42; // b"BLP1"
    const MAGIC_BLP2: u32 = 0x32504c42; // b"BLP2"

    let magic = read_u32(&mut cur)?;
    if magic != MAGIC_BLP0 && magic != MAGIC_BLP1 && magic != MAGIC_BLP2 {
        return Err(BlpError::InvalidMagic);
    }

    // BLP0/BLP1 store every header field as a 4-byte integer.
    // BLP2 packs color_encoding, alpha_size, preferred_format, and has_mipmaps
    // into individual bytes, preceded by a 4-byte format version field.
    let (color_encoding, alpha_size, preferred_format, width, height) = match magic {
        MAGIC_BLP0 | MAGIC_BLP1 => {
            let enc   = ColorEncoding::try_from(read_i32(&mut cur)? as u8)?;
            let alpha = read_i32(&mut cur)? as u8;
            let w     = read_i32(&mut cur)? as u32;
            let h     = read_i32(&mut cur)? as u32;
            let pf    = PixelFormat::try_from(read_i32(&mut cur)? as u8)
                            .unwrap_or(PixelFormat::Unspecified);
            let _hm   = read_i32(&mut cur)?; // has_mipmaps flag — unused; we scan the offset table
            (enc, alpha, pf, w, h)
        }
        MAGIC_BLP2 => {
            let ver = read_u32(&mut cur)?;
            if ver != 1 {
                return Err(BlpError::InvalidFormatVersion(ver));
            }
            let enc   = ColorEncoding::try_from(read_u8(&mut cur)?)?;
            let alpha = read_u8(&mut cur)?;
            let pf    = PixelFormat::try_from(read_u8(&mut cur)?)
                            .unwrap_or(PixelFormat::Unspecified);
            let _hm   = read_u8(&mut cur)?;
            let w     = read_i32(&mut cur)? as u32;
            let h     = read_i32(&mut cur)? as u32;
            (enc, alpha, pf, w, h)
        }
        _ => unreachable!(),
    };

    // Mipmap offset table: 16 × u32, zero entries mean "no mip at this level".
    let mut mip_offsets = [0u32; 16];
    for o in &mut mip_offsets { *o = read_u32(&mut cur)?; }

    // Mipmap size table: 16 × u32, parallel to mip_offsets.
    let mut mip_sizes = [0u32; 16];
    for s in &mut mip_sizes   { *s = read_u32(&mut cur)?; }

    let mut palette     = [Rgba8::default(); 256];
    let mut jpeg_header = Vec::new();

    match color_encoding {
        ColorEncoding::Palette => {
            // 256 palette entries, each a little-endian i32 with layout BGRA
            // (blue in the lowest byte, alpha in the highest).
            for c in &mut palette {
                let v = read_i32(&mut cur)?;
                c.b = (v        & 0xFF) as u8;
                c.g = ((v >> 8) & 0xFF) as u8;
                c.r = ((v >>16) & 0xFF) as u8;
                c.a = ((v >>24) & 0xFF) as u8;
            }
        }
        ColorEncoding::Jpeg => {
            // A single JPEG header is shared by all mipmap levels.
            // Guard against maliciously large size claims before allocating.
            let hdr_size = read_i32(&mut cur)? as usize;
            if hdr_size > MAX_JPEG_HEADER {
                return Err(BlpError::DataTooShort);
            }
            let mut hdr = vec![0u8; hdr_size];
            cur.read_exact(&mut hdr)?;
            jpeg_header = hdr;
        }
        _ => {}
    }

    let data = cur.into_inner();

    Ok(BlpFile {
        color_encoding,
        alpha_size,
        preferred_format,
        width,
        height,
        mip_offsets,
        mip_sizes,
        palette,
        jpeg_header,
        data,
    })
}

// ── Alpha extraction ──────────────────────────────────────────────────────────

/// Extracts the alpha value for pixel `index` from a palette mipmap data slice.
///
/// Data layout: `[palette_index × n_pixels][alpha_data]`, where `alpha_start`
/// equals `n_pixels`. The caller must verify that `data` is long enough before
/// calling this function.
///
/// ## Packing formats
///
/// * **1-bit** — 8 pixels packed into each byte, LSB first.
///   Bit set → `0xFF` (opaque), bit clear → `0x00` (transparent).
/// * **4-bit** — 2 pixels per byte. Even pixels use the low nibble (shifted
///   left to the high-nibble position), odd pixels use the high nibble as-is.
///   This matches the original SereniaBLPLib behaviour.
/// * **8-bit** — one byte per pixel, direct value.
/// * **Other** — treated as no alpha; returns `0xFF` (fully opaque).
fn palette_alpha(data: &[u8], index: usize, alpha_start: usize, alpha_size: u8) -> u8 {
    match alpha_size {
        1 => {
            let byte = data[alpha_start + index / 8];
            if byte & (0x01 << (index % 8)) != 0 { 0xFF } else { 0x00 }
        }
        4 => {
            let byte = data[alpha_start + index / 2];
            if index % 2 == 0 { (byte & 0x0F) << 4 } else { byte & 0xF0 }
        }
        8 => data[alpha_start + index],
        _ => 0xFF,
    }
}

// ── Channel swap ──────────────────────────────────────────────────────────────

/// Converts a BGRA buffer to RGBA by swapping the red and blue channels in place.
///
/// ARGB8888 data is stored on disk in BGRA memory order (matching the
/// `System.Drawing` / GDI+ convention). This function normalises it to the
/// RGBA order that [`BlpFile::get_pixels`] always returns.
fn bgra_to_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    for chunk in out.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    out
}

// ── Low-level reader helpers ──────────────────────────────────────────────────

fn read_u8(r: &mut impl Read) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_i32(r: &mut impl Read) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_le_bytes(b))
}
