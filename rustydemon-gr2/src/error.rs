//! Error types for the Granny3D reader.

use thiserror::Error;

/// Every failure mode the reader can surface.
#[derive(Debug, Error)]
pub enum GrannyError {
    /// Input was shorter than the fixed 32-byte file header.
    #[error("file too short: {0} bytes (need at least {1})")]
    TooShort(usize, usize),

    /// The 16-byte magic didn't match any known Granny format variant.
    #[error("unknown magic: not a Granny3D file")]
    BadMagic,

    /// A field advertised an offset/length that walked off the end of the
    /// input buffer.  Usually indicates a truncated or malformed file.
    #[error("out-of-range read: want [{start}..{end}), have {have}")]
    OutOfRange {
        start: usize,
        end: usize,
        have: usize,
    },

    /// Sector advertised a compression type we don't know how to handle.
    /// `0 = None`, `1 = Oodle0`, `2 = Oodle1`, `3 = Bitknit1`, `4 = Bitknit2`.
    /// Rusty Demon implements None and Bitknit2.
    #[error("unsupported compression type {0} (supported: None=0, Bitknit2=4)")]
    UnsupportedCompression(u32),

    /// Bitknit stream violated an invariant mid-decode (e.g. truncated,
    /// distance larger than destination, or the initial word header was
    /// malformed).
    #[error("bitknit decode error: {0}")]
    BitknitDecode(&'static str),

    /// Element parser hit a type tag we don't know how to interpret.
    /// Granny has ~22 type IDs; we implement the ones D2R actually uses.
    #[error("unknown element type id {0}")]
    UnknownElementType(u32),

    /// A pointer fixup referenced a sector that doesn't exist.
    #[error("pointer references sector {sector} but only {count} sectors exist")]
    BadSectorRef { sector: u32, count: usize },

    /// A string element's UTF-8 didn't validate.
    #[error("invalid utf-8 in string element")]
    BadUtf8,
}

pub type Result<T> = std::result::Result<T, GrannyError>;
