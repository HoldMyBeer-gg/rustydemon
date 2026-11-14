# Diablo Immortal: MPK packages and Messiah assets

Diablo Immortal is built on NetEase's Messiah engine. Its content does not
live in CASC. Battle.net installs it as product `anbs` and lays down a CASC
style skeleton, but every asset ships in Messiah MPK packages under
`Package/MPK/`. This note records what rustydemon reads from those packages
and where in the code each part is read, so the note can be checked against
the code.

All integers are little-endian. Counts and figures below come from a full
pass over a live Windows install with the crates as of v0.7.0.

## Where the files live

A default Battle.net install sits at `C:\Program Files (x86)\Diablo Immortal\`.

```
Package/MPK/Engine.mpkinfo        index for the Engine package
Package/MPK/Engine.mpk            its one data file
Package/MPK/Resources.mpkinfo     index for the Resources package
Package/MPK/Resources.mpk         data file 0
Package/MPK/Resources1.mpk ...    data files 1, 2, ...
```

There is no `.build.info`. rustydemon recognises the install by the
presence of `Package/MPK/Resources.mpkinfo` and names the product `anbs`.

| Step | Code |
|---|---|
| Detect the product | `rustydemon-lib/src/config.rs`, `CascConfig::detect_products` |
| Route the install to the MPK path | `rustydemon-lib/src/handler.rs`, `CascHandler` open path, then `finish_mpk` |
| Config for an MPK install | `rustydemon-lib/src/config.rs`, `CascConfig::for_mpk` |

On the install checked, the two indexes list 737,252 files across 113 data
files: `Engine.mpk` and 112 `Resources` data files.

## The `.mpkinfo` index

```
u32  version (1)
u32  entry count
entry * count:
  u16  name length
  [n]  name, UTF-8, '/' separated (e.g. Package/Char/npc/a_drink.anim)
  u32  offset into the data file
  u32  stored size
  u32  field: data file number << 1 | flag
[16] MD5 of every byte before it
```

An entry maps to its bytes like this:

- `field >> 1` is the data file number. 0 is `<Name>.mpk`, n is
  `<Name>n.mpk`. The `.mpkinfo` stem gives `<Name>`.
- Seek to `offset` in that data file and read `stored size` bytes.
- Decode the payload (next section).

The low bit of `field` is set on 15,230 entries. Its meaning is not known.

| Step | Code |
|---|---|
| Parse an index and check its MD5 | `rustydemon-mpk/src/lib.rs`, `parse_index` |
| Open every `.mpkinfo` in a folder | `rustydemon-mpk/src/lib.rs`, `MpkArchive::open` |
| Entry to data file path | `rustydemon-mpk/src/lib.rs`, `MpkArchive::data_file` |
| Read and decode one entry | `rustydemon-mpk/src/lib.rs`, `MpkArchive::read` |

MPK has no content keys. rustydemon gives each entry a synthetic key, the
bytes `MPK\0` followed by its index, so the CASC side of the app can hold
it: `rustydemon-lib/src/root/mpk.rs`, `key_for` and `entry_index`.

## Payloads

A stored payload is one of:

```
"ZZZ4"  u32 decompressed size  LZ4 block      compressed
anything else                                 the file's bytes as they are
```

The LZ4 data is a single raw block (no LZ4 frame header).

One file differs: the resource registry (below) is wrapped as

```
"CCCC"  "ZZZ4"  u32 decompressed size  LZ4 block  stray bytes
```

A few bytes follow the last LZ4 sequence, so a standard block decoder
rejects it. rustydemon decodes until the stated size is reached and ignores
the rest.

| Step | Code |
|---|---|
| Decode any payload | `rustydemon-mpk/src/lib.rs`, `decode_payload` |
| Decode to a stated size, ignoring the tail | `rustydemon-mpk/src/lib.rs`, `lz4_block_until` |

## Resource registry

`Package/resource.repository` names every resource. Without it, most files
are only GUIDs: `Package/ab/abcdef01-2345-...`, where `ab` is the first two
hex digits of the GUID. Textures add `.6`.

```
u32  version (1)
u32  entry count
u16  0
u16  length, type names joined by ';'      (Texture2D;ParticleSystem;...)
u16  0xFFFF, u32 length, folder names joined by ';'
entry * count:
  [5]  unknown
  [16] GUID
  u16  length, name
  u16  folder index
  u16  type index
  u16  dependency count
  [16] * dependency count: GUIDs
```

The folder name `@^_^@` means no folder. The GUID's bytes print in order
as `8-4-4-4-12` hex digits to give the package file name.

rustydemon builds a readable path for each resource as
`folder/name.type`, with the type lowercased, and lists the file under both
names. For example a mesh lives at `Package/62/6225ff69-...` and also shows
as `Char/new_dl_f_warlock/Mesh_warlock_yuanchuemo_body.mesh`.

On the install checked the registry holds 586,531 entries. The largest
types are Texture2D (190,691), Material (122,177), Mesh (90,353), Model
(73,505), ParticleSystem (46,103), SkinSkeleton (29,407) and LodModel
(23,372).

Dependencies run downward: a Model depends on its Meshes and Materials, a
Material on its Textures. To find a mesh's textures rustydemon walks
mesh, then the Models that depend on it, then those Models' Materials,
then each Material's base map.

| Step | Code |
|---|---|
| Parse the registry | `rustydemon-messiah/src/repository.rs`, `Repository::parse` |
| GUID to package path | `rustydemon-messiah/src/repository.rs`, `package_path`, `uuid_string`, `parse_uuid` |
| Readable path | `rustydemon-messiah/src/repository.rs`, `Repository::readable_path` |
| Either path back to a GUID | `rustydemon-messiah/src/repository.rs`, `Repository::guid_for_path` |
| Mesh to its Materials | `rustydemon-messiah/src/repository.rs`, `Repository::materials_for_mesh` |
| List files under both names | `rustydemon-lib/src/root/mpk.rs`, `MpkRootHandler::new` |
| Load it with the install | `rustydemon-lib/src/handler.rs`, `finish_mpk` |

## Meshes

A mesh is a `.MESSIAH` object:

```
[8]  ".MESSIAH"
u32  version (8)
u32  unknown
u16  flag: 0 plain, 2 quantised
u16  submesh count
u32  vertex count
u32  index count
4 *  (u16 length, layout name)    unused slots say "None"
6 f32  bounding box min, max
4 f32  bounding sphere centre, radius
submesh count * 4 u32: start index, index count, start vertex, vertex count
indices: u16, or u32 when there are more than 65535 vertices
one vertex stream per layout that is not "None", vertex count entries each
submesh count * 52 bytes of per-submesh bounds
```

Indices are absolute into the vertex list, not relative to the submesh's
start vertex.

### Layout names

A layout name is elements joined by `_`. Each element is three characters:
a meaning letter, a component count (1 to 4), and a type letter.

| Meaning | | Type | |
|---|---|---|---|
| P | position | F | f32 |
| N | normal | H | f16 |
| T | texture coordinate | B | byte |
| Q | tangent frame | | |
| C | colour | | |
| W | bone weights | | |
| I | bone indices | | |

Layouts seen: `P3F_N4B_T2F`, `P3F`, `Q4H_T2H`, `W4B_I4B`, `C4B_Q4H_T2H`.
The first layout always starts with `P3F`, which is what the cheap check
for a mesh looks for.

### Quantised meshes (flag 2)

With flag 2, the indices and then each element of each stream, in layout
order, are stored as packed runs:

```
u32  value count
f32  range, f32 minimum     (F and H elements only)
u8   bits per value
values packed least significant bit first, rounded up to whole bytes
```

A float element's value is `minimum + q / (2^bits - 1) * range`. Index and
byte runs carry no range or minimum and are used as they are.

On the install checked, 90,352 of 90,353 meshes parse. The one that fails
uses the layout `P4F_T4Fx2_I`, whose `x2` and bare `I` tokens do not fit
the three character element form.

| Step | Code |
|---|---|
| Cheap check | `rustydemon-messiah/src/lib.rs`, `looks_like_mesh` |
| Parse a mesh | `rustydemon-messiah/src/lib.rs`, `Mesh::parse` |
| Layout names | `rustydemon-messiah/src/lib.rs`, `parse_layout` |
| Plain streams | `rustydemon-messiah/src/lib.rs`, `read_plain` |
| Quantised streams | `rustydemon-messiah/src/lib.rs`, `read_quantised`, `read_packed` |
| 3D preview and OBJ export | `rustydemon/src/preview/messiah.rs`, `MessiahMeshPreview` |

## Textures

A texture is `Package/ab/<guid>.6`:

```
[4]  02 02 02 01
u8   1
u8   format
u8   mip count
u8   flags
[4]  unknown
u16  width
u16  height
[16] unknown
u32  size of the rest
[4]  1, format, mip count, a fourth byte
mip * count, smallest first:
  u32  entry size, counting itself
  u16  width
  u16  height
  u16  unknown
  u16  row pitch
  u32  data size
  then "NNNN" + data, or "ZZZ4" + u32 size + an LZ4 block
```

Mip data begins at byte 40. Every texture seen so far uses format `0x19`,
which is BC7.

### Finding the top mip

The mips are stored smallest first, so the full size image is the last
one. rustydemon picks the mip with the most pixels rather than trusting
the order or the file name.

The file name does not give the size. Worked example from the install
checked:

```
Textures/Landscape_Surfaces/Albedo_2K_tbljaifk.texture2d

02 02 02 01  01 19 08 08  00 00 00 01  00 02 00 02  ...
             |  |  |  |               |     |
             |  |  |  flags 0x08      |     height 0x0200 = 512
             |  |  mip count 8        width 0x0200 = 512
             |  format 0x19 (BC7)
             1

mips: 4x4, 8x8, 16x16, 32x32, 64x64, 128x128, 256x256, 512x512
```

The name says 2K, but the file holds eight mips and the top one is 512 by
512. The header width and height agree with the top mip. A sibling,
`Megascans/Surfaces/Old_Fort_Stone_Wall_tfhgbipn/Albedo_2K_tfhgbipn.texture2d`,
holds ten mips and does reach 2048 by 2048 (header `0a` mips, width and
height `0x0800`).

In the repeated block at byte 36 the fourth byte read `00` on both files,
while the flags byte at offset 7 read `08`. What either byte means is not
known.

| Step | Code |
|---|---|
| Cheap check | `rustydemon-messiah/src/texture.rs`, `looks_like_texture` |
| Parse header and mips | `rustydemon-messiah/src/texture.rs`, `Texture::parse` |
| Pick the top mip | `rustydemon-messiah/src/texture.rs`, `Texture::largest_mip` |
| BC7 decode for preview and export | `rustydemon/src/preview/messiah.rs`, `decode_texture` |

## Materials

A material is a `.MESSIAH` object holding one parameter block per
sub-material. Each block is a run of text lines `key=value`. Sub-material
n paints submesh n. rustydemon reads only `tBaseMap=<texture guid>` from
each block, the base colour texture; an all zero GUID means no texture.

Meshes whose materials have an empty `tBaseMap` (effect and fresnel meshes)
show untextured. That is what the data says, not a reader fault.

| Step | Code |
|---|---|
| Parse base maps | `rustydemon-messiah/src/material.rs`, `Material::parse` |
| Submesh to texture | `rustydemon/src/preview/messiah.rs`, `submesh_materials` |

## Glossary

Resource names are romanised Chinese (pinyin), e.g.
`Mesh_m_crusader_toukui_half_sz06_009` (`toukui` is helmet). rustydemon
glosses about 65 known words into English in the file list. A name with
none of them gets no gloss rather than a guess. For `.texture2d` names the
last token also names the map: `_d` colour, `_n` normal, `_e` glow, `_m`
mix.

| Step | Code |
|---|---|
| Word list and lookup | `rustydemon-messiah/src/glossary.rs`, `gloss` |
| Shown in the file list | `rustydemon/src/ui/results.rs`, `row_meta_glossed` |

## Not known yet

Each of these is unknown, not guessed at:

- The low flag bit in an `.mpkinfo` entry's archive field.
- The 5 bytes before each registry entry's GUID.
- The mesh header's second u32, and the contents of the 52 bytes of
  per-submesh bounds at the end of a mesh.
- The mesh layout `P4F_T4Fx2_I` (one mesh on the install checked).
- Texture header: the 4 bytes at offset 8, the 16 bytes at offset 16, the
  flags byte, the fourth byte of the repeated block, and the u16 between
  height and row pitch in each mip header.
- Texture formats other than `0x19`. None have been seen.
- Materials beyond `tBaseMap`: the other keys, and the binary data between
  the text blocks. There is no material preview, so `.material` files show
  as hex; the mesh preview reads them only for their base maps.
- File types rustydemon does not decode, which show as hex. By count in
  the indexes: `.anim` (38,546), `.graph` (21,594), `.plist` (14,115),
  `.csb` (8,779), `.bnk` (8,302), `.skeleton` (6,785), `.1` (6,298),
  `.human` (4,859), `.ets` (4,445), `.uis` (1,466), `.wem` (662), `.mont`
  (510), `.cache` (385), `.0` (257), `.motion` (231), and the registry
  types other than Mesh, Texture2D and Material (Model, ParticleSystem,
  SkinSkeleton, LodModel, CollisionShape and the rest).
