# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
