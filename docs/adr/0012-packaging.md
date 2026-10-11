# ADR-0012: Packaging with cargo-packager, native libraries loaded at runtime

- **Status:** Accepted (2026-10-11)
- **Milestone:** M8
- **Answers:** SPEC.md §9 Q9 (packaging tool) and M8's "static linking vs. dynamic loading for `ort`".

## Context

M8 needs an NSIS installer (Windows x64), a `.dmg` (macOS arm64), and an AppImage plus a `.deb` (Linux x64), built by CI on a `v*` tag. Each must carry PDFium and ONNX Runtime and find them at runtime. The owner wanted test builds early, so testers can check the M6/M7 macOS and Linux items before the rest of M8 is done.

Facts checked on 2026-10-11 against the cargo-packager 0.11.8 source and docs:

- It builds `.app`, `.dmg`, NSIS, WiX, `.deb`, AppImage and pacman packages from one config (`packager.json`, camelCase keys).
- It merges a user `Info.plist` (`macos.infoPlistPath`) into the plist it generates, and the user's keys win. `macos.backgroundApp` sets `LSUIElement`.
- It copies `resources` next to the executable (NSIS, WiX), to `Contents/Resources` (`.app`), to `/usr/lib/<exe>` (`.deb`) and to `$APPDIR/usr/lib/<exe>` (AppImage). These locations come from its `resource-resolver` crate.
- It signs on macOS only when `signingIdentity` is set. It has no ad-hoc fallback.
- With no Cargo package next to the config file, the config must set `name` and `binariesDir`.

## Options

1. **cargo-packager.** One config file and one CLI covers every format M8 lists. It is maintained by CrabNebula, the authors of Tauri's bundler.
2. **Per-OS scripts, as Zed does.** Full control, but it means writing three packaging pipelines (NSIS script, bundle and `hdiutil` steps, AppImage and `.deb` tooling) and keeping them up to date.

## Decision

- **cargo-packager 0.11.8**, configured in `packager.json` at the repo root, and pinned in `release.yml`.
- **macOS signing for test builds:** the workflow builds the `.app`, ad-hoc signs it with `codesign --sign -` and makes the `.dmg` with `hdiutil`. A signed and notarized build replaces this once a Developer ID exists.
- **ONNX Runtime and PDFium stay dynamically loaded.** `ort` keeps `load-dynamic`, and PDFium stays loaded through `pdfium-render`, as in development. `cargo xtask stage-libs` copies this OS's two libraries into `target/bundle-libs/`, which `packager.json` ships as resources. At runtime `paths::installed_library` looks in the four places above, after the `MAGI_*_PATH` overrides and before the dev `vendor/` folder. Static linking would mean building ONNX Runtime from source on three OSes for no gain the user would notice, and PDFium is only published as a shared library.

## Consequences

- No new Rust dependencies. cargo-packager is a CLI used only in CI and on dev machines.
- Release builds are unsigned (Windows) or ad-hoc signed (macOS). Users must click through SmartScreen or Gatekeeper (README "Install"). On macOS, privacy grants are tied to the code signature, so each new test build may ask again (`docs/permissions.md`).
- `packager.json` repeats the version. `release.yml` fails a tag that does not match it.
- The `.deb`'s `depends` list is written by hand and is unverified on a clean machine (M8 verification). The AppImage bundles its libraries through linuxdeploy.
