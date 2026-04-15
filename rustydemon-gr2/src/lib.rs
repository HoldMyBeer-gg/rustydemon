//! Pure-Rust reader for Granny3D (`.gr2` / D2R `.model`) files.
//!
//! Rusty Demon uses this to preview Diablo II: Resurrected model
//! assets.  The crate covers:
//!
//! - The 32-byte file header across all six known magic variants
//!   (LE/BE × 32/64-bit × file-format 6/7).
//! - The file-info struct and sector-descriptor table.
//! - Bitknit2 decompression — pure-Rust port of the
//!   [powzix/ooz](https://github.com/powzix/ooz) C++ decoder.
//!   Raw (compression type 0) sectors pass through unmodified.
//! - Pointer-fixup tables (kept as side tables, not in-place relocated).
//! - The type-tree / element walker for Granny types 1..=22 — the
//!   ones D2R actually uses.  Types we don't fully decode surface as
//!   [`ElementValue::Opaque`] so structural walks keep making
//!   progress past unsupported leaf types.
//!
//! For the high-level entry point, see [`GrannyFile::from_bytes`].
//!
//! Attribution: the layout fields were originally reverse-engineered
//! by the authors of [opengr2](https://crates.io/crates/opengr2) and
//! [lslib](https://github.com/Norbyte/lslib); Bitknit2 was decoded by
//! Fabian Giesen in [powzix/ooz](https://github.com/powzix/ooz).  This
//! crate is a clean-room port written against those reference sources.

pub mod bitknit;
pub mod element;
pub mod error;
pub mod file_info;
pub mod granny_file;
pub mod header;
pub mod mesh;
pub mod reference;
pub mod section;

pub use element::{Element, ElementValue, Transform};
pub use error::{GrannyError, Result};
pub use granny_file::{GrannyFile, GrannySummary};
pub use header::{has_granny_magic, Endian, Header};
pub use mesh::Mesh;
