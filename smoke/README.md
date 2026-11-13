# Smoke scripts

Functional checks of the real desktop app, driven through the accessibility
tree with [bad-ass-mcp](https://github.com/HoldMyBeer-gg/bad-ass-mcp)'s BAM
runner. They cover what unit tests can't: the window, menus and tree as a
screen reader or automation tool sees them.

## Scripts

| script | needs a game install | covers |
|---|---|---|
| `launch-empty.json` | no | window title, toolbar, status bar text, File menu |
| `about.json` | no | Help → About: its headings, version, link cards and buttons are in the accessibility tree, and Close dismisses it |
| `load-keys-dialog.json` | no | File → Load TACT Keys… through the native open dialog: type a path, open it, status bar reports the keys loaded |
| `wowf/ghost-entries.json` | WoW Forever + listfile | issue #3: folders the build does not ship stay out of the tree; real folders expand and browse |
| `wowf/end-to-end.json` | WoW Forever + listfile | the whole app in one pass: load, browse the tree, text preview, glob search, M2 model preview, texture preview, Export As PNG through the native save dialog, Help menu |
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

`load-keys-dialog.json` runs with `bam.json` too. On Linux add
`"helpers": ["xdg-desktop-portal"]` so the run may drive the file dialog.
On macOS nothing is added: the dialog is a sheet in the app's own window.
That script sends real key presses on macOS (Cmd+Shift+G and Return), so
the window takes the foreground while it runs.

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

`wowf/end-to-end.json` uses the same config, with two additions on Linux:

```json
{
  "window": "Rusty Demon",
  "app": [
    "env", "-u", "WAYLAND_DISPLAY",
    "/path/to/target/release/rustydemon",
    "/path/to/World of Warcraft",
    "--listfile",
    "/path/to/verified-listfile.csv"
  ],
  "helpers": ["xdg-desktop-portal"],
  "ready": { "name": "Search" },
  "launch_timeout": 30
}
```

- `env -u WAYLAND_DISPLAY` runs the app under XWayland. The script types
  into the search box, and typed text only arrives there.
- `helpers` lets the run drive the save dialog. On KDE the dialog belongs to
  `xdg-desktop-portal-kde`, a process that was running before the app
  started, so the runner would otherwise leave it alone.

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
  presses once it reads the value back. The key presses select the old
  text first, so a second search replaces the first.
- Static labels (status bar, panel text) were not findable by name on macOS
  with older bad-ass-mcp versions, so `launch-empty.json` gates one check
  with `"only": ["linux", "windows"]`. With a0fe32a they resolve on macOS
  (seen through a direct lookup on 2026-11-12, not yet in a script run).
- Run from the desktop session. Over SSH or in tmux the app has no display:
  every script fails with `no window matching`. Export `DISPLAY`,
  `XAUTHORITY` (on SteamOS, `/run/user/1000/xauth_*`), `WAYLAND_DISPLAY` and
  `XDG_RUNTIME_DIR` from the desktop session first.

## Known limits

- **Names added after 0.5.2.** About is drawn inside the main window, so
  its contents are in the tree (it used to be a separate OS window with
  none). The search box is named `Search files`
  (scripts written before that find it by role: `textfield` on Windows and
  macOS, `entry` on Linux). Section headers are headings with plain names
  (`File Browser`, not the letter-spaced text on screen), the status bar is
  one `status` node, the Viewer dropdown and preview text boxes are named,
  and a texture preview is an `image` named with its size.
- **Disabled items read as enabled on Linux.** A greyed-out item such as
  Load Listfile… with no archive open is published over AT-SPI with the
  `enabled` state. egui marks it disabled; `accesskit_atspi_common` 0.9.3,
  which egui 0.29 uses, reports any control that cannot be read-only as
  enabled. A script cannot tell that such an item is disabled.
- **Linux, window lookup.** rustydemon's AT-SPI application name is empty.
  bad-ass-mcp's main branch (a0fe32a, 2026-11-12) falls back to the frame's
  name, which carries the title (also seen with 1.6.0-29-g76c9ed4 on XFCE).
  A version without that fallback reports `no window matching 'Rusty Demon'`.
- **Native file dialogs.** Export As PNG is scripted on Linux (KDE) and
  Windows in `wowf/end-to-end.json`. The dialog's title and its filename
  field differ per OS, so those values are per-platform:

  | OS | dialog window | filename field | button |
  |---|---|---|---|
  | Linux (KDE) | `xdg-desktop-portal-kde` | role `text`, focused | `Save` |
  | Windows | `Save As` | role `textfield`, name `File name:`, focused | `Save` |

  The pick-file dialog is scripted in `load-keys-dialog.json` on Linux
  (KDE), macOS and Windows, from File → Load TACT Keys…, which works with no archive
  open. The folder pickers (Export Folder, Open Game Directory) and Load
  Listfile are not scripted yet; `--listfile` covers loading. What the
  pick-file dialog looks like per desktop (XFCE recorded by hand on
  2026-11-12, the others from script runs the same day):

  | OS | dialog | path field | buttons |
  |---|---|---|---|
  | Linux (KDE) | `xdg-desktop-portal-kde`; needs `"helpers": ["xdg-desktop-portal"]` | role `text`, focused | `Open`, `Cancel` |
  | Linux (XFCE, GTK portal) | listed as `xdg-desktop-portal-gtk`, no title; needs `"helpers": ["xdg-desktop-portal"]`, which matches the KDE portal too | hidden until Ctrl+L or a leading `/` is typed, then role `text`, focused | `Select` (disabled until a path is entered), `Cancel` |
  | macOS | a sheet named `Open` inside the main window, same process, no helpers; the phase's `window` is `null` there | none until Cmd+Shift+G, then role `textfield`, name `text field`, focused; Return selects the typed file in the panel | `Open` (disabled until a file is chosen; learn it with role `button`, the sheet shares the name), `Cancel` |
  | Windows | the app's own top-level window titled `Open`, same process, no helpers | role `textfield`, name `File name:`, focused (a combobox and a label share the name) | `Open`: learn it with role `splitbutton`, because the window and the two comboboxes' dropdown arrows are also named `Open` (role `button`); `Cancel` |

  On macOS, send the dialog's keys without a `window_id`: a key posted to
  the app's process never reaches an Open panel, and still reports ok. A
  plain Escape sent that way closes the Go-to sheet but not the Open sheet;
  a global Escape or a click on `Cancel` closes it.

  On Windows the Open button reports role `button` until the dialog has
  loaded its folder view, then `splitbutton`. bad-ass-mcp before PR #36
  took its proof screenshot of the main window with `PrintWindow`, which
  stalled that load for good ("Working on it…"), so the script failed at
  learning the button.
- **Linux, no dialog appears.** The portal is started by D-Bus with systemd's
  user environment. If that has a different `DISPLAY` than the session (seen
  on a remote-desktop XFCE box), the portal cannot open its window, the app
  shows no error, and the request stays pending. Compare
  `systemctl --user show-environment` with the session's `DISPLAY`.
- **Windows needs a recent bad-ass-mcp.** The script types into the search
  box twice, and the dialog is given a path under `$SMOKE`. Older versions
  appended the second search to the first, and left `$SMOKE` relative when
  `--results` was, so the dialog saved somewhere else. Both are fixed in
  bad-ass-mcp ed0374d (2026-11-12).
