//! Dump sector metadata without attempting to decompress anything.
//! Used for structural sanity-checking of Granny files while the
//! Bitknit decoder is still being debugged.

use rustydemon_gr2::{
    file_info::parse_file_info, header::parse_header, section::parse_section_info,
};

fn main() {
    let path = std::env::args().nth(1).expect("usage: info <file>");
    let bytes = std::fs::read(&path).unwrap();
    println!("=== {} ({} bytes) ===", path, bytes.len());

    let (header, mut cursor) = parse_header(&bytes).unwrap();
    println!(
        "header: {:?} bits_64={} size={} format={}",
        header.endian, header.bits_64, header.size, header.format
    );

    let (fi, next) = parse_file_info(&bytes, cursor, header.endian).unwrap();
    cursor = next;
    println!(
        "file_info: version={} total_size={} crc32=0x{:08X} info_size={} sectors={} type_ref={:?} root_ref={:?} tag=0x{:X}",
        fi.format_version,
        fi.total_size,
        fi.crc32,
        fi.file_info_size,
        fi.sector_count,
        fi.type_ref,
        fi.root_ref,
        fi.tag
    );

    for i in 0..fi.sector_count {
        let (info, next) = parse_section_info(&bytes, cursor, header.endian).unwrap();
        cursor = next;
        println!(
            "  sector {i}: comp={} off=0x{:X} c_len={} d_len={} align={} stop0=0x{:X} stop1=0x{:X} fixup(off=0x{:X},size={}) marshal(off=0x{:X},size={})",
            info.compression_type,
            info.data_offset,
            info.compressed_length,
            info.decompressed_length,
            info.alignment,
            info.oodle_stop_0,
            info.oodle_stop_1,
            info.fixup_offset,
            info.fixup_size,
            info.marshall_offset,
            info.marshall_size
        );
    }
}
