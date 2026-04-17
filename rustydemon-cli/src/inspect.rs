//! `inspect` subcommand — print format-specific metadata for a local file.
//!
//! Reads a file from disk and runs the appropriate parser based on magic
//! bytes / extension.  Prints a text summary to stdout — the same kind
//! of info the GUI preview panel shows, minus GPU rendering.
//!
//! Supported formats:
//! - `.model` / Granny3D (`rustydemon-gr2`)
//! - `.m2` / WoW M2 (`wow-alchemy-m2`)
//! - `.wmo` / WoW WMO (`wow-wmo`)
//! - `.blp` / BLP texture (`rustydemon-blp2`)
//! - `.texture` / D2R `<DE(` container

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;

#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Path to a local file, OR a virtual path inside a CASC archive
    /// (when --archive is set).
    pub file: String,

    /// CASC game installation root.  When set, `file` is treated as a
    /// virtual path inside the archive (e.g.
    /// `Creature/LadySylvanasWindrunner/LadySylvanasWindrunner.m2`).
    #[arg(long, short = 'a')]
    pub archive: Option<PathBuf>,

    /// Community listfile (required for WoW path resolution when using
    /// --archive).
    #[arg(long, short = 'l')]
    pub listfile: Option<PathBuf>,

    /// Extract by FileDataID instead of path (use with --archive).
    #[arg(long)]
    pub fdid: Option<u32>,

    /// Product UID (auto-detected if omitted).
    #[arg(long)]
    pub product: Option<String>,

    /// Export mesh geometry as Wavefront OBJ to this path.
    #[arg(long)]
    pub obj: Option<PathBuf>,

    /// Render the mesh to a PNG file (headless wgpu, no window).
    #[arg(long)]
    pub png: Option<PathBuf>,
}

pub fn run(args: &InspectArgs) -> Result<()> {
    let (data, filename) = if let Some(archive_path) = &args.archive {
        load_from_archive(
            archive_path,
            &args.file,
            args.fdid,
            args.listfile.as_deref(),
            args.product.as_deref(),
        )?
    } else {
        let path = Path::new(&args.file);
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (data, filename)
    };

    if data.is_empty() {
        println!("(empty file)");
        return Ok(());
    }

    // Dispatch on magic bytes, then fall back to extension.
    if rustydemon_gr2::has_granny_magic(&data) {
        inspect_granny(&data, args.obj.as_deref(), args.png.as_deref())?;
    } else if data.len() >= 4 && (&data[..4] == b"MD20" || &data[..4] == b"MD21") {
        inspect_m2(&data, &filename, args.obj.as_deref(), args.png.as_deref())?;
    } else if data.len() >= 4 && &data[..4] == b"REVM" {
        inspect_wmo(&data, &filename, args.obj.as_deref())?;
    } else if data.len() >= 4 && &data[..4] == b"BLP2" {
        inspect_blp(&data)?;
    } else if data.len() >= 4 && &data[..4] == b"<DE(" {
        inspect_texture_de(&data)?;
    } else {
        println!(
            "Unknown format (magic: {:02X?}, {} bytes)",
            &data[..data.len().min(8)],
            data.len()
        );
    }

    Ok(())
}

fn inspect_granny(data: &[u8], obj_path: Option<&Path>, png_path: Option<&Path>) -> Result<()> {
    use rustydemon_gr2::{ElementValue, GrannyFile};

    let gf =
        GrannyFile::from_bytes(data).map_err(|e| anyhow::anyhow!("Granny3D parse failed: {e}"))?;
    let summary = gf.summary();

    println!("D2R .model  (Granny3D)\n");
    println!(
        "Format            {} ({}-bit {:?})",
        gf.header.format,
        if gf.header.bits_64 { 64 } else { 32 },
        gf.header.endian
    );
    println!("File size         {} bytes", gf.file_info.total_size);
    println!("CRC-32            0x{:08X}", gf.file_info.crc32);
    println!("Sections          {}\n", summary.section_count);

    println!("Contents");
    println!("  Models          {}", summary.models);
    println!("  Meshes          {}", summary.meshes);
    println!("  Skeletons       {}", summary.skeletons);
    println!("  Animations      {}", summary.animations);
    println!("  Textures        {}\n", summary.textures);

    // Source file.
    for e in &gf.root_elements {
        if e.name == "FromFileName" {
            if let ElementValue::String(s) = &e.value {
                println!("Source file       {s}");
            }
        }
    }

    // Art tool info.
    if let Some(art_tool) = gf.find("ArtToolInfo") {
        if let ElementValue::Reference(children) = &art_tool.value {
            for e in children {
                match (e.name.as_str(), &e.value) {
                    ("FromArtToolName", ElementValue::String(s)) => {
                        println!("Authored in       {s}");
                    }
                    ("UnitsPerMeter", ElementValue::F32(v)) => {
                        println!("Units             {v:.4} per metre");
                    }
                    _ => {}
                }
            }
        }
    }

    // Texture filenames.
    let tex_names = gf.texture_filenames();
    if !tex_names.is_empty() {
        println!("\nTextures ({}):", tex_names.len());
        for name in &tex_names {
            println!("  {name}");
        }
    }

    // Mesh geometry.
    let meshes = gf.meshes();
    if !meshes.is_empty() {
        println!("\nMeshes ({}):", meshes.len());
        for m in &meshes {
            println!(
                "  '{}'  {} verts / {} tris  material={}  bbox=({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
                m.name,
                m.positions.len(),
                m.indices.len() / 3,
                m.material_index
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "none".into()),
                m.bbox_min[0], m.bbox_min[1], m.bbox_min[2],
                m.bbox_max[0], m.bbox_max[1], m.bbox_max[2],
            );
        }
    }

    // Top-level tree.
    println!("\nTop-level tree:");
    for e in &gf.root_elements {
        println!("  - {} :: {}", e.name, element_kind(&e.value));
    }

    // OBJ export.
    if let Some(path) = obj_path {
        if meshes.is_empty() {
            println!("\nNo geometry to export.");
        } else {
            write_obj_multi(path, &meshes)?;
        }
    }

    // PNG render.
    if let Some(path) = png_path {
        if meshes.is_empty() {
            println!("\nNo geometry to render.");
        } else {
            // Flatten all meshes into one RenderMesh.
            let mut positions: Vec<[f32; 3]> = Vec::new();
            let mut all_uvs: Vec<[f32; 2]> = Vec::new();
            let mut indices: Vec<u32> = Vec::new();
            let mut bbox_min = [f32::INFINITY; 3];
            let mut bbox_max = [f32::NEG_INFINITY; 3];
            for m in &meshes {
                let vbase = positions.len() as u32;
                positions.extend_from_slice(&m.positions);
                all_uvs.extend_from_slice(&m.uvs);
                indices.extend(m.indices.iter().map(|i| vbase + i));
                for axis in 0..3 {
                    bbox_min[axis] = bbox_min[axis].min(m.bbox_min[axis]);
                    bbox_max[axis] = bbox_max[axis].max(m.bbox_max[axis]);
                }
            }
            let render_mesh = crate::render::RenderMesh {
                positions,
                uvs: all_uvs,
                indices,
                bbox_min,
                bbox_max,
            };
            crate::render::render_to_png(&render_mesh, path, 800, 600)?;
        }
    }

    Ok(())
}

fn inspect_m2(
    data: &[u8],
    filename: &str,
    obj_path: Option<&Path>,
    _png_path: Option<&Path>,
) -> Result<()> {
    use wow_alchemy_data::types::WowStructR;
    use wow_alchemy_m2::M2Model;

    let mut reader = Cursor::new(data);
    let m2 = M2Model::wow_read(&mut reader).map_err(|e| anyhow::anyhow!("M2 parse failed: {e}"))?;

    let md20 = &m2.md20;
    let is_chunked = &m2.magic == b"MD21";

    // SFID chunk (skin file FDIDs).
    let skin_fdids: Vec<u32> = m2
        .chunks
        .iter()
        .find_map(|c| match c {
            wow_alchemy_m2::model::M2Chunk::SFID(skins) => Some(skins.file_ids.clone()),
            _ => None,
        })
        .unwrap_or_default();

    // TXID chunk (texture FDIDs) — stored as a flat Vec<u32>.
    let texture_fdids: Vec<u32> = m2
        .chunks
        .iter()
        .find_map(|c| match c {
            wow_alchemy_m2::model::M2Chunk::TXID(ids) => Some(ids.clone()),
            _ => None,
        })
        .unwrap_or_default();

    println!("M2 model  •  {filename}\n");
    println!(
        "Format        {}",
        if is_chunked {
            "MD21 (Legion+)"
        } else {
            "MD20 (legacy)"
        }
    );
    println!("Name          {}", md20.name);
    println!("Vertices      {}", md20.vertices.len());
    println!("Textures      {}", md20.textures.len());
    println!("Materials     {}", md20.materials.len());
    println!("Bones         {}", md20.bones.len());
    println!("Animations    {}", md20.animations.len());
    println!("Skin files    {}", skin_fdids.len());

    if !skin_fdids.is_empty() {
        println!("\nSkin FDIDs:");
        for (i, fdid) in skin_fdids.iter().enumerate() {
            println!("  [{i}] {fdid}");
        }
    }

    if !texture_fdids.is_empty() {
        println!("\nTexture FDIDs:");
        for (i, fdid) in texture_fdids.iter().enumerate() {
            println!("  [{i}] {fdid}");
        }
    }

    // Bounding box from header.
    let bb = &md20.header.bounding_box;
    println!(
        "\nBounding box  ({:.2}, {:.2}, {:.2}) .. ({:.2}, {:.2}, {:.2})",
        bb.min.x, bb.min.y, bb.min.z, bb.max.x, bb.max.y, bb.max.z,
    );

    // Chunks summary.
    if !m2.chunks.is_empty() {
        println!("\nChunks ({}):", m2.chunks.len());
        for c in &m2.chunks {
            println!("  {}", chunk_label(c));
        }
    }

    // OBJ export — uses MD20 vertices + SKIN indirection (no archive
    // needed since the M2 carries all vertices inline and the skin
    // file would only be needed for LOD selection, which we skip).
    if let Some(path) = obj_path {
        let positions: Vec<[f32; 3]> = md20
            .vertices
            .iter()
            .map(|v| [v.position.x, v.position.y, v.position.z])
            .collect();
        let normals: Vec<[f32; 3]> = md20
            .vertices
            .iter()
            .map(|v| [v.normal.x, v.normal.y, v.normal.z])
            .collect();
        let uvs: Vec<[f32; 2]> = md20
            .vertices
            .iter()
            .map(|v| [v.tex_coords.x, v.tex_coords.y])
            .collect();

        // Without a skin file we can't build triangles (the skin carries
        // the index buffer). Report that clearly.
        println!(
            "\nOBJ export: writing {} vertices (no triangles — skin file not available from disk)",
            positions.len()
        );
        write_obj_verts_only(path, &positions, &normals, &uvs)?;
    }

    Ok(())
}

fn inspect_wmo(data: &[u8], filename: &str, _obj_path: Option<&Path>) -> Result<()> {
    use wow_wmo::{parse_wmo, ParsedWmo};

    let mut reader = Cursor::new(data);
    match parse_wmo(&mut reader) {
        Ok(ParsedWmo::Root(root)) => {
            let mn = root.bounding_box_min;
            let mx = root.bounding_box_max;
            println!("WMO root  •  {filename}\n");
            println!("Version       {}", root.version);
            println!("Groups        {}", root.n_groups);
            println!("Materials     {}", root.n_materials);
            println!("Textures      {}", root.textures.len());
            println!("Portals       {}", root.n_portals);
            println!("Lights        {}", root.n_lights);
            println!("Doodad sets   {}", root.n_doodad_sets);
            println!(
                "\nBounding box  ({:.1}, {:.1}, {:.1}) .. ({:.1}, {:.1}, {:.1})",
                mn[0], mn[1], mn[2], mx[0], mx[1], mx[2],
            );
            if !root.textures.is_empty() {
                println!("\nTextures:");
                for (i, t) in root.textures.iter().enumerate() {
                    println!("  [{i}] {t}");
                }
            }
            if !root.group_file_ids.is_empty() {
                println!("\nGroup FDIDs:");
                for (i, fdid) in root.group_file_ids.iter().enumerate() {
                    println!("  [{i}] {fdid}");
                }
            }
        }
        Ok(ParsedWmo::Group(group)) => {
            let total_batches = group.trans_batch_count as u32
                + group.int_batch_count as u32
                + group.ext_batch_count as u32;
            println!("WMO group  •  {filename}\n");
            println!("Version       {}", group.version);
            println!("Vertices      {}", group.n_vertices);
            println!("Triangles     {}", group.n_triangles);
            println!(
                "Batches       {} (trans {} / int {} / ext {})",
                total_batches,
                group.trans_batch_count,
                group.int_batch_count,
                group.ext_batch_count,
            );
        }
        Err(e) => {
            println!("WMO magic detected but parse failed: {e}");
        }
    }

    Ok(())
}

fn inspect_blp(data: &[u8]) -> Result<()> {
    use rustydemon_blp2::BlpFile;

    let blp =
        BlpFile::from_bytes(data.to_vec()).map_err(|e| anyhow::anyhow!("BLP parse failed: {e}"))?;

    println!("BLP texture\n");
    println!("Dimensions    {}x{}", blp.width, blp.height);
    println!("Encoding      {:?}", blp.color_encoding);
    println!("Alpha size    {}", blp.alpha_size);
    println!("Mipmap count  {}", blp.mipmap_count());
    println!("File size     {} bytes", data.len());

    // Try decoding mip 0 to verify it works.
    match blp.get_pixels(0) {
        Ok((_, w, h)) => println!("\nMip 0 decode  OK ({w}x{h})"),
        Err(e) => println!("\nMip 0 decode  FAILED: {e}"),
    }

    Ok(())
}

fn inspect_texture_de(data: &[u8]) -> Result<()> {
    // Inline the small <DE( header parse — avoids pulling in the GUI crate.
    if data.len() < 0x2C {
        println!("<DE( header too short ({} bytes)", data.len());
        return Ok(());
    }

    let format_code = data[4];
    let width = u32::from_le_bytes(data[0x08..0x0C].try_into().unwrap());
    let height = u32::from_le_bytes(data[0x0C..0x10].try_into().unwrap());
    let mip_count = u32::from_le_bytes(data[0x1C..0x20].try_into().unwrap());

    println!("D2R .texture  (<DE( container)\n");
    println!("Dimensions    {width}x{height}");
    println!("Format code   0x{format_code:02X}");
    println!("Mip count     {mip_count}");
    println!("File size     {} bytes", data.len());

    // Block size class.
    if width > 0 && height > 0 && mip_count > 0 {
        let table_start = 0x24usize;
        if data.len() >= table_start + 8 {
            let mip0_size =
                u32::from_le_bytes(data[table_start..table_start + 4].try_into().unwrap()) as usize;
            let blocks = ((width.max(4) / 4) as usize) * ((height.max(4) / 4) as usize);
            let bpb = mip0_size.checked_div(blocks.max(1)).unwrap_or(0);
            let bc_guess = match bpb {
                8 => "BC1 or BC4 (8 bytes/block)",
                16 => "BC3, BC5, or BC7 (16 bytes/block)",
                _ => "unknown block size",
            };
            println!("Mip 0 size    {} bytes", mip0_size);
            println!("Block class   {bc_guess}");

            // Try decoding mip 0.
            let offset_field_pos = table_start + 4;
            if data.len() >= offset_field_pos + 4 {
                let self_rel = u32::from_le_bytes(
                    data[offset_field_pos..offset_field_pos + 4]
                        .try_into()
                        .unwrap(),
                ) as usize;
                let mip0_offset = offset_field_pos + self_rel;
                if mip0_offset + mip0_size <= data.len() {
                    let mip = &data[mip0_offset..mip0_offset + mip0_size];
                    let w = width as usize;
                    let h = height as usize;
                    let mut pixels = vec![0u32; w * h];

                    // Try the likely BC format based on block size.
                    let (ok, name) = if bpb == 8 {
                        if texture2ddecoder::decode_bc1(mip, w, h, &mut pixels).is_ok() {
                            (true, "BC1")
                        } else if texture2ddecoder::decode_bc4(mip, w, h, &mut pixels).is_ok() {
                            (true, "BC4")
                        } else {
                            (false, "?")
                        }
                    } else if bpb == 16 {
                        if texture2ddecoder::decode_bc3(mip, w, h, &mut pixels).is_ok() {
                            (true, "BC3")
                        } else if texture2ddecoder::decode_bc7(mip, w, h, &mut pixels).is_ok() {
                            (true, "BC7")
                        } else if texture2ddecoder::decode_bc5(mip, w, h, &mut pixels).is_ok() {
                            (true, "BC5")
                        } else {
                            (false, "?")
                        }
                    } else {
                        (false, "?")
                    };

                    if ok {
                        println!("\nMip 0 decode  OK ({name})");
                    } else {
                        println!("\nMip 0 decode  FAILED (no BC codec matched)");
                    }
                }
            }
        }
    }

    Ok(())
}

fn element_kind(v: &rustydemon_gr2::ElementValue) -> String {
    use rustydemon_gr2::ElementValue;
    match v {
        ElementValue::Reference(c) => format!("Reference ({} children)", c.len()),
        ElementValue::ReferenceArray(g) => format!("ReferenceArray ({} entries)", g.len()),
        ElementValue::ArrayOfReferences(g) => {
            format!("ArrayOfReferences ({} entries)", g.len())
        }
        ElementValue::String(s) => format!("String {s:?}"),
        ElementValue::Transform(_) => "Transform".into(),
        ElementValue::F32(v) => format!("f32 {v}"),
        ElementValue::I32(v) => format!("i32 {v}"),
        ElementValue::Opaque(id) => format!("Opaque(type={id})"),
        ElementValue::Array(v) => format!("Array ({} entries)", v.len()),
    }
}

// ── OBJ export helpers ──────────────────────────────────────────────────────

/// Mesh data suitable for OBJ export.
struct ObjMesh<'a> {
    name: &'a str,
    positions: &'a [[f32; 3]],
    normals: &'a [[f32; 3]],
    uvs: &'a [[f32; 2]],
    indices: &'a [u32],
}

/// Write multiple Granny meshes to a single OBJ file with named groups.
fn write_obj_multi(path: &Path, meshes: &[rustydemon_gr2::Mesh]) -> Result<()> {
    let obj_meshes: Vec<ObjMesh<'_>> = meshes
        .iter()
        .map(|m| ObjMesh {
            name: &m.name,
            positions: &m.positions,
            normals: &m.normals,
            uvs: &m.uvs,
            indices: &m.indices,
        })
        .collect();
    write_obj_file(path, &obj_meshes)
}

/// Write an OBJ with vertices only (no faces). Used for M2 when the
/// skin file isn't available on disk.
fn write_obj_verts_only(
    path: &Path,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
) -> Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?,
    );
    writeln!(f, "# rustydemon-cli inspect --obj")?;
    writeln!(f, "# vertices only (no skin file for triangles)\n")?;
    for p in positions {
        writeln!(f, "v {:.6} {:.6} {:.6}", p[0], p[1], p[2])?;
    }
    for n in normals {
        writeln!(f, "vn {:.6} {:.6} {:.6}", n[0], n[1], n[2])?;
    }
    for uv in uvs {
        writeln!(f, "vt {:.6} {:.6}", uv[0], uv[1])?;
    }
    println!("Wrote {} to {}", positions.len(), path.display());
    Ok(())
}

/// Write one or more meshes to a Wavefront OBJ file.
fn write_obj_file(path: &Path, meshes: &[ObjMesh<'_>]) -> Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?,
    );
    writeln!(f, "# rustydemon-cli inspect --obj")?;

    // OBJ uses 1-based global vertex indices across all groups, so we
    // track the running offset as we emit each mesh.
    let mut v_offset: u32 = 0;
    let mut total_verts: u32 = 0;
    let mut total_tris: u32 = 0;

    for mesh in meshes {
        writeln!(f, "\ng {}", mesh.name)?;

        for p in mesh.positions {
            writeln!(f, "v {:.6} {:.6} {:.6}", p[0], p[1], p[2])?;
        }
        for n in mesh.normals {
            writeln!(f, "vn {:.6} {:.6} {:.6}", n[0], n[1], n[2])?;
        }
        for uv in mesh.uvs {
            writeln!(f, "vt {:.6} {:.6}", uv[0], uv[1])?;
        }

        let has_normals = !mesh.normals.is_empty();
        let has_uvs = !mesh.uvs.is_empty();
        for tri in mesh.indices.chunks_exact(3) {
            let (a, b, c) = (
                tri[0] + v_offset + 1,
                tri[1] + v_offset + 1,
                tri[2] + v_offset + 1,
            );
            match (has_uvs, has_normals) {
                (true, true) => writeln!(f, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}")?,
                (true, false) => writeln!(f, "f {a}/{a} {b}/{b} {c}/{c}")?,
                (false, true) => writeln!(f, "f {a}//{a} {b}//{b} {c}//{c}")?,
                (false, false) => writeln!(f, "f {a} {b} {c}")?,
            }
        }

        total_verts += mesh.positions.len() as u32;
        total_tris += mesh.indices.len() as u32 / 3;
        v_offset += mesh.positions.len() as u32;
    }

    println!(
        "Wrote {total_verts} verts / {total_tris} tris to {}",
        path.display()
    );
    Ok(())
}

/// Load a file from a CASC archive by virtual path.
fn load_from_archive(
    archive_path: &Path,
    virtual_path: &str,
    fdid: Option<u32>,
    listfile: Option<&Path>,
    product: Option<&str>,
) -> Result<(Vec<u8>, String)> {
    use rustydemon_lib::{CascConfig, CascHandler};

    let product = product.map(String::from).unwrap_or_else(|| {
        CascConfig::detect_products(archive_path)
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                if archive_path.join("Data").join(".build.config").is_file() {
                    "fenris".into()
                } else {
                    "wow".into()
                }
            })
    });

    eprintln!("Opening {} (product: {product})", archive_path.display());
    let mut casc = CascHandler::open_local(archive_path, &product)
        .with_context(|| format!("opening {}", archive_path.display()))?;
    casc.load_builtin_paths();

    if let Some(lf) = listfile {
        let content = std::fs::read_to_string(lf)
            .with_context(|| format!("reading listfile {}", lf.display()))?;
        let fdid_map = casc.fdid_hash_snapshot();
        let (filenames, tree) = rustydemon_lib::prepare_listfile(&content, &fdid_map);
        eprintln!("  listfile: {} entries", filenames.len());
        casc.apply_listfile(filenames, tree);
    }

    let (data, label) = if let Some(id) = fdid {
        let data = casc
            .open_file_by_fdid(id)
            .with_context(|| format!("opening FDID {id} from archive"))?;
        (data, format!("FDID {id}"))
    } else {
        let data = casc
            .open_file_by_name(virtual_path)
            .with_context(|| format!("opening {virtual_path} from archive"))?;
        (data, virtual_path.to_string())
    };

    let filename = Path::new(virtual_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| virtual_path.to_string());

    eprintln!("  loaded {} ({} bytes)", label, data.len());
    Ok((data, filename))
}

fn chunk_label(c: &wow_alchemy_m2::model::M2Chunk) -> String {
    use wow_alchemy_m2::model::M2Chunk;
    match c {
        M2Chunk::SFID(s) => format!("SFID — {} skin file IDs", s.file_ids.len()),
        M2Chunk::TXID(t) => format!("TXID — {} texture file IDs", t.len()),
        M2Chunk::AFID(a) => format!("AFID — {} animation file IDs", a.len()),
        M2Chunk::TXAC(t) => format!("TXAC — {} entries", t.len()),
        M2Chunk::Unknown(bytes) => format!("Unknown — {} bytes", bytes.len()),
        _ => format!("{c:?}"),
    }
}
