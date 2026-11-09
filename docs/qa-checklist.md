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
| 1 | Hotkey toggles search | Press the shortcut twice: opens, closes. | ✅ 2026-09-29 | ✅ 2026-11-06 CI (Shift+Cmd+Space opens search) | |
| 2 | `--toggle` and single instance | `magi --toggle` opens search in the running app; a plain second launch opens settings and exits. | ✅ 2026-09-29 (`--toggle`) | ✅ 2026-11-06 CI (second launch exits; --toggle after a force-quit) | |
| 3 | Open and Reveal | Enter opens, Ctrl/Cmd+Enter reveals; on a deleted file both keep the window. | ✅ 2026-09-29 | | |
| 4 | Hides on blur and Esc | Click another window; press Esc. | ✅ 2026-09-29 | | |
| 5 | Multi-monitor placement | Two displays: move the cursor to the second, press the shortcut; search opens centered there. | not run (one monitor) | | |
| 6 | HiDPI | Display scale 150 % then 200 % (Settings › System › Display): text crisp, search and settings windows scale, nothing clipped. | | | |
| 7 | IME / dead keys | Type `canción` with the dead `'` key; an IME (e.g. Japanese) composes in place. | ✅ dead keys 2026-09-29; IME not run | | |
| 8 | Works with no tray | Linux GNOME without AppIndicator: plain launch opens settings, hotkey opens search, Settings › General › Quit exits. | n/a | n/a | |
| 9 | Window first frame < 150 ms | `search window first frame elapsed=` in `magi.log`, 10 samples. | ✅ 2026-09-29, max 57 ms | ✅ 2026-11-06 CI, worst 86 ms | |
| 10 | Results < 400 ms after typing, warm | `search results after typing stopped elapsed=` | ✅ 2026-09-30, 154–214 ms | ✅ 2026-11-06 CI, 3 ms | |
| 11 | Idle RSS ≤ 150 MB | Window closed, models unloaded; Task Manager › Details › Memory (private working set). | ✅ ~24 MB | ❌ 2026-11-06 CI: footprint 346 MB idle, no window, no models (RSS 525 MB) | |
| 12 | Software rendering | See "No GPU" below. | not run | | |
| 13 | Transparency effects | Off in the OS → search and settings are solid; turning it on/off **while settings is open** updates the window when you come back to it. | ✅ off → solid 2026-09-29; live change both ways 2026-10-11 (back to see-through ~3 s after refocus) | ⚠️ 2026-10-13 (v0.1.0 photos): dark search and settings see-through over the wallpaper; light settings looks solid near "Más transparente", check | |
| 14 | Hotkey recorder | Settings › General › Change, press Ctrl+Alt+K: saved, works at once, no restart. Esc cancels. Same from onboarding's finish screen. | ⚠️ 2026-10-11 settings: recorded, works at once, Esc cancels; onboarding not run | | |
| 15 | Hotkey conflict | See "Hotkey conflicts" below. | ⚠️ 2026-10-10 hotkey layer only (`hotkey::Hotkey::set`, real `RegisterHotKey`): `Win+E` and `Win+R` refused, a second holder of `Ctrl+Alt+K` refused, a refused change keeps the old one, freed once the holder is dropped, `Alt+Space` reserved. Not seen: the settings warning box, AutoHotkey, the startup case | | |
| 16 | Real feature download | Isolated instance without `MAGI_FAKE_EMBEDDER`; see "Downloads" below. | ⚠️ 2026-10-11 OCR (13 MB): Installed, Remove download empties models/. Not run: 135 MB model, network cut | | |
| 17 | Launch at login | See "Launch at login" below. | ⚠️ 2026-10-11 on and off (key written, then removed); 2026-10-10 on: `HKCU\…\Run` has `Magi` = `…\magi.exe --background` (written by the app). Not run: switching off, Task Manager Startup apps, sign out/in | | |
| 18 | Remove folder | Trash on a folder asks first; Cancel keeps it, Remove removes it (and the folders it absorbed). | ✅ 2026-10-11 (absorbed folders not run) | | |
| 19 | Unreadable list live | Settings › Index open; drop a broken PDF (e.g. `echo x > bad.pdf`) into a root: it appears without reopening; Retry all updates it. | ✅ 2026-10-11 | | |
| 20 | Light-mode see-through | Light theme, blurred background: every step of the slider changes the window, from "More solid" (opaque) to "More transparent". | ✅ 2026-10-11 (brightness falls monotonically, 245.7 to ~215) | | |
| 21 | Dark-mode switch contrast | Dark theme: an off switch is clearly visible on its box. | ✅ 2026-10-11 (track ratio ~1.7:1) | | |
| 22 | Spanish and English | `ui.language = es`, then `en`, from settings: every window relabels live; tray too. | ✅ es 2026-09-30 | ⚠️ 2026-10-13 es (follows the system); en not seen | |
| 23 | Startup failure | Isolated instance with a folder where the database should be (`mkdir $MAGI_DATA_DIR/magi.db`): "Magi couldn't start" with the error; Show log reveals `magi.log`; Quit and closing both exit. | ✅ 2026-10-05; Show log 2026-10-11 | | |
| 24 | Onboarding setup | `--onboarding`: drag a folder from Explorer/Finder onto the drop zone, it is added (a dropped file is not); every icon draws; ⓘ opens what a feature does; the button reads "Download {size} and start", or "Start" with every feature off; light and dark, English and Spanish: no name cut off, no scrolling in the features column. | ⚠️ 2026-10-11 all but dropping a folder | | |
| 25 | Detail pane | Settings › Appearance › Search window layout › List and preview (the drawing is framed in the accent), then reopen search: the list is on the left, the pane on the right. Arrow keys and the mouse move the pane with the selection, and the window keeps its height. A PDF shows a page chip and an image its thumbnail; a text file shows a small glyph beside its name, with no empty box. A match on the file name shows no snippet card. Open and Show in folder stay in view. The details line has type, date, pages and size. Clicking the folder and Show in folder reveal the file; Open opens it. Light and dark, English and Spanish: nothing is cut off. Off again: the old single list. | | | |
| 26 | Alt+Enter asks | With results on screen, Alt+Enter opens the Ask window with the search text as the question; with no results it does nothing. See "Ask Magi" below. Linux: also on KDE, whose desktop may take the combination first. | ⚠️ 2026-11-09 by hand: it opened with only folded weak results (fixed, `askable`); after the fix pressed in the List layout with results, opened. With only folded results after the fix: test platform only | ✅ 2026-11-09 CI: Alt+Enter on a search for "invoices" opened the Ask window with the query in the box | not run (GNOME or KDE) |
| 27 | Every Ask entry point | Footer button and Alt+Enter with the detail pane on and off; the pane's button with it on. Each opens the window; asking again from a new search keeps one window with the new contents. | ⚠️ 2026-11-09 by hand, before the chat rework, by the owner; after it only Alt+Enter in List (scripted). Footer button and the pane button after the rework clicked (scripted, real server) | not run | not run |
| 28 | Ask Magi full flow | Against the deployed server; see "Ask Magi" below. | ⚠️ 2026-11-09 scripted input, deployed server: every bullet below passed, the daily-limit turn included; three issues found, fixed, and seen fixed after the redeploy, see `docs/progress.md` | ⚠️ 2026-11-09 CI: sign-in against the deployed server passed (keychain write works on the runner); asking not reached, the script's click missed the new send arrow (script fixed, re-run pending) | not run |

## macOS session (M6 column + M7)

**On CI first:** the "macOS checks" workflow (`.github/workflows/macos-checks.yml`,
run by hand) checks M2, M3, M5, M6 (with a folder made unreadable, not the
real consent prompt), M10, M11, M14, M16 (a disk image ejected and
re-mounted), M20, M21 and rows 2, 9, 10, 11 on a hosted Mac, with a
pass/fail table in the run summary and screenshots. A hosted Mac has no
iCloud account, network share, battery or real blur, so M12, M13, M15 and
M19 and the real consent prompt (M4, M7) stay with a borrowed Mac.

For a borrowed Mac: one sitting, in this order. Use the `.dmg` from the
`release.yml` draft release (a signed bundle matters: macOS ties permission
grants to the app's signature, so a terminal-launched dev build gets its
prompts attributed to the terminal). Record each result as ✅ / ❌ / ⚠️ with
the date in `docs/progress.md`, plus a screenshot where it says so.

**Install and first run**

- M1. Open the `.dmg`, drag Magi to Applications, open it. Note any
  Gatekeeper warning (ad-hoc signed, not notarized: right-click › Open is
  expected).
- M2. ✅ 2026-11-06 CI. ✅ 2026-10-13 no Dock icon (Cmd-Tab and the menu-bar icon not seen;
  a menu-bar manager may have hidden it). No Dock icon and not in Cmd-Tab; the menu-bar icon is there.
  Screenshot the menu bar, light and dark (the icon is full colour; judge
  whether it looks out of place).
- M3. ✅ 2026-11-06 CI. Onboarding and Settings come to the front when opened from the menu
  bar, not behind the browser.

**Permissions (Files and Folders)**

- M4. Add `~/Documents`: the system prompt shows Magi's reason text
  (screenshot it). Click **Don't Allow**.
- M5. ✅ 2026-11-06 CI (folder made unreadable; real prompt not seen). Settings › Folders: Documents says which permission is missing and
  shows "Open System Settings"; the button lands on Privacy & Security ›
  Files and Folders (screenshot).
- M6. ✅ 2026-11-06 CI. Grant access there. Within ~60 s Documents starts indexing with no
  restart or rescan.
- M7. Add a folder outside the protected set that is denied (e.g. another
  user's folder, or `~/Library/Mail`): the guidance says Full Disk Access
  and that a restart may be needed; the button opens that page.
- M8. Run the onboarding permission check (`--onboarding`, or a fresh
  account): the denied folder is listed with the same guidance.
- M9. Switch the language to Spanish: the guidance relabels.

**Background behaviour**

- M10. ✅ 2026-11-06 CI. Force-quit Magi (Activity Monitor), relaunch: it starts, and
  `magi --toggle` (from the bundle: `/Applications/Magi.app/Contents/MacOS/magi --toggle`)
  opens search. This is the stale-socket fix.
- M11. ✅ 2026-11-06 CI. Shift+Cmd+Space opens and closes search (table row 1).
- M12. iCloud Drive with "Optimize Mac Storage" and a file evicted
  (cloud icon): after indexing it is still a cloud file (not downloaded)
  and is found by its name.
- M13. A folder on an SMB share: Settings says it is checked periodically,
  not watched; a file added on the share appears within a few minutes.
- M14. ✅ 2026-11-06 CI (after the bundle fix). A `.app` or `.photoslibrary` inside an indexed folder is one item,
  not its contents.
- M15. ⚠️ 2026-10-13 status read "En pausa: con batería"; timing and
  resume on AC not seen. On battery with "Pause on battery" on: indexing pauses within 30 s,
  resumes on AC; the status says "Paused: on battery".
- M16. ✅ 2026-11-06 CI (disk image). Unplug a USB drive that is a root: it shows "missing", its results
  go; replug: it resumes with no re-embedding.
- M17. Ten idle minutes with windows closed: memory (Activity Monitor ›
  Memory) back near the idle footprint (table row 11).
- M18. Type a search while a big folder indexes: results still come
  quickly (row 10's log line).

**Look and feel**

- M19. Search and settings over a busy wallpaper, light and dark: the
  blur is real (see-through), the slider changes it; with System Settings ›
  Accessibility › Display › Reduce transparency on, both go solid (row 13).
- M20. ✅ 2026-11-06 CI. Type `canción` with the dead-key accent; also search a file whose
  name has an accent (macOS may store it decomposed).
- M21. ✅ 2026-11-06 CI. Launch at login on: a "background item added" notice appears once;
  after logging out and in, Magi runs with no window.

Then the M6 table above, macOS column: rows 1–7, 9–11, 13–28 (8 is
Linux-only; use Cmd where the table says Ctrl).

## Ask Magi

Say plainly in `docs/progress.md` what was run by hand and what only ran in
CI or on the test platform. `MAGI_FAKE_ASK=1` runs the isolated instance
against an in-process fake server, for rows 26 and 27 only: row 28 is the real
server, with a real account and a real question.

- Signed out: opening the window shows sign-in, and search works as before.
- Sign in, or create an account, in the Ask window or in Settings › Account
  (signed out it shows the same form); either one opens the chat. The
  first-use disclosure shows above the box once, and not after the first
  question.
- The box holds the search text; the chips above it are up to 5 results with
  text, ticked; a photo with no text is not listed. Untick one: it is not
  cited or sent.
- Enter sends: your question on the right, Magi's answer on the left with its
  files as numbered chips; a chip opens its file. Count used goes up by one.
- Ask a follow-up: a second turn under the first, the box says "Ask a
  follow-up…".
- Turn the network off and send: one line on that turn and Try again; turn it
  on and retry.
- Settings › Account: signed in as, sign out (the open Ask window returns to
  sign-in), delete account (asks first; signing in again then fails).
- Light and dark, English and Spanish: nothing cut off.

**CI (macOS):** the "macOS checks" workflow presses Alt+Enter and, when the
`MAGI_ASK_TEST_EMAIL` and `MAGI_ASK_TEST_PASSWORD` secrets exist, signs in and
asks; the answer is graded from `13-ask-answer.png`. If a keychain prompt blocks
that run, use the fake store there (`docs/issues/ask-magi/plan.md`).

**Real Mac, once:** the keychain prompt and "Always Allow", which CI cannot
show. About ten minutes.

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
Settings › What's searched:

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
