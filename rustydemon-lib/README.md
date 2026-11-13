# rustydemon-lib

A Rust library for reading CASC (Content-Addressable Storage Container)
archives from local game installations.

It is the engine behind [Rusty Demon](https://github.com/HoldMyBeer-gg/rustydemon),
a cross-platform CASC explorer, and has no GUI dependencies of its own.

## Install

```toml
[dependencies]
rustydemon-lib = "0.6"
```

## Quick start

```rust,no_run
use rustydemon_lib::{CascHandler, LocaleFlags, SearchQuery};

// Open a local game installation.
let mut casc = CascHandler::open_local("/path/to/game", "wow")?;
casc.set_locale(LocaleFlags::EN_US);

// Load a community listfile (optional, needed for filename-based access).
let listfile = std::fs::read_to_string("community-listfile.csv")?;
casc.load_listfile(&listfile);

// Open a file by path.
let bytes = casc.open_file_by_name(
    "Interface/Glues/Models/UI_MainMenu/UI_MainMenu.m2",
)?;
println!("File is {} bytes", bytes.len());

// Search every entry in the root manifest, not just one folder.
let results = casc.search(SearchQuery::new().filename(".blp").limit(50));
for r in results {
    println!("{:016X}  {:?}", r.hash, r.filename);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Files can also be opened by name hash, FileDataID, content key, or encoding
key: see `open_file_by_hash`, `open_file_by_fdid`, `open_by_ckey`, and
`open_by_ekey` on `CascHandler`.

Game data tables keep rows for unreleased content in blocks encrypted with
keys that are not public. `open_file_by_fdid_partial` and
`open_by_ckey_partial` return such a file with zeros in place of the blocks
that could not be decrypted, along with the names of the missing keys.

## What it reads

- Battle.net installs: `.idx` local indices, the encoding file, the root
  manifest, and BLTE-encoded `data.NNN` archives (including Salsa20-encrypted
  blocks when the key is known).
- TVFS roots, including the Diablo II: Resurrected multi-storage layout.
- The Steam distribution of Diablo IV, which ships a static container with no
  `.build.info` and no encoding file. `CascHandler::open_local` detects it
  on its own.
- Installs with an unrecognised root, through an INSTALL-manifest fallback.

## Lookup chain

```text
filename
  -> Jenkins96 hash
    -> root manifest
      -> CKey (content key)
        -> encoding file
          -> EKey (encoding key)
            -> local index (*.idx)
              -> (archive, offset, size)
                -> data.NNN
                  -> BLTE decode
                    -> raw file bytes
```

## Features

| Feature | Default | What it does |
|---------|---------|--------------|
| `cdn`   | off     | Fetches loose metadata blobs (ENCODING, DOWNLOAD, TVFS root) from the CDN when an install does not ship them locally. Plain HTTP through `ureq`, no TLS stack. |

## License

AGPL-3.0 with the Commons Clause. See
[LICENSE](https://github.com/HoldMyBeer-gg/rustydemon/blob/main/LICENSE) and
[COMMERCIAL.md](https://github.com/HoldMyBeer-gg/rustydemon/blob/main/COMMERCIAL.md).
