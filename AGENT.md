# Agent Guide — rustydemon

How to build, check, test, and validate changes to this codebase
without requiring the human to launch the GUI.

## Quick reference

```bash
# Type-check the whole workspace (fast, ~2s)
cargo check

# Full CI gate (what the pre-commit hook runs)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clippy -p rustydemon-lib --features cdn

# Build a specific crate
cargo check -p rustydemon-cli
cargo check -p rustydemon-gr2
cargo check -p rustydemon-blp2
cargo check -p rustydemon-lib
```

## Pre-commit hooks

The repo has a pre-commit hook that runs fmt, clippy, and tests.
Every `git commit` and `git push` triggers it. If it fails the commit
is rejected — fix the issue, re-stage, and create a NEW commit (do
not amend).

## Self-testing with `rustydemon-cli inspect`

The CLI's `inspect` subcommand lets you validate format parsing
without launching the GUI or opening a CASC archive:

```bash
# Inspect any supported file on disk
cargo run -p rustydemon-cli -- inspect path/to/file.m2
cargo run -p rustydemon-cli -- inspect path/to/file.model
cargo run -p rustydemon-cli -- inspect path/to/file.wmo
cargo run -p rustydemon-cli -- inspect path/to/file.blp
cargo run -p rustydemon-cli -- inspect path/to/file.texture

# Export mesh geometry as OBJ (works for .model, .wmo, .m2)
cargo run -p rustydemon-cli -- inspect path/to/file.model --obj /tmp/out.obj
```

This prints the same metadata the GUI preview panel shows: vertex
counts, texture lists, bounding boxes, chunk inventories, etc. Use
it after modifying any parser to verify the output makes sense.

### Known test files

The human has extracted sample files in `~/Downloads/`:
- `LadySylvanasWindrunner.m2` — WoW M2 (MD21, 4815 verts, 140 bones)
- `sylvanasshadowlands2.m2` — WoW M2 (MD21, 14711 verts, 207 bones)

## Feedback loop for format changes

1. **Edit** the parser or preview plugin
2. **`cargo check`** — catches type errors in ~2s
3. **`cargo run -p rustydemon-cli -- inspect <file>`** — verify parsed output
4. **`cargo test --workspace`** — run the full test suite
5. **Commit** — the pre-commit hook re-runs fmt + clippy + tests

This loop takes seconds, not the 5+ minutes of rebuilding and
launching the GUI.

## When you DO need the human

- Verifying 3D rendering (camera, lighting, texture appearance)
- Testing sibling file resolution (needs a live CASC archive open)
- Anything involving the egui UI layout or interaction
- Audio playback testing

## Workspace layout

```
rustydemon/              GUI binary (egui + wgpu)
  src/preview/           Format-specific preview plugins
  src/viewport3d/        wgpu 3D rendering pipeline
  src/ui/                UI panels
rustydemon-cli/          Headless CLI (export + inspect)
rustydemon-lib/          CASC archive reader (shared library)
rustydemon-blp2/         BLP texture decoder
rustydemon-gr2/          Granny3D / D2R .model reader
```

## Supported preview formats

| Format | Magic | Plugin | 3D viewport | Textures |
|--------|-------|--------|-------------|----------|
| .blp | `BLP2` | blp.rs | — | decode + PNG export |
| .pcx | PCX header | pcx.rs | — | decode + PNG export |
| .texture | `<DE(` | texture.rs | — | BC decode + PNG export |
| .tex | (D4 raw BC) | tex.rs | — | BC decode + PNG export |
| .wmo | `REVM` | model3d.rs | yes, with BLP materials | sibling groups + textures |
| .m2 | `MD20`/`MD21` | m2.rs | yes, hash-coloured | skin LOD via SFID |
| .model | Granny magic | model_d2r.rs | yes, with .texture materials | sibling textures via name |
| .pow | (D4 binary) | pow.rs | — | text summary only |
| .vid | (BK2 container) | vid.rs | — | header + .bk2 export |
| audio | WAV/MP3/OGG | audio.rs | — | metadata only |
| text | UTF-8 heuristic | text.rs | — | monospace view |

## Key architectural patterns

- **Preview plugins** implement `PreviewPlugin` trait (`can_preview` +
  `build`). Register in `preview/mod.rs::registry()`.
- **SiblingFetcher** lets plugins pull related files from the open
  archive (e.g. WMO groups, M2 skins, D2R textures).
- **Mesh3dCpu** is the CPU-side mesh handed to the wgpu viewport.
  Batches reference materials by index; empty `materials` vec triggers
  hash-coloured fallback rendering.
- **MeshCallback** (viewport3d) does offscreen wgpu rendering with a
  blit pass into the egui surface. Camera is orbit-style (drag=rotate,
  scroll=zoom).
