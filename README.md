# MAGI

Local semantic file search. Indexes the contents of files in folders you
choose (text, code, PDFs, Office documents, images via OCR and visual
embeddings, QR codes) and lets you search by meaning


## Install

Download the file for your system from the
[Releases page](https://github.com/CamiloParraS/MAGI/releases). Test builds are
not signed yet, so each OS warns the first time you open Magi.

- **Windows (x64):** run `magi_<version>_x64-setup.exe`. If SmartScreen says
  "Windows protected your PC", click **More info → Run anyway**.
- **macOS (Apple Silicon, macOS 13+):** open `Magi_<version>_aarch64.dmg` and
  drag Magi to Applications. Open it once; when macOS says it can't verify the
  developer, go to **System Settings → Privacy & Security** and click
  **Open Anyway**. Magi lives in the menu bar (no Dock icon). Each new test
  build may ask for folder access again (`docs/permissions.md`).
- **Linux (x64):** either install the `.deb` (`sudo apt install ./<file>.deb`),
  or make the `.AppImage` executable (`chmod +x <file>.AppImage`) and run it.
  The tray icon needs AppIndicator support (GNOME: the AppIndicator
  extension); without it, open Magi again or bind `magi --toggle` to a key.

Check a download against `SHA256SUMS` on the same release.

## Privacy

Magi reads only the folders you add, and never changes or moves anything in
them. The index stays on your computer, and nothing leaves it unless you ask it
to. The only network access is downloading the search models you choose in
setup, from the addresses listed in `models/manifest.toml`, and, if you sign in
to a Magi account, signing in and sending questions to Ask Magi. Signed out,
Magi makes no other connection. There is no telemetry, analytics or crash
reporting.

Ask Magi is optional. Each question sends its text, the passages of the
files you leave ticked that best match it (up to 8,000 characters in all) and
those files' names, never a whole file or full paths. They pass
through Magi's server to OpenRouter and an AI host that doesn't keep them.
Magi's server saves none of it; it stores only your account and a daily count
of questions.

## Uninstall and remove data

1. Turn off **Settings › General › Start with your computer**, then quit Magi
   from its tray or menu-bar icon.
2. Uninstall: Windows, **Settings › Apps**; macOS, move Magi from Applications
   to the Trash; Linux, `sudo apt remove magi` or delete the AppImage.
3. Delete Magi's data (index, downloaded models, logs) and settings:

| OS | Data | Settings |
| --- | --- | --- |
| Windows | `%APPDATA%\magi\magi\data` | `%APPDATA%\magi\magi\config` |
| macOS | `~/Library/Application Support/dev.magi.magi` | same folder |
| Linux | `~/.local/share/magi` | `~/.config/magi` |
