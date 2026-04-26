# D4 `.vid` (Movie) — child reference stubs

Investigation triggered by `vid_preview` reporting "did not start with the
expected MOVI magic" on every `.vid` file in a Steam D4 install.

## What we observed

Every `.vid` SNO in `base/child/Movie/...` resolves to exactly **52 bytes**
beginning with `0xDEADBEEF` (little-endian `EF BE AD DE`).  The 128-byte
`MOVI`-prefixed format that `vid_preview.rs` was written for is **not** what
ships in the named TVFS tree — those `.vid` entries are redirection records,
not BK2 containers.

`base/meta/Movie/<name>.vid` exists alongside `base/child/Movie/<name>-0.vid`
and is **byte-identical** to the child.  No `base/payload/Movie/` entries
exist at all (compare to e.g. `MarkerSet`, `Texture`, `Power` which all have
real payload trees).

## Stub layout (52 bytes, all values little-endian)

```
0x00  u32   magic         = 0xDEADBEEF
0x04  u32   reserved?     = 0
0x08  u64   reserved?     = 0
0x10  u32   field_a       (varies per file)
0x14  u32   reserved?     = 0
0x18  u32   field_b       (varies, often field_a + 1)
0x1C  u32   field_c       (sometimes 0xFFFFFFFF, sometimes another small int)
0x20  u32   field_d       (sometimes 0xFFFFFFFF, sometimes another small int)
0x24  u8    flags?        (observed 0xD1 or 0xDF)
0x25  u8    reserved      = 0
0x26  u16   reserved      = 0
0x28  u32   maybe_count?  (usually 0 or 1)
0x2C  u32   maybe_index?  (usually 0 or 1)
0x30  u32   trailer       (usually 0)
```

### Sample values

| Movie | field_a | field_b | field_c | field_d | flags |
|---|---|---|---|---|---|
| `Blizz-Logo` | `0x0016A6C5` | `0x0016A6C6` | `0xFFFFFFFF` | `0xFFFFFFFF` | `0xD1` |
| `DIA_FenrisAnnounce` | `0x00073077` | `0x00072FC0` | `0x00074932` | `0x00074933` | `0xDF` |
| `DX1_Campfire_Barb` | `0x001D8AA3` | `0xFFFFFFFF` | `0xFFFFFFFF` | `0xFFFFFFFF` | `0xD1` |
| `Axe Bad Data` | `0x00072F76` | `0xFFFFFFFF` | `0xFFFFFFFF` | `0xFFFFFFFF` | `0xD1` |

`field_a` doesn't match any SNO ID directly (`**/<field_a>*` returns 0
matches).  `field_b` *sometimes* matches a Speech SNO (e.g. `0x16A6C6 =
1484486` resolves to `*Speech/meta/1484486` in 7 locales) but not always —
suggesting these are not SNO IDs but a different kind of index.

The `0xD1` vs `0xDF` flag and the variable number of populated fields
(`Blizz-Logo` and the campfires use 1 active field, `FenrisAnnounce` uses 4)
suggest a count-prefixed array where unused slots are `0xFFFFFFFF`.

## Where the actual BK2 lives — open question

Hypotheses, none verified:

1. A separate streaming-archive layer that is *not* indexed by the SNO TVFS
   names.  Either resolved by content-key (the engine looks up the stub
   fields against a CDN-style `.index` we haven't enumerated) or by a
   sibling container the launcher places in a Movie-specific directory.
2. The `field_a..field_d` values are 32-bit truncations of EKeys, used to
   look up a dedicated movie-archive index.
3. The BK2 streams are downloaded on-demand from CDN and are not present in
   a local install at all — would explain why the user sees ~half the
   `.vid` SNOs report `index entry not found for ekey static container`
   (those are stubs whose data file was reaped or never downloaded).

A definitive answer probably needs:

- Capturing the engine fetching a movie at runtime (lsof/strace on the
  process while playing one)
- Or finding TACTLib / DataTool's D4 Movie extractor — neither
  `OWLib/TACTLib` nor `OWLib/TankLib` here have a D4-specific Movie path;
  TACTLib's `0xDEADBEEF` references are unrelated sentinels in
  `ContainerHandler.cs`.

## What the preview plugin does today

`rustydemon/src/preview/vid.rs` now detects the 52-byte `DEADBEEF` stub
and reports it as a redirection record rather than crashing the user
through "did not start with the expected MOVI magic".  Real BK2 fetch
requires the redirection layer above, which is not implemented.
