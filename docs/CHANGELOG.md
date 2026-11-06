# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### 0.1.2 (test build)

- **Detail pane (optional):** Settings › Appearance › Search window layout ›
  List and preview shows the selected result beside the list, with its page
  or thumbnail, why it matched, and Open / Show in folder.
- **Copy a result as a file** (paste it into a chat or an email) on Windows,
  macOS and Linux; a setting keeps the old copy-the-path behaviour.
- **Search window:** calmer look, one reason per result in the footer,
  match-kind chips, readable dark glass; the footer's unreadable-files note
  opens Settings. Screen-reader labels throughout.
- **Settings:** pause and rescan from Index; folders and file types read as
  one list; text fields show "Saved"; a button back to search.
- **Onboarding:** "Start with your computer" is a switch, as in Settings;
  clearer setup footer.
- **Search quality:** a strong image match now reaches the top 10 more
  often; a result is tagged "File name" only when the words matched its name.
- macOS: an app or photo library inside an indexed folder is found by its
  name, instead of waiting in the queue and then showing as unreadable.
- `THIRD_PARTY_LICENSES.md` lists every bundled library and model licence.

### 0.1.1 (Linux fixes)

- The app no longer crashes at startup: the tray icon talks to the desktop
  over D-Bus (StatusNotifierItem) instead of GTK. A left click on it now
  opens search; the menu is on the right click.
- On X11 the search bar takes the keyboard (it never did before).
- The `.deb` no longer depends on GTK, AppIndicator or libxdo.

### 0.1.0 (first test build)

First test build: local hybrid search (keywords, meaning, text in images,
image content) over folders you choose, with a menu-bar/tray app, a global
shortcut, onboarding, settings, and per-folder permission guidance. Unsigned
installers for Windows (NSIS), macOS (`.dmg`, Apple Silicon) and Linux
(`.deb`, AppImage).
