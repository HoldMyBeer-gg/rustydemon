pub mod d4;
pub mod install;
pub mod mndx;
pub mod s1;
pub mod tvfs;
pub mod wow;

use std::collections::HashMap;

use crate::{
    error::CascError,
    types::{LocaleFlags, RootEntry},
};

/// Common interface for game-specific root manifest parsers.
///
/// Each game stores its file manifest differently.  A `RootHandler` abstracts
/// over those differences, mapping filename hashes (Jenkins96 or FileDataId)
/// to [`RootEntry`] records that contain the content key needed to look up the
/// file in the encoding table.
pub trait RootHandler: Send {
    /// Number of distinct filename hashes in the manifest.
    fn count(&self) -> usize;

    /// Retrieve all root entries for a given filename hash.
    ///
    /// Multiple entries for the same hash can exist when the same file is
    /// present in several locales or content variants.
    fn get_all_entries(&self, hash: u64) -> &[RootEntry];

    /// Retrieve entries for `hash` that match `locale`.
    ///
    /// Returns only entries whose [`RootEntry::locale`] has at least one bit
    /// in common with `locale`.
    fn get_entries(&self, hash: u64, locale: LocaleFlags) -> Vec<&RootEntry> {
        self.get_all_entries(hash)
            .iter()
            .filter(|e| e.locale.intersects(locale))
            .collect()
    }

    /// Iterate every (hash, RootEntry) pair in the manifest.
    ///
    /// This powers the regedit-style global search: every file known to the
    /// manifest is exposed regardless of which folder the UI is looking at.
    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_>;

    /// Translate a numeric FileDataId to the Jenkins96 hash stored here.
    ///
    /// Returns `None` for games / manifest versions that don't use FileDataIds.
    fn hash_for_file_data_id(&self, id: u32) -> Option<u64>;

    /// Translate a Jenkins96 hash back to a FileDataId, if known.
    fn file_data_id_for_hash(&self, hash: u64) -> Option<u32>;

    /// Snapshot of all FileDataId → hash mappings for off-thread listfile parsing.
    ///
    /// The default returns an empty map (games without FileDataIds).
    fn fdid_hash_map(&self) -> HashMap<u32, u64> {
        HashMap::new()
    }

    /// Return built-in file paths from the manifest (e.g. TVFS path table).
    ///
    /// Most root handlers return an empty vec; only TVFS-based handlers
    /// populate this, letting the UI build a tree without a listfile.
    fn builtin_paths(&self) -> Vec<(u64, String)> {
        Vec::new()
    }

    /// Whether this root handler provides a complete built-in path table,
    /// meaning no external listfile is required (or wanted).
    fn has_builtin_paths(&self) -> bool {
        false
    }

    /// Short human-readable name of the root format for diagnostics.
    fn type_name(&self) -> &'static str {
        "Unknown"
    }
}

/// Load the appropriate root handler for a BLTE-decoded root file.
///
/// Supports:
/// - MNDX format (SC1, SC2, Heroes of the Storm)
/// - WoW MFST format (WoW, D3, and legacy flat format)
///
/// Other games fall back to [`DummyRootHandler`].
pub fn load(data: Vec<u8>) -> Result<Box<dyn RootHandler>, CascError> {
    use mndx::MndxRootHandler;
    use s1::S1RootHandler;
    use wow::WowRootHandler;

    // SC1 Remastered: plain-text root (`path[:LOCALE]|hexmd5`). Check this
    // before the binary heuristics since text roots have no magic bytes.
    if S1RootHandler::looks_like_s1_root(&data) {
        if let Ok(handler) = S1RootHandler::parse(&data) {
            return Ok(Box::new(handler));
        }
    }

    if data.len() >= 4 {
        let maybe_magic = u32::from_le_bytes(data[..4].try_into().unwrap());

        // MNDX root (SC2, HOTS): 'MNDX' = 0x58444E4D
        if maybe_magic == 0x5844_4E4D {
            let handler = MndxRootHandler::parse(&data)?;
            return Ok(Box::new(handler));
        }

        // WoW MFST magic: 'MFST' (0x4D465354) or old-style flat format.
        if maybe_magic == 0x4D46_5354 || data.len().is_multiple_of(28) {
            let handler = WowRootHandler::parse(&data)?;
            return Ok(Box::new(handler));
        }
    }

    // Unknown format → empty dummy.
    Ok(Box::new(DummyRootHandler))
}

// ── Dummy root handler (unknown games) ────────────────────────────────────────

/// Placeholder root handler that returns nothing.
///
/// Used when the game format is not yet implemented or the manifest could not
/// be parsed.
pub struct DummyRootHandler;

impl RootHandler for DummyRootHandler {
    fn count(&self) -> usize {
        0
    }
    fn get_all_entries(&self, _hash: u64) -> &[RootEntry] {
        &[]
    }
    fn all_entries(&self) -> Box<dyn Iterator<Item = (u64, &RootEntry)> + '_> {
        Box::new(std::iter::empty())
    }
    fn hash_for_file_data_id(&self, _id: u32) -> Option<u64> {
        None
    }
    fn file_data_id_for_hash(&self, _hash: u64) -> Option<u32> {
        None
    }
    fn type_name(&self) -> &'static str {
        "Dummy"
    }
}
