# ADR-0010: Optional search features, locale-neutral core, window transparency

- **Status:** Accepted; to be implemented in M6
- **Milestone:** M6

## Context

Through M5 the engine assumed all three models were downloaded: e5 (~135 MB),
OCR (~13 MB) and SigLIP 2 (~532 MB), ~680 MB in total. Asking a user to accept
that much download before the app does anything hurts onboarding, and the
engine already works with no models at all: BM25 and filename search (M2).
M6 must also ship a complete Spanish translation, and the search window should
look translucent.

File types (`indexing.file_types`) are already configurable in the core, and
turning a type off skips its content extraction. They do not line up one to one
with models: images use both OCR and SigLIP, and all text-like kinds use e5.

## Decision

**Users choose search features, not file types or models.** Each feature is
backed by exactly one downloadable asset, and the word "model" never appears in
the UI.

| Feature (`Feature`) | Asset  | English                       | Spanish                          |
| ------------------- | ------ | ----------------------------- | -------------------------------- |
| `meaning`           | e5     | Search by meaning             | Buscar por significado           |
| `image_text`        | OCR    | Read text in images           | Leer texto en imágenes           |
| `image_visual`      | SigLIP | Find images by what they show | Buscar imágenes por su contenido |

- **Every feature is optional.** With none installed, search is BM25 plus
  filename; the search that would use a missing feature is simply skipped.
- **Desired state is separate from availability.** `enabled` (config) is what
  the user wants; `install` is what is on disk.
  `FeatureStatus { feature, enabled, install, backfill: Option<{done, total}> }`
  with `install = NotInstalled | Downloading{bytes, total} | Installed{size_bytes} | Failed{code}`.
- **Download lifecycle:** `NotInstalled → Downloading → Installed | NotInstalled
(cancel) | Failed{code}`. Each file is written to `<name>.tmp`, verified by
  SHA-256, then renamed; a partial file is never reported as `Installed`. No
  resume. `code` is one of `DownloadNetworkError`, `ChecksumMismatch`,
  `DiskFull`, `PermissionDenied`. Cancelling is not a failure.
- **Enabling a feature backfills** only files missing its output: e5 and
  SigLIP embed the chunks or images already stored, while OCR re-extracts images.
  No other file is re-indexed. Backfill runs at normal indexing priority.
- **Disabling a feature stops using it; it never deletes derived data.** Stored
  vectors stay on disk, unused (a query cannot be embedded without the asset).
  OCR text already indexed stays searchable through FTS. Re-enabling needs no
  re-indexing, apart from files that changed while the feature was off.
- **"Remove download"** is a separate, explicit action that deletes only the
  asset files. A per-feature "Remove indexed data" action is deferred; global
  `clear_index` covers it for now.
- **The core owns feature state and progress.** `engine://features` always
  carries the complete `FeatureStatus[]`, never a delta, so the tray, settings
  and search window need no reconciliation logic. It replaces `models_status`,
  `download_models` and `engine://models`.
- **Onboarding defaults:** `meaning` and `image_text` pre-selected (~148 MB);
  `image_visual` offered unselected with its size.
- **CLI parity:** `magi-cli features list | enable <f> | disable <f>
[--delete-download]`; `doctor` reports installed features. Offline asset
  import stays CLI-only.

**The core is locale-neutral.** Anything Rust sends to the UI that a user reads
(errors, permission guidance, failure codes) is a stable code plus parameters,
exported as `ts-rs` enums; the frontend localizes it. Localized text is never a
machine-readable contract. The tray, built in Rust, keeps its own small string
table. `ui.language = "system" | "en" | "es"`; `system` resolves any `es-*`
locale to Spanish, everything else to English. Changing the language changes
presentation only, never indexed or search data. The frontend uses a typed
dictionary (the `es` dictionary must satisfy the `en` type) and `Intl` for
plurals, dates and numbers; no i18n library for two languages.

**Search-window transparency uses native effects** through the `window-vibrancy`
crate (Rust bindings over OS APIs, no bundled C/C++): Mica on Windows 11,
Acrylic on Windows 10, vibrancy on macOS, solid on Linux or on any failure.
`ui.transparency_mode = match_system | always | never` resolves as: `never` →
solid; `always` → native effect if the OS can provide it; `match_system` →
follows the OS reduce-transparency preference. `ui.transparency_intensity`
(0.40–0.95) sets the alpha of the tint layer over the effect and is independent
of the mode; the slider is disabled whenever the effective state is solid.
Settings and onboarding windows stay solid.

## Consequences

- The core must treat each embedder and the OCR engine as optional: the
  pipeline skips a missing feature's work instead of failing the file, and
  search skips a missing feature's vector query.
- A file can be `indexed` without some feature's output, so backfill needs a
  cheap query for "files missing feature X's output".
- The search window shows a hint only when fewer than 3 results come back and a
  feature that could help is off. A hint driven by image-related query words is
  deferred (fragile in two languages).
- NFR-7 now describes the full download; the default download is ~148 MB.
- Launch at login is offered in onboarding and checked by default (§9 Q7).

## Implementation notes (Plan 1)

- `FeatureStatus` also carries `download_size`, so sizes can be shown before
  consent (FR-10).
- `Failed.code` adds `WriteFailed` for local I/O failures other than disk full
  or permission denied.
- Partial files are kept after a cancel, and a later download resumes them
  (existing M3 behavior). The state still returns to `NotInstalled`, and
  nothing partial is ever `Installed`: installed means every manifest file is
  at its final name with the manifest size.
- Backfill re-processes the affected files through the normal pipeline instead
  of embedding stored chunks. This keeps one code path, and it rebuilds chunks
  at the real tokenizer's boundaries (chunking without e5 uses an approximate
  token count).
- Features take effect when the engine starts. The host restarts the engine
  after `set_enabled` or a finished download (Plan 2).

## Implementation notes (Plan 2)

`host::Host` restarts the engine when any feature's `(enabled, installed)`
pair changes — the same `engine_inputs` comparison `host::supervise` makes
before and after each engine start, so a feature flip that lands mid-start is
never lost. Progress (`Install::Downloading` byte counts) and backfill
progress are excluded from that comparison on purpose: they change
constantly and never change what the engine loads.
