# Smoke scripts

Functional checks of the real desktop app, driven through the accessibility
tree with [bad-ass-mcp](https://github.com/HoldMyBeer-gg/bad-ass-mcp)'s BAM
runner. They cover what unit tests can't: the window, menus and tree as a
screen reader or automation tool sees them.

## Scripts

| script | needs a game install | covers |
|---|---|---|
| `launch-empty.json` | no | window title, toolbar, status bar text, File menu |
| `wowf/ghost-entries.json` | WoW Forever + listfile | issue #3: folders the build does not ship stay out of the tree; real folders expand and browse |
| `wowf/end-to-end.json` | WoW Forever + listfile | the whole app in one pass: load, browse the tree, text preview, glob search, M2 model preview, texture preview, extraction to disk through `rustydemon-cli`, Help menu |
| `wow-menu/sylvanas-spin.json` | detected WoW install + listfile | File → Open Game, glob search, M2 preview, orbiting the 3D viewport by drag (Windows) |

Each script's `verified` field lists the platforms it has actually passed
on, with the date. A platform without an entry is written from the code,
not from a run.

## Running

Build first (`cargo build --release -p rustydemon`), then from the repo root:

```
bad-ass-mcp run smoke/ --config bam.json
bad-ass-mcp run smoke/wowf/ --config bam-wowf.json
bad-ass-mcp run smoke/wow-menu/ --config bam-wow-menu.json
```

The configs are not committed, because paths differ per machine.

`bam.json`:

```json
{
  "window": "Rusty Demon",
  "app": "/path/to/target/release/rustydemon",
  "ready": { "name": "Search" },
  "launch_timeout": 30
}
```

`bam-wowf.json` is the same, with the game directory and listfile on the
app's command line so no file dialog is needed:

```json
{
  "window": "Rusty Demon",
  "app": [
    "/path/to/target/release/rustydemon",
    "/path/to/World of Warcraft",
    "--listfile",
    "/path/to/verified-listfile.csv"
  ],
  "ready": { "name": "Search" },
  "launch_timeout": 30
}
```

`wowf/end-to-end.json` uses the same config, with two additions.

Its extraction phase runs `rustydemon-cli`, and reads where things are from
the environment:

```
export RUSTYDEMON_CLI=/path/to/target/release/rustydemon-cli
export WOW_DIR="/path/to/World of Warcraft"
export WOW_LISTFILE=/path/to/verified-listfile.csv
```

On Linux it types into the search box, and typed text only arrives when the
app runs under XWayland. Start the `app` list with `env -u WAYLAND_DISPLAY`:

```json
"app": [
  "env", "-u", "WAYLAND_DISPLAY",
  "/path/to/target/release/rustydemon",
  "/path/to/World of Warcraft",
  "--listfile",
  "/path/to/verified-listfile.csv"
]
```

That script sends real key presses, so the window takes the foreground while
it runs, and the run stops if you touch the keyboard or mouse.

`bam-wow-menu.json` passes only the listfile, because the script opens
the game itself from File → Open Game. The listfile waits and loads when
the archive opens:

```json
{
  "window": "Rusty Demon",
  "app": [
    "/path/to/target/release/rustydemon",
    "--listfile",
    "/path/to/verified-listfile.csv"
  ],
  "ready": { "name": "Search" },
  "launch_timeout": 30
}
```

`app` makes the run launch its own copy and drive only that one, so a
rustydemon you already have open is left alone. Results and `report.html`
land in `bam-results/`. The script format is documented in bad-ass-mcp's
`docs/bam.md`.

### The `window` value differs per OS

The runner matches `window` against whatever the platform backend calls the
window, and that is not the same thing everywhere:

| OS | matched against | use |
|---|---|---|
| Windows | window title | `"Rusty Demon"` |
| macOS | process name | `"rustydemon"` |
| Linux | AT-SPI application name, then the frame's title | `"Rusty Demon"` |

## Conventions

- Find elements by accessible **name**. Every row in the file browser and
  the folder/search list publishes its file name; folder carets are
  `Expand <name>` / `Collapse <name>`.
- Names can repeat once folders are expanded. WoW Forever has a nested
  `interface/interface`, so after expanding the root `interface` there are
  two rows with that name and `Expand interface` refers to the child. Check
  for `Collapse <name>` after expanding, and avoid acting on a name that a
  child folder may share.
- Keep to the keys the runner knows. bad-ass-mcp 1.6 rejects a script with
  an unknown key; put free-form remarks in `note`, `notes` or `covers`.
- Don't use keys to drive the UI. A key sent through the accessibility API
  does not reach egui on native Wayland, so scripts click instead.
- `type` on the search box works on Windows: egui ignores the
  accessibility "set value" call, and bad-ass-mcp falls back to real key
  presses once it reads the value back. On the way it tries a clipboard
  paste, which currently leaves the typed text on the clipboard.
- Static labels (status bar, panel text) are not findable by name on macOS,
  so gate those checks with `"only": ["linux", "windows"]`.

## Known limits

- **The About window.** Help → About opens a separate egui viewport, and
  only the main window publishes an accessibility tree. Nothing inside About
  can be found or clicked, by a script or by a screen reader.
- **The search box has no name.** It is found by role: `textfield` on
  Windows, `entry` on Linux.
- **Linux, window lookup.** rustydemon's AT-SPI application name is empty.
  bad-ass-mcp's main branch (a0fe32a, 2026-11-12) falls back to the frame's
  name, which carries the title. A version without that fallback reports
  `no window matching 'Rusty Demon'`.
- **Native file dialogs.** Flows that pick a folder or file through a dialog
  (Export, Load Listfile from the menu) are not scripted. On KDE the dialog
  belongs to `xdg-desktop-portal-kde`, a process that exists before the app
  starts, and the runner only drives the process it launched. `--listfile`
  covers loading; export is checked with `rustydemon-cli export`.
