//! Verify that `LocalIndexHandler::merge_archive_indices` actually loads
//! D2R's archive-style `.index` entries — independent of whether
//! `CascHandler::open_local` succeeds, which still needs CDN fetch for
//! the loose ENCODING blob.

use rustydemon_lib::{local_index::LocalIndexHandler, CascConfig, Md5Hash};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base =
        Path::new("/run/media/deck/sdcard/SteamLibrary/steamapps/common/Diablo II Resurrected");

    let config = CascConfig::load_local(base, "osi")?;
    let data_path = config.data_path();
    let ecache_path = config.ecache_path();
    let mut lix = LocalIndexHandler::load_multi(&[data_path.as_path(), ecache_path.as_path()])?;

    let before = lix.count();
    println!("before merge_archive_indices: {before} entries");

    let indices_path = config.archive_indices_path();
    let archives = config.archives();
    println!(
        "archives list: {} hashes, indices dir: {}",
        archives.len(),
        indices_path.display()
    );

    let added = lix.merge_archive_indices(&indices_path, archives, 0)?;
    println!("merge added:                {added}");
    println!("total after merge:          {}", lix.count());

    let enc_ekey = Md5Hash([
        0x9B, 0xBD, 0xA0, 0x45, 0x06, 0x40, 0x0A, 0x1F, 0xDE, 0x08, 0x06, 0x67, 0xBD, 0xA6, 0x36,
        0x5E,
    ]);
    match lix.get_entry(&enc_ekey) {
        Some(e) => println!(
            "ENCODING resolved: archive={} offset=0x{:x} size={} storage={}",
            e.index, e.offset, e.size, e.storage
        ),
        None => println!("ENCODING: not in local index (expected — it's in file-index)"),
    }

    Ok(())
}
