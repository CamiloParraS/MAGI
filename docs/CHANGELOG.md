# Changelog

All notable changes are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### 0.2.2

- **Ask Magi: files found by name now carry their text.** A result matched
  only by its file name used to be attached to a question as an empty chip;
  it now shows the start of the file's indexed text, and the best-matching
  parts are sent as for any other file.
- **Ask Magi:** the file picker lines names up whatever the icon (glyph or
  thumbnail), and the suggested questions stay out of the way while it is
  open.
- **Clearer empty states, a labelled Ask key and a calmer finish screen**
  during setup; the finish screen says when models are still downloading.
- The detail pane fades in the first time it appears.

### 0.2.1

- **Ask Magi can read more than snippets.** Each file you attach to a
  question shows how it will be sent (the best-matching parts, the whole
  file with its size, or, for photos, as a picture) and switches on click.
  "Add file" searches your index inside the window; you can also drop files
  onto it. Up to 3 pictures and 5 files per question; EXIF data is removed
  from pictures and cloud placeholders are never read. Answers can be
  selected and copied.
- **Ask Magi in onboarding:** a switch on the setup screen, off until you
  choose it. Turning it on adds a sign-in or create-account step before the
  finish screen. It can be skipped: search needs no account.
- **Sign-in redesign:** one screen, the same in the Ask window, onboarding
  and Settings (side by side there, so it fits the window).
- **Settings › Ask Magi:** a switch hides or shows the Ask Magi button and
  its shortcut in the search window; the button now sits beside the search
  box.
- **Ask window:** opens wider, the question box floats over the end of the
  chat, and an arrow scrolls back to the latest answer.
- **Empty folders state:** the whole drop zone is one button, in the same
  style as the rest of the app.
- **Fix:** questions with a picture no longer fail with a server error.

### 0.2.0 (test build)

- **Ask Magi (optional, needs a Magi account):** ask a question in plain
  words and get an answer that cites your files. Open it from the search
  window, the tray, or Alt+Enter. Nothing is sent until you sign in and send
  a question; the first time, a card says what is sent. Sign in, sign out
  and delete your account from Settings › Account. Signed out, Magi makes no
  connection other than model downloads (ADR-0014).
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
