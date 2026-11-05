# M7 plan: permissions and background-behavior hardening

**Status:** drafted 2026-10-11. Triage: `ready-for-agent` (no issue tracker is configured in this environment, so this file stands in for the issue; copy it into the tracker once `/setup-matt-pocock-skills` has been run). Progress is recorded in `docs/progress.md`; this file is the plan, not the log.

M6 is not signed off (the macOS and Linux columns of `docs/qa-checklist.md` are open), so CLAUDE.md's "one milestone at a time" gate is not met yet. This plan is ready to start the moment the owner decides to overlap the two; the macOS fixes below were made ahead of it because they cannot wait for a Mac.

## Problem Statement

Magi works on the developer's Windows machine, but on the OSes a user actually meets, it does not yet explain itself when the OS gets in the way, and on macOS several things are simply wrong:

- On macOS the first time a folder like Documents is added, the system asks for consent. If the user says no, the folder shows "No access" and nothing tells them how to fix it. Nothing says which permission is missing, and nothing takes them to the right Settings page. The same gap exists for Linux (watch limit reached, no global shortcut under Wayland) and Windows has nothing to explain.
- The consent prompt on macOS has no explanatory text, because the app has no `Info.plist` usage strings.
- On macOS, the app shows a Dock icon even though it is meant to live in the menu bar; the settings window can open behind other apps; a crash leaves a stale socket file that disables single-instance and `--toggle` for good; a folder on a network share (SMB, NFS) looks watched but never updates.
- Whether the background behavior (low priority, pause on battery, idle unload, memory on an 8 GB machine, search while indexing) holds up on every OS has only been measured on Windows.

## Solution

From the user's side, after M7:

- Adding or opening a folder that Magi cannot read shows a plain explanation of the missing permission for that OS, and, where the OS offers one, a button that opens the right settings page (macOS: Files and Folders, or Full Disk Access). Once the user grants access, the folder recovers on its own within a minute and indexes, with no restart or manual rescan.
- The same guidance is available during onboarding (the permission check) and in Settings next to each affected folder.
- Linux users whose watch limit is too low, or whose desktop does not allow a global shortcut, see what to do and how to make it permanent.
- macOS shows a consent prompt that says why Magi wants the folder, and the app is menu-bar only: no Dock icon, windows still come to the front.
- A per-OS checklist, with screenshots, proves each behavior; known caveats (macOS permission grants being tied to the app's code signature) are written down in `docs/permissions.md`.

## User Stories

1. As a macOS user adding my Documents folder, I want the consent prompt to say why Magi needs it, so that I can decide with confidence.
2. As a macOS user who clicked "Don't Allow", I want the folder to say which permission is missing, so that I know what went wrong.
3. As a macOS user with a denied folder, I want an "Open System Settings" button that lands on the right page, so that I do not hunt through Privacy & Security.
4. As a macOS user who just granted access, I want the folder to start indexing by itself within a minute, so that I do not have to restart or rescan.
5. As a macOS user whose folder needs Full Disk Access (for example the whole home folder or a protected location), I want the guidance to point at Full Disk Access instead, so that granting the wrong permission does not leave me stuck.
6. As a macOS user, I want no Dock icon for Magi, so that a background utility does not clutter my Dock and Cmd-Tab.
7. As a macOS user, I want Settings and the welcome window to come to the front when I open them from the menu bar, so that they do not appear hidden behind my browser.
8. As a macOS user who force-quit or crashed Magi, I want the next launch and `magi --toggle` to still work, so that a stale socket file does not silently break the shortcut flow.
9. As a macOS user with a folder on an SMB or NFS share, I want Magi to check it periodically and say so, so that I am not misled into thinking changes are picked up live.
10. As a macOS user with iCloud "Optimize Mac Storage", I want Magi to never download my evicted files, so that indexing does not fill my disk or my bandwidth.
11. As a macOS user, I want evicted iCloud files to remain findable by name, so that the index still helps me find them.
12. As a macOS user, I want package folders like `.app` and `.photoslibrary` treated as single items, so that Magi does not index the guts of an app or my photo library.
13. As a macOS user on battery power, I want indexing to pause (when I have chosen that), so that Magi does not drain my laptop.
14. As a Windows user with OneDrive "online-only" files, I want them to stay online-only after indexing and still be findable by name, so that Magi does not fill my disk.
15. As a Windows user with very deep folders, I want paths over 260 characters indexed and openable, so that nothing is silently missing.
16. As a Windows user, I want files another program has locked to be retried later without being counted as broken, so that my error list stays meaningful.
17. As a Linux user whose inotify watch limit is too low, I want the folder to say it is checked periodically, and be told the exact setting and how to keep it across reboots, so that I can fix it once.
18. As a Linux user under Wayland whose global shortcut fails to register, I want instructions to bind a desktop shortcut to `magi --toggle`, so that I still get the quick search.
19. As a Linux user on GNOME without tray support, I want everything to remain reachable without a tray icon, so that I am not locked out of settings.
20. As any user, I want an unplugged USB drive to show as "missing", its results hidden and its index kept, so that I do not lose work.
21. As any user, I want a replugged drive to resume with no re-embedding, so that unplugging costs nothing.
22. As any user on battery with pause enabled, I want indexing to pause within 30 seconds and resume on AC, so that the setting does what it says.
23. As any user, I want the app's memory to drop back to the idle footprint after ten quiet minutes, with the ML models unloaded, so that it is a polite background citizen.
24. As any user on an 8 GB machine indexing a big mixed folder (phone photos included) with a browser open, I want the machine to stay responsive and not swap, so that Magi never makes my computer unusable.
25. As any user, I want to type searches while a large index runs and still get quick results, so that indexing never blocks me.
26. As any user, I want the onboarding permission check to show every unreadable folder with guidance, so that problems are caught at setup rather than discovered later.
27. As any user, I want the guidance text in my chosen language (English or Spanish), so that it is understandable.
28. As any user, I want guidance for a folder to disappear when the problem is fixed, so that the screen does not nag.
29. As the maintainer, I want a written per-OS behavior and caveat document, so that support questions (especially about macOS permissions after an update) have an answer.
30. As the maintainer, I want the macOS permission-related behaviors recorded as "verified" or "unverified, no Mac available", so that I know what a first Mac tester should check.
31. As a developer running an unsigned dev build on macOS, I want to know that permission prompts may repeat after each rebuild and may be attributed to my terminal, so that I do not mistake it for a bug.
32. As a developer, I want the permission report to be a plain data call on the host, so that it is testable without a window.
33. As a developer, I want the guidance expressed as codes and parameters, never as prose from Rust, so that the UI translates it.

## Implementation Decisions

**Status of the spec's M7 deliverables (from reading the code on 2026-10-11)**

- Already built in earlier milestones: low-priority indexing threads on all three OSes, ONNX thread caps by power state, pause on battery, model idle unload, low-memory mode, launch-at-login toggle, Windows cloud-placeholder and file-lock handling, network drives polled on Windows, Linux watch-limit fallback to polling with a warning log, per-root health (`ok`, `permission_denied`, `missing`, `watch_failed`) with 30-second re-probe and auto-reconcile on recovery, opaque package bundles, macOS dataless detection.
- Built in this pass (macOS, see below): usage strings file, Accessory activation policy, window activation, stale-socket recovery, network-volume polling.
- Not built: the permission report, the guidance UI, the settings deep link, Linux Wayland guidance, `docs/permissions.md`, and every verification item.

**Permission report (core)**

- A new host operation returns the list of folders that need the user's attention. Each entry has the folder's id and path, the kind of problem (access denied, folder missing, watching not possible), and a stable guidance code with parameters. It is computed from the roots' current health, so it needs no new database state.
- It also pushes on the existing event channel whenever the set of problems changes, so open windows update live. The spec already names a `permissions` event; it carries the complete list, never a delta, like `features`.
- Guidance codes name the situation per OS (macOS needs Files and Folders access, macOS may need Full Disk Access, Linux watch limit with the current limit as a parameter, Linux hotkey unavailable under Wayland, Windows access denied with nothing to grant). Rust never produces prose (locale-neutrality rule); the desktop string table (en and es) renders each code.
- Whether macOS needs Files and Folders or Full Disk Access is decided by where the folder is. The protected set (Desktop, Documents, Downloads, iCloud Drive, removable and network volumes) maps to Files and Folders; anything else denied maps to Full Disk Access.
- The settings deep link per OS is a platform function in the platform layer, returning nothing where the OS has no such page. Its macOS values are the two `x-apple.systempreferences` URLs in SPEC.md §6.1. The UI opens it with GPUI's own URL opener. No business logic gets an OS branch.
- The Wayland case is not a root problem; it is the hotkey state the shell already tracks. The report's Linux entry for it is produced by the desktop app, not the core.

**Guidance UI (desktop)**

- Settings, Folders: a denied or missing folder row gains the explanation and, when the OS has one, the "Open System Settings" action. A folder with watching not possible shows the polling note plus the Linux limit advice. This closes the M6 carry-over "fix actions for missing / permission_denied roots".
- Onboarding: the existing setup permission check lists the same entries.
- Opening settings never blocks on a probe; the re-probe loop keeps running and the list updates from the pushed event.

**macOS (done in this pass, unverified on hardware)**

- `Info.plist` fragment with the five usage descriptions the spec lists, menu-bar-only mode, and a minimum OS version of 13. The M8 packaging tool merges it into the generated bundle plist; it owns identifier, executable and version.
- Runtime Accessory activation policy, applied after GPUI's launch (GPUI forces the regular policy first), for builds that are not bundled. The settings and onboarding windows explicitly activate the app, because an Accessory app is not brought forward by its window.
- Single-instance claim: a socket name that is taken but unanswered is a stale file and is replaced; otherwise the previous "run without single-instance" fallback stays.
- Network volumes (SMB, NFS, AFP, WebDAV) are detected by filesystem type and polled instead of watched, because the macOS file-event service does not report changes on them.

**Verification work (all OSes)**

- The spec's per-OS manual checklist (SPEC.md §7 M7) is run on a fresh user account or VM, results and screenshots recorded in `docs/progress.md` and `docs/permissions.md`. The 8 GB check and the idle-footprint check are expected to expose the known NFR-11 overshoot (1.76 GB against 1.5 GB with the visual-image feature on); the owner decides then whether to relax the number or lower the batch, not this plan.

## Testing Decisions

- **A good test here checks external behavior:** given roots in particular states on a particular OS, the report contains exactly the right entries and guidance codes; a window given such a report shows the right actions. It does not check how the report is computed or which function builds a string.
- **Seam 1 (proposed, primary): the host API.** The permission report is tested through `Host` against a real engine on temporary folders with the deterministic fake embedder (`MAGI_FAKE_EMBEDDER`), the way `tests/host.rs` and the reconcile recovery tests already work: make a folder unreadable or missing, assert the report and the pushed event, restore it, assert the entry disappears and the folder indexes. OS-specific guidance is chosen through pure functions in the platform layer (as `pmset_on_battery` and `macos_flags_dataless` are), so macOS and Linux guidance is unit-tested on Windows.
- **Seam 2 (proposed, secondary): the desktop's pure view-state functions**, the same style as the root-state tests in the settings module, plus the existing headless GPUI tests for the settings window. Strings are checked by the compiler (en must cover es).
- Fewer seams were not possible: the report is core behavior, the rendering is desktop behavior. Neither needs a new seam type.
- **Prior art:** root-health and recovery tests in the reconcile module; the platform module's pure decoders; the settings and onboarding headless tests; the stale-socket test added for macOS (compiled and run only on macOS, so it is unverified until CI runs it).
- **Not unit-testable, so manual:** a real consent prompt, a real deny and grant, a real iCloud eviction, a real Wayland session, a real network share. These are the verification checklist, not tests.

## Out of Scope

- Packaging, signing, notarization and the `.dmg` (M8). Without a signed, bundled build the macOS consent flow cannot be fully exercised; that is a M8 dependency, not a reason to delay the guidance work.
- Security-scoped bookmarks and the App Sandbox (the spec ships non-sandboxed).
- Making the macOS tray icon a template (monochrome) image: it depends on how the brand mark looks in the menu bar and needs eyes on a Mac.
- Replacing the shell-outs used for the macOS "reduce motion" and battery checks with native calls.
- Search-quality and memory budget changes (NFR-11), beyond recording the numbers.
- Intel macOS (best-effort per the spec's open question Q5).

## Further Notes

**macOS things worth knowing (the user asked for these)**

Fixed blind in this pass, with how it was checked:

| Item | What was wrong | Checked |
| --- | --- | --- |
| Dock icon | GPUI forces the regular activation policy at launch, so a Dock icon always showed; the spec asks for Accessory | New code type-checks against the real macOS target in an isolated crate; behavior unseen |
| Windows opening behind other apps | An Accessory app is not activated by opening a window | Same; the activation call is a no-op on Windows and Linux |
| Stale socket | The macOS socket is a file in `/tmp`; the CI screenshot log shows "Address already in use" on a fresh launch, so a killed process had left the file and single-instance was off | Logic compiles on Windows; the test for it runs only on macOS CI |
| Network volumes | No file events from SMB/NFS/AFP; the app believed they were watched | Pure type-name function tested everywhere; the `statfs` call type-checks for macOS |
| Usage strings | None existed, so the consent prompt would have been blank | File added; not testable without a bundle |

Known risks that were deliberately not changed (they need a Mac):

1. **Permission grants follow the code signature.** Every unsigned rebuild can re-prompt, and a binary started from a terminal is attributed to the terminal. A fresh Files-and-Folders grant takes effect immediately; a Full Disk Access grant may need the app restarted. `docs/permissions.md` must say so, and the "recovers within 60 s" check should be run with a signed bundle.
2. **The menu-bar icon is full colour.** macOS expects a monochrome template image that follows light and dark menu bars; the brand blue is readable on both but will not look native. Decide with the real icon on a real menu bar.
3. **Window blur (vibrancy)** is still undecided: the VM screenshots prove nothing (already recorded in `docs/progress.md`).
4. **Reduce-motion** reads a preference domain that newer macOS versions may refuse; the fallback is "motion on", which is harmless. `NSWorkspace` has the proper answer if it ever matters.
5. **Ventura (13)** shows evicted iCloud files as hidden `.name.icloud` stubs rather than dataless files; they are hidden, so they are skipped, and findable-by-name will not work for them there. Sonoma and later use the dataless flag the code checks.
6. **Launch at login** uses a user LaunchAgent; macOS 13+ shows a "background item added" notice the first time. Expected, but should be seen once.
7. **File names** typed with accents may be stored decomposed on some volumes; search should be checked once with Spanish accents on a Mac (dead keys already pass on Windows).
8. **The global shortcut** (Shift+Cmd+Space) has never been pressed on a real Mac.

**Order of work (when M7 starts):** the permission report and its host test; the settings guidance; the onboarding check; Linux guidance and `doctor` output; `docs/permissions.md` with per-OS tables; then the manual verification rounds, one OS at a time as machines allow.

**Update `docs/architecture.md`** when the report lands, since the host API and the event list are public contracts.
