//! (sector, position) reference pair.
//!
//! Granny stores absolute addresses as a (sector index, byte offset
//! within that sector's decompressed data) pair.  This lets the file
//! format keep everything on-disk as small offsets rather than
//! pointer-width fields, and lets the runtime rebase each sector's
//! data independently during load.

/// Two-u32 reference used by FileInfo (type_ref, root_ref) and by the
/// fixup pointer table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reference {
    pub sector: u32,
    pub position: u32,
}
