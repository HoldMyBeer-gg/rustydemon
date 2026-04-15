//! Throwaway probe: load a Granny file from disk and print a
//! structural summary.  Used for end-to-end validation of the
//! Bitknit2 port against real D2R assets.

use rustydemon_gr2::GrannyFile;

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <file>");
    let bytes = std::fs::read(&path).expect("read");
    println!("=== {} ({} bytes) ===", path, bytes.len());
    match GrannyFile::from_bytes(&bytes) {
        Ok(gf) => {
            println!(
                "header: endian={:?} bits_64={} size={} format={}",
                gf.header.endian, gf.header.bits_64, gf.header.size, gf.header.format
            );
            println!(
                "file_info: version={} total_size={} crc32=0x{:08X} info_size={} sectors={} tag=0x{:08X}",
                gf.file_info.format_version,
                gf.file_info.total_size,
                gf.file_info.crc32,
                gf.file_info.file_info_size,
                gf.file_info.sector_count,
                gf.file_info.tag,
            );
            for (i, s) in gf.sections.iter().enumerate() {
                println!(
                    "  sector {i}: comp={} off={} c_len={} d_len={} fixups={}",
                    s.info.compression_type,
                    s.info.data_offset,
                    s.info.compressed_length,
                    s.info.decompressed_length,
                    s.info.fixup_size
                );
            }
            let summary = gf.summary();
            println!("summary: {:?}", summary);

            let meshes = gf.meshes();
            println!("\nextracted meshes ({}):", meshes.len());
            for m in &meshes {
                println!(
                    "  '{}'  verts={} tris={} bbox=[{:.2},{:.2},{:.2}]..[{:.2},{:.2},{:.2}] mat={:?}",
                    m.name,
                    m.positions.len(),
                    m.indices.len() / 3,
                    m.bbox_min[0], m.bbox_min[1], m.bbox_min[2],
                    m.bbox_max[0], m.bbox_max[1], m.bbox_max[2],
                    m.material_index
                );
                if !m.positions.is_empty() {
                    println!("    first pos = {:?}", m.positions[0]);
                    println!("    first uv  = {:?}", m.uvs[0]);
                    println!("    first idx = {:?}", &m.indices[..6.min(m.indices.len())]);
                }
            }

            println!("\nroot elements ({}):", gf.root_elements.len());
            for e in gf.root_elements.iter() {
                print_element(e, 1);
            }
        }
        Err(e) => {
            println!("ERROR: {e}");
        }
    }
}

fn print_element(e: &rustydemon_gr2::Element, depth: usize) {
    let pad = "  ".repeat(depth);
    println!("{pad}- {} :: {}", e.name, value_kind(&e.value));
    use rustydemon_gr2::ElementValue::*;
    if depth > 6 {
        return;
    }
    match &e.value {
        Reference(children) => {
            for c in children {
                print_element(c, depth + 1);
            }
        }
        ReferenceArray(groups) | ArrayOfReferences(groups) => {
            for (i, g) in groups.iter().enumerate().take(4) {
                println!("{pad}  [{i}]");
                for c in g {
                    print_element(c, depth + 2);
                }
            }
            if groups.len() > 4 {
                println!("{pad}  … {} more", groups.len() - 4);
            }
        }
        _ => {}
    }
}

fn value_kind(v: &rustydemon_gr2::ElementValue) -> String {
    use rustydemon_gr2::ElementValue::*;
    match v {
        Reference(c) => format!("Reference({} children)", c.len()),
        ReferenceArray(c) => format!("ReferenceArray({} entries)", c.len()),
        ArrayOfReferences(c) => format!("ArrayOfReferences({} entries)", c.len()),
        String(s) => format!("String({s:?})"),
        Transform(_) => "Transform".into(),
        F32(v) => format!("F32({v})"),
        I32(v) => format!("I32({v})"),
        Opaque(id) => format!("Opaque(type={id})"),
        Array(v) => format!("Array({} entries)", v.len()),
    }
}
