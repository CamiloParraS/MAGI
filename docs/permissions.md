# Permissions and OS behavior

What Magi does when the OS gets in the way, per OS, and what the user is told (SPEC.md §6, M7). The codes are `dto::Guidance`; the words live in the desktop string table (en, es).

A folder is re-probed every 30 seconds while it is unreadable or missing; when it comes back it is reconciled and indexed with no restart (SPEC.md §6.1 "Recovery").

**Verification status:** nothing on this page has been checked on a real Mac or a Linux desktop yet. Rows marked "unverified" are what the first tester on that OS should check, and each check is recorded in `docs/progress.md`.

## Guidance codes

| Code | When | What the user sees | Action |
| --- | --- | --- | --- |
| `missing` | Any OS: the folder is gone (moved, deleted, drive unplugged) | "Folder not found. Reconnect the drive; the index is kept." | Try again (rescan) |
| `mac_files_and_folders` | macOS: denied inside Desktop, Documents, Downloads, iCloud Drive (`~/Library/Mobile Documents`) or `/Volumes` | Allow Magi in Privacy & Security › Files and Folders | Open System Settings |
| `mac_full_disk_access` | macOS: denied anywhere else | Allow Magi in Privacy & Security › Full Disk Access, then restart Magi | Open System Settings |
| `access_denied` | Windows, Linux: this user account cannot read the folder | "Magi isn't allowed to read this folder." | Try again |
| `watch_limit { limit }` | Linux: out of inotify watches, so the folder is polled | The current limit and the setting to raise | none (see below) |
| `polled` | Windows, macOS: a network drive, polled instead of watched | "Can't watch for changes here; checking now and then instead" | none |

## macOS

- **Consent prompts.** Desktop, Documents, Downloads, iCloud Drive, removable and network volumes prompt the first time Magi reads them. The prompt's text comes from `apps/desktop/Info.plist` (merged into the bundle by the M8 packager). Unverified.
- **Settings links.** `x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders` and `…?Privacy_AllFiles` (SPEC.md §6.1). Unverified on 13, 14 and 15:

  | macOS | Files and Folders link | Full Disk Access link |
  | --- | --- | --- |
  | 13 Ventura | ☐ | ☐ |
  | 14 Sonoma | ☐ | ☐ |
  | 15 Sequoia | ☐ | ☐ |

- **Grants follow the code signature.** An unsigned or ad-hoc signed dev build may prompt again after every rebuild, and a binary started from a terminal is attributed to the terminal app (the grant shows up under Terminal or iTerm, not Magi). Test the deny-then-grant flow with a signed bundle (M8).
- **Full Disk Access** usually takes effect only after the app restarts, hence the "then restart Magi" in its guidance. A Files and Folders grant takes effect at once and the 30 s re-probe picks it up.
- **Network volumes** (smbfs, nfs, afpfs, webdav) get no file events, so they are polled every 15 minutes (`polled`).
- **iCloud "Optimize Mac Storage".** Evicted files are dataless (`SF_DATALESS`); their content is never read, and they stay findable by name. On Ventura evicted files can show up as hidden `.name.icloud` stubs instead; those are skipped, so they are not findable by name there.
- **Menu-bar only.** No Dock icon (`LSUIElement`, and the Accessory activation policy at runtime for unbundled builds).
- **Launch at login** is a user LaunchAgent; macOS 13+ shows a "background item added" notice the first time. Expected.

## Windows

- No consent prompts. A root the account cannot read is `access_denied`; unreadable subfolders are skipped with a debug log.
- OneDrive online-only files are never read (recall attributes) and stay online-only; they are findable by name.
- Network and mapped drives are polled (`polled`).
- Files locked by another program are retried later and do not count as errors on the first three tries.

## Linux

- Unreadable root → `access_denied`; unreadable subfolders are skipped with a log.
- **inotify watch limit.** One watch per folder. When the kernel runs out, the root falls back to polling every 15 minutes and shows `watch_limit` with the current limit (`magi-cli doctor` prints it too). To raise it now, and to keep it across reboots:

  ```sh
  sudo sysctl fs.inotify.max_user_watches=524288
  echo fs.inotify.max_user_watches=524288 | sudo tee /etc/sysctl.d/90-magi-inotify.conf
  ```

  Then restart Magi so it watches the folder again. Unverified.
- **Wayland.** Global shortcuts may not work. When `XDG_SESSION_TYPE=wayland` and the shortcut fails to register, Settings › General says to add a desktop keyboard shortcut that runs `magi --toggle`. Unverified.
- **No tray** (GNOME without the AppIndicator extension): launching Magi again, the shortcut or `magi --toggle` opens search, and its gear opens Settings.
