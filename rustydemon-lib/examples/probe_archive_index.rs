//! Validate the archive_index parser against real D2R .index files.
use rustydemon_lib::archive_index::parse_file;
use std::{fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        "/run/media/deck/sdcard/SteamLibrary/steamapps/common/\
         Diablo II Resurrected/data/indices",
    );
    let mut total_files = 0usize;
    let mut total_entries = 0usize;
    let mut total_expected = 0u64;
    let mut mismatches = 0usize;
    let mut parse_errors = 0usize;
    let mut first_footer_printed = false;

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("index") {
            continue;
        }
        total_files += 1;
        match parse_file(&path) {
            Ok((footer, entries)) => {
                if !first_footer_printed {
                    eprintln!("first footer: {:#?}", footer);
                    first_footer_printed = true;
                }
                total_entries += entries.len();
                total_expected += footer.element_count as u64;
                // Allow entries.len() >= element_count because the footer
                // element_count can lag slightly in rewritten indices.
                // Flag only underruns (we decoded fewer than expected).
                if (entries.len() as u32) < footer.element_count {
                    mismatches += 1;
                    if mismatches <= 10 {
                        eprintln!(
                            "UNDERRUN: {} expected={} decoded={} ob={} sb={} page={}",
                            path.file_name().unwrap().to_string_lossy(),
                            footer.element_count,
                            entries.len(),
                            footer.offset_bytes,
                            footer.size_bytes,
                            footer.page_length,
                        );
                    }
                }
            }
            Err(e) => {
                parse_errors += 1;
                if parse_errors <= 3 {
                    eprintln!(
                        "ERROR: {}: {e}",
                        path.file_name().unwrap().to_string_lossy()
                    );
                }
            }
        }
    }

    println!("\n── summary ──");
    println!("files:          {total_files}");
    println!("parse errors:   {parse_errors}");
    println!("total entries:  {total_entries}");
    println!("footer sum:     {total_expected}");
    println!("underruns:      {mismatches}");

    // Also find our ENCODING ekey across all .index files to confirm it
    // appears exactly once and at a real entry boundary.
    let needle = [
        0x9Bu8, 0xBD, 0xA0, 0x45, 0x06, 0x40, 0x0A, 0x1F, 0xDE, 0x08, 0x06, 0x67, 0xBD, 0xA6, 0x36,
        0x5E,
    ];
    let mut found_in = 0;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("index") {
            continue;
        }
        if let Ok((_, entries)) = parse_file(&path) {
            for e in &entries {
                if e.ekey.0 == needle {
                    found_in += 1;
                    println!(
                        "ENCODING hit: {} offset=0x{:x} size={}",
                        path.file_name().unwrap().to_string_lossy(),
                        e.archive_offset,
                        e.encoded_size
                    );
                }
            }
        }
    }
    println!("ENCODING ekey matched in {found_in} parsed entries");

    Ok(())
}
