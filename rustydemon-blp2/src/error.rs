use thiserror::Error;

/// All errors that can be returned by this crate.
#[derive(Debug, Error)]
pub enum BlpError {
    /// An I/O error occurred while reading the file or buffer.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The first four bytes of the file are not a recognised BLP magic number
    /// (`BLP0`, `BLP1`, or `BLP2`).
    #[error("invalid BLP magic number")]
    InvalidMagic,

    /// A BLP2 file declared a format version other than `1`.
    #[error("invalid BLP2 format version: expected 1, got {0}")]
    InvalidFormatVersion(u32),

    /// The color encoding byte is not one of the five known values
    /// (JPEG=0, Palette=1, DXT=2, ARGB8888=3, ARGB8888Dup=4).
    #[error("unsupported color encoding: {0}")]
    UnsupportedEncoding(u8),

    /// [`BlpFile::get_pixels`] was called on a file whose mipmap offset table
    /// contains no non-zero entries.
    #[error("no mipmaps present")]
    NoMipmaps,

    /// A mipmap offset/size pair points outside the bounds of the file buffer,
    /// or `offset + size` overflows `usize`.
    #[error("mipmap data out of bounds")]
    OutOfBounds,

    /// The mipmap data slice is present but shorter than the decoded image
    /// requires (e.g. palette indices are truncated, or alpha bytes are missing).
    #[error("mipmap data too short for declared image dimensions")]
    DataTooShort,

    /// The declared image dimensions (`width × height × 4`) would exceed the
    /// 256 MiB allocation ceiling, or the multiplication overflows `usize`.
    #[error("image dimensions too large")]
    ImageTooLarge,

    /// JPEG decoding failed. The inner string contains the decoder's error message.
    #[error("JPEG decode error: {0}")]
    JpegDecode(String),
}
