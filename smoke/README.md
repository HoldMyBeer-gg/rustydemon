# Smoke scripts

Functional checks of the real desktop app, driven through the accessibility
tree with [bad-ass-mcp](https://github.com/HoldMyBeer-gg/bad-ass-mcp)'s BAM
runner. They cover what unit tests can't: the window, menus and tree as a
screen reader or automation tool sees them.

## Running

Build first (`cargo build --release -p rustydemon`), then from the repo root:

```
bad-ass-mcp run smoke/ --config bam.json
```

`bam.json` is not committed, because the binary path differs per machine:

```json
{
  "window": "Rusty Demon",
  "app": {
    "linux": "/path/to/target/release/rustydemon",
    "macos": "/path/to/target/release/rustydemon",
    "windows": "C:\\path\\to\\target\\release\\rustydemon.exe"
  },
  "ready": { "name": "Search" },
  "launch_timeout": 30
}
```

`app` makes the run launch its own copy and drive only that one, so a
rustydemon you already have open is left alone. Results and `report.html`
land in `bam-results/`. The script format is documented in bad-ass-mcp's
`docs/bam.md`.

## Scripts

| script | needs a game install | covers |
|---|---|---|
| `launch-empty.json` | no | window title, toolbar, status bar text, File menu |

## Conventions

- Find elements by accessible **name**. Every row in the file browser and
  the folder/search list publishes its file name; folder carets are
  `Expand <name>` / `Collapse <name>`.
- Don't rely on keys to drive the UI. A key sent through the accessibility
  API does not reach egui on native Wayland, so scripts click instead.
- `verified` records the platforms a script has actually passed on, with
  the date. A platform without an entry is written from the code, not a run.

## Known limits

- **Linux, window lookup.** The runner matches windows by AT-SPI application
  name. egui does not set one, so rustydemon's is empty and
  `"window": "Rusty Demon"` finds nothing until bad-ass-mcp falls back to
  the frame's name, which does carry the title.
- **Linux, file dialogs.** On KDE the open/save dialogs belong to
  `xdg-desktop-portal-kde`, a process that exists before the app starts.
  With `app` set the runner only drives the launched process, so flows that
  load a listfile or pick an export folder can't be scripted yet. Those
  flows are covered by `rustydemon-cli export` and by hand for now.
