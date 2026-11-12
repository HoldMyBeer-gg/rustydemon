# rustydemon-gr2

A pure-Rust reader for Granny3D files (`.gr2`, and the `.model` files in
Diablo II: Resurrected). No C dependencies and no Granny runtime.

Part of [Rusty Demon](https://github.com/HoldMyBeer-gg/rustydemon), which uses
it to preview D2R models.

## Install

```toml
[dependencies]
rustydemon-gr2 = "0.5"
```

## Quick start

```rust,no_run
use rustydemon_gr2::GrannyFile;

let bytes = std::fs::read("character.model")?;
let gr2 = GrannyFile::from_bytes(&bytes)?;

let summary = gr2.summary();
println!("{} mesh(es), {} skeleton(s)", summary.meshes, summary.skeletons);

// Geometry comes back as flat buffers, ready to upload to a GPU.
for mesh in gr2.meshes() {
    println!(
        "{}: {} vertices, {} triangles",
        mesh.name,
        mesh.positions.len(),
        mesh.indices.len() / 3,
    );
}

for texture in gr2.texture_filenames() {
    println!("references {texture}");
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## What it covers

- The 32-byte file header in all six known magic variants (little and big
  endian, 32 and 64-bit, file format 6 and 7).
- The file-info struct and the sector descriptor table.
- Bitknit2 decompression, ported to Rust from
  [powzix/ooz](https://github.com/powzix/ooz). Uncompressed sectors pass
  through untouched.
- Pointer fixup tables, kept as side tables instead of relocating in place.
- The type tree and element walker for Granny types 1 through 22, which are
  the ones D2R uses. Types that are not fully decoded come back as
  `ElementValue::Opaque`, so a structural walk keeps going past them.
- Mesh extraction: positions, normals, UVs, indices, bounding box, and
  material index.

## Credits

The file layout was reverse-engineered by the authors of
[opengr2](https://crates.io/crates/opengr2) and
[lslib](https://github.com/Norbyte/lslib). Bitknit2 was decoded by Fabian
Giesen in [powzix/ooz](https://github.com/powzix/ooz). This crate is a
clean-room port written against those references.

## License

AGPL-3.0 with the Commons Clause. See
[LICENSE](https://github.com/HoldMyBeer-gg/rustydemon/blob/main/LICENSE) and
[COMMERCIAL.md](https://github.com/HoldMyBeer-gg/rustydemon/blob/main/COMMERCIAL.md).
