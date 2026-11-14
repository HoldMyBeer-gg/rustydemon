# rustydemon-mpk

Reads the MPK packages of NetEase's Messiah engine, as shipped by Diablo
Immortal under `Package/MPK/`.

```no_run
use rustydemon_mpk::MpkArchive;

let archive = MpkArchive::open("Diablo Immortal/Package/MPK")?;
let i = archive
    .entries()
    .iter()
    .position(|e| e.path.ends_with("a_drink.anim"))
    .expect("present");
let bytes = archive.read(i)?;
println!("{} bytes", bytes.len());
# Ok::<(), rustydemon_mpk::MpkError>(())
```

Part of [Rusty Demon](https://github.com/HoldMyBeer-gg/rustydemon).
