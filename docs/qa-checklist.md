# QA checklist (M6)

The manual checks M6 needs (SPEC.md §7 M6 Verification). Only Windows can
be run for now; the macOS and Linux columns stay empty until a machine for
each is available, and M6 is not signed off on them until then.

Mark each cell ✅ / ❌ / ⚠️ with the date, and put the evidence in
`docs/progress.md`. Release build unless the row says otherwise.

## Isolated dev instance

Most rows run against a throwaway instance so they cannot touch the real
index, config or registry entries (Git Bash):

```sh
export USERNAME=magi-qa                    # own single-instance pipe
export MAGI_DATA_DIR=$TEMP/magi-qa/data
export MAGI_CONFIG_DIR=$TEMP/magi-qa/config
export MAGI_FAKE_EMBEDDER=1                # drop for rows that need real models
cargo run -p magi-desktop --release
```

`magi --toggle` must be run with the same `USERNAME` to reach this instance.
Launch at login is the exception: it writes the real `HKCU\…\Run` key.

## Checks

| # | Check | How | Windows | macOS | Linux |
|---|-------|-----|---------|-------|-------|
| 1 | Hotkey toggles search | Press the shortcut twice: opens, closes. | ✅ 2026-09-29 | | |
| 2 | `--toggle` and single instance | `magi --toggle` opens search in the running app; a plain second launch opens settings and exits. | ✅ 2026-09-29 (`--toggle`) | | |
| 3 | Open and Reveal | Enter opens, Ctrl/Cmd+Enter reveals; on a deleted file both keep the window. | ✅ 2026-09-29 | | |
| 4 | Hides on blur and Esc | Click another window; press Esc. | ✅ 2026-09-29 | | |
| 5 | Multi-monitor placement | Two displays: move the cursor to the second, press the shortcut; search opens centered there. | not run (one monitor) | | |
| 6 | HiDPI | Display scale 150 % then 200 % (Settings › System › Display): text crisp, search and settings windows scale, nothing clipped. | | | |
| 7 | IME / dead keys | Type `canción` with the dead `'` key; an IME (e.g. Japanese) composes in place. | ✅ dead keys 2026-09-29; IME not run | | |
| 8 | Works with no tray | Linux GNOME without AppIndicator: plain launch opens settings, hotkey opens search, Settings › General › Quit exits. | n/a | n/a | |
| 9 | Window first frame < 150 ms | `search window first frame elapsed=` in `magi.log`, 10 samples. | ✅ 2026-09-29, max 57 ms | | |
| 10 | Results < 400 ms after typing, warm | `search results after typing stopped elapsed=` | ✅ 2026-09-30, 154–214 ms | | |
| 11 | Idle RSS ≤ 150 MB | Window closed, models unloaded; Task Manager › Details › Memory (private working set). | ✅ ~24 MB | | |
| 12 | Software rendering | See "No GPU" below. | not run | | |
| 13 | Transparency effects | Off in the OS → search and settings are solid; turning it on/off **while settings is open** updates the window when you come back to it. | ✅ off → solid 2026-09-29; live change not run | | |
| 14 | Hotkey recorder | Settings › General › Change, press Ctrl+Alt+K: saved, works at once, no restart. Esc cancels. Same from onboarding's finish screen. | | | |
| 15 | Hotkey conflict | See "Hotkey conflicts" below. | | | |
| 16 | Real feature download | Isolated instance without `MAGI_FAKE_EMBEDDER`; see "Downloads" below. | | | |
| 17 | Launch at login | See "Launch at login" below. | | | |
| 18 | Remove folder | Trash on a folder asks first; Cancel keeps it, Remove removes it (and the folders it absorbed). | | | |
| 19 | Unreadable list live | Settings › Index open; drop a broken PDF (e.g. `echo x > bad.pdf`) into a root: it appears without reopening; Retry all updates it. | | | |
| 20 | Light-mode see-through | Light theme, blurred background: every step of the slider changes the window, from "More solid" (opaque) to "More transparent". | | | |
| 21 | Dark-mode switch contrast | Dark theme: an off switch is clearly visible on its box. | | | |
| 22 | Spanish and English | `ui.language = es`, then `en`, from settings: every window relabels live; tray too. | ✅ es 2026-09-30 | | |
| 23 | Startup failure | Isolated instance with a folder where the database should be (`mkdir $MAGI_DATA_DIR/magi.db`): "Magi couldn't start" with the error; Show log reveals `magi.log`; Quit and closing both exit. | ✅ 2026-10-05 except Show log | | |
| 24 | Onboarding setup | `--onboarding`: drag a folder from Explorer/Finder onto the drop zone, it is added (a dropped file is not); every icon draws; ⓘ opens what a feature does; the button reads "Download {size} and start", or "Start" with every feature off; light and dark, English and Spanish: no name cut off, no scrolling in the features column. | | | |

## Hotkey conflicts

The conflict path runs when `RegisterHotKey` fails or the combination is
one the OS reserves. Three ways to make it fail on purpose, no extra tools
needed for the first two:

1. **Windows' own shortcuts.** Record `Win+E` or `Win+R`: Explorer holds
   them, so the box must say "Another app or the system uses this shortcut"
   and the old shortcut must keep working. `Alt+Space` must be refused too
   (reserved, it registers but belongs to the window menu).
2. **A second Magi.** Start the isolated instance above with
   `ui.hotkey = "Ctrl+Alt+K"` in its `config.toml`. In your normal Magi,
   record Ctrl+Alt+K: refused. Close the isolated instance and record it
   again: accepted.
3. **Another app.** Any app with a global shortcut works, e.g. an
   AutoHotkey v2 script with the single line `^!j::MsgBox "taken"`, then
   record Ctrl+Alt+J.

Also check the startup case: put a taken combination in `ui.hotkey` (method
2 or 3), start Magi, open Settings › General: the warning box is there
with that combination, and recording a free one clears it.

## Downloads

On the isolated instance with real models (no `MAGI_FAKE_EMBEDDER`), from
both onboarding (`cargo run -p magi-desktop --release -- --onboarding`) and
Settings › Search features:

- Turn on (size): the progress bar moves, then "Installed", then the
  backfill counts down.
- Turn the network off halfway: a failed state with a reason and Try again;
  no `Installed` asset left behind (check `MAGI_DATA_DIR/models`).
- Remove download: the model files go; searching by meaning stops, the
  indexed data stays (turning it back on re-downloads but does not re-embed).

## Launch at login

This writes your real registry, so do it on your own account and undo it:

- Switch on Settings › General › Start with your computer. Check
  `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run` lists Magi
  with `--background`, and Task Manager › Startup apps shows it.
- Sign out and in: Magi is running (tray icon, hotkey works) with no window.
- Switch off: the `Run` value is gone.

## No GPU

Windows 11 Home has no Hyper-V or Windows Sandbox, so:

- **VirtualBox VM** with 3D acceleration off (Windows then uses the
  Microsoft Basic Render Driver, i.e. software rendering): install the
  release build, open search and settings. If it does not render, the GPU
  requirement in SPEC.md §1 is the documented reason.
- A cheaper first try on the host: disable the GPU in Device Manager (it
  falls back to the Basic Display Adapter), run Magi, re-enable. Only if
  you are fine with a flicker and a low resolution for a minute.
