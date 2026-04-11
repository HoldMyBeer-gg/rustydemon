pub mod wow;

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
pub trait RootHandler {
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
}

/// Load the appropriate root handler for a BLTE-decoded root file.
///
/// Currently supports the WoW MFST format.  Other games fall back to
/// [`DummyRootHandler`].
pub fn load(data: Vec<u8>) -> Result<Box<dyn RootHandler>, CascError> {
    use wow::WowRootHandler;

    // WoW MFST magic: 'MFST' (0x4D465354) or old-style (first 4 bytes = count).
    if data.len() >= 4 {
        let maybe_magic = u32::from_le_bytes(data[..4].try_into().unwrap());
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
}
