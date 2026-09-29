# ADR-0011: GPUI desktop shell

- **Status:** Accepted (2026-09-29); implemented in M6 Plans 3–5
- **Milestone:** M6
- **Supersedes:** the desktop-shell, frontend and TS-bindings parts of ADR-0001. ADR-0010's window-transparency section is amended below.

## Context

M6 Plans 1–2 hosted `magi-core` in Tauri 2 with a React + TypeScript webview
(branch `feat/User_Interface`). No UI had been built yet beyond a throwaway
visual prototype (variant A "Pane" chosen; screenshots in
`docs/screenshots/m6-variant-a/`). Everything the UI needs already sits behind
one Rust seam, `host::Host`, so the shell is the only thing that changes.

Reasons to move:

- **Footprint.** A webview adds its own processes (WebView2 on Windows) to the
  idle RSS that NFR-1 caps at 150 MB, and to the time from hotkey to a visible
  search window (< 150 ms, M6).
- **One language.** Dropping React/TS/Vite/pnpm/ts-rs removes a whole
  toolchain, a serialized IPC layer and its generated bindings.
- A deliberate preference for a native Rust UI over a web stack.

Facts checked on 2026-09-28 (sources: zed-industries/zed `main`, crates.io,
longbridge/gpui-component):

- Upstream `gpui` is pre-1.0 with no semver policy; its crates.io release
  stopped at 0.2.2 (Oct 2025) and current code is `publish = false`.
- `gpui-component` 0.7 (Apache-2.0, on crates.io, used in Longbridge Pro)
  re-exports a pinned GPUI snapshot (`gpui-pre`) and provides text input
  with IME, virtualized lists, forms and theming.
- GPUI has frameless and pop-up windows, per-display placement, Mica/MicaAlt/
  Blurred/Transparent backgrounds (**no Acrylic**), folder picker, open/reveal
  with the OS, AccessKit screen-reader support and a headless test platform.
- GPUI has **no** tray icon, global hotkey, single-instance or autostart.
- Rendering needs Direct3D 11 (Windows), Metal (macOS) or Vulkan 1.3/GL via
  wgpu (Linux). A software fallback on GPU-less machines is unverified.

## Options

1. **Keep Tauri + React.** Mature and already wired up; keeps the web stack
   and the webview's footprint.
2. **Upstream GPUI as a git dependency pinned to a Zed commit.** Latest
   code, but we would write our own text input and list, and track Zed's
   churn directly.
3. **`gpui-component` from crates.io** (chosen). Ready-made widgets on a
   pinned GPUI; we move at Longbridge's release pace.
4. **Community forks** (`gpui-ce`, `adabraka-gpui`): extra features, smaller
   maintenance base.

No throwaway spike or Tauri baseline was run: the owner committed to GPUI, so
a spike's answer could not change the decision. The risky integrations are
proven instead in the first, test-first slice of M6 Plan 3 (a walking
skeleton), and SPEC.md is amended if that slice finds something.

## Decision

**The desktop app is a single Rust binary using GPUI through `gpui-component`.**
All windows (search, settings, onboarding) are GPUI; there is no webview.

- **Host API instead of IPC.** The UI calls `host::Host` directly. SPEC.md
  §5.7 keeps the same commands, events and error codes as plain Rust calls
  and channels. `ts-rs`, `just bindings` and the generated TS bindings are
  removed. `Host` calls stay blocking and run on GPUI's background executor;
  async exists only in the desktop crate.
- **Shell integration:**
  - tray: `tray-icon`, running on GPUI's main thread; fallback `gpui-tray` if the two event loops conflict
  - global hotkey: `global-hotkey`
  - single-instance and `--toggle` forwarding: a local socket (named pipe or Unix domain socket) via `interprocess`, never loopback TCP (NFR-9)
  - launch at login: `auto-launch`
  - folder picker and open/reveal: GPUI's `prompt_for_paths`, `open_with_system` and `reveal_path`
- **Security.** Webview capabilities, CSP and the asset-protocol scope go
  away with the webview: there is no longer untrusted content running script
  in a privileged context. The one rule kept: the UI opens and reveals files
  only by `file_id` through `Host`, never by a path it builds.
- **Window background** (amends ADR-0010):
  - Windows 11: Mica
  - Windows 10: **solid**
  - macOS: Blurred, if it proves to be real vibrancy, otherwise solid
  - Linux: solid

  `transparency_intensity` stays only if it visibly changes something on a GPUI backdrop; otherwise it is removed from config and settings.
  Not taken: Acrylic on Windows 10 by calling `window-vibrancy` on GPUI's raw window handle. This is the route back if Windows 10 translucency becomes a requirement.
- **Localization.** ICU4X (`icu`, `compiled_data`) for plurals, dates and
  numbers. One Rust string table (a struct per language) shared by the UI
  and the tray, so `es` covering `en` is checked by the compiler. Trim data
  with `icu4x-datagen` only if the measured binary size matters.
- **Tests.** UI logic (highlight ranges, keyboard navigation, settings
  validation, transparency and language resolution) lives in plain Rust
  functions tested with `cargo test`; `#[gpui::test]` covers a few view
  smoke tests. Vitest, ESLint and `pnpm` leave `just check`.
- **Hardware.** The reference machine needs D3D11 / Metal / Vulkan 1.3 or GL
  capable integrated graphics, for UI rendering only (inference stays CPU).

## Consequences

- Node.js, pnpm and the WebView2/WebKitGTK prerequisites leave SPEC.md §4.
  The workspace member `apps/desktop/src-tauri` becomes `apps/desktop`.
- Dependency on a pre-1.0 UI framework: GPUI upgrades arrive only when
  `gpui-component` bumps, and may break our code. If `gpui-component`
  stalls, fall back to option 2 (git dependency on a Zed commit).
- Tray, hotkey, single-instance and autostart are our own integration code
  instead of maintained Tauri plugins.
- No bundler: `tauri-action` and Tauri's NSIS/dmg/AppImage bundling are gone.
  The packaging tool is SPEC.md §9 open question Q9, blocking M8.
- M7/M8 verification on clean VMs must confirm GPUI renders there. If
  GPU-less VMs cannot render, the GPU requirement is documented and those
  checks move to real hardware or GPU passthrough.
- Only Windows is available for testing now. CI builds the app and runs
  its headless tests on all three OSes; macOS and Linux behaviour (tray,
  vibrancy, Wayland) is an explicit M6 risk until verified there.
- The Tauri implementation is preserved on `feat/User_Interface` and the
  pre-change spec in `docs/SPEC-v1.1-tauri.md`.
