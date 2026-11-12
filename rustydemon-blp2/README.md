# rustydemon-blp2

A Rust library for reading BLP texture files (BLP0, BLP1, BLP2) and decoding
them to RGBA.

Part of [Rusty Demon](https://github.com/HoldMyBeer-gg/rustydemon), but usable
on its own: it only needs the bytes of a `.blp` file.

## Install

```toml
[dependencies]
rustydemon-blp2 = "0.5"
```

## Quick start

```rust,no_run
use rustydemon_blp2::BlpFile;

let blp = BlpFile::open("texture.blp")?;
println!("{}x{}, {} mip level(s)", blp.width, blp.height, blp.mipmap_count());

// Decode the base (largest) mipmap. Always RGBA, 4 bytes per pixel.
let (pixels, w, h) = blp.get_pixels(0)?;
assert_eq!(pixels.len(), (w * h * 4) as usize);
# Ok::<(), rustydemon_blp2::BlpError>(())
```

Already have the bytes in memory (for example, straight out of a CASC
archive)? Use `BlpFile::from_bytes(data)` instead of `BlpFile::open`.

## Format support

| Format      | Encoding                          | Alpha modes       |
|-------------|-----------------------------------|-------------------|
| BLP0 / BLP1 | Palette, DXT1/3/5, ARGB8888, JPEG | 0 / 1 / 4 / 8-bit |
| BLP2        | Palette, DXT1/3/5, ARGB8888, JPEG | 0 / 1 / 4 / 8-bit |

## Output

`BlpFile::get_pixels` always returns RGBA (red first, alpha last), 4 bytes per
pixel, row-major. The on-disk ARGB8888 layout is BGRA; the swap is done for
you.

## Mipmaps

A BLP file stores up to 16 mipmap levels. Level 0 is the base image and each
level after it is half the size, down to 1x1. Asking for a level past the last
one clamps to the last valid level.

## License

AGPL-3.0 with the Commons Clause. See
[LICENSE](https://github.com/HoldMyBeer-gg/rustydemon/blob/main/LICENSE) and
[COMMERCIAL.md](https://github.com/HoldMyBeer-gg/rustydemon/blob/main/COMMERCIAL.md).
