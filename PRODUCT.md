# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop-native (Windows 10/11, macOS 13+ Apple Silicon, Linux x64). Not web: a Rust GPUI app on `gpui-component` with no webview (ADR-0011). Per-OS window materials (Windows 11 blurred, Windows 10 solid), tray/menu-bar presence, global hotkey.

## Users

Knowledge workers and students with thousands of badly named files (screenshots, downloads, PDFs, Office docs, photos) and a vague memory of the content, not the filename. They hit a global hotkey, type what they remember, open or reveal the file, and leave. Work in English and Spanish; both UI languages are first-class. Typically on modest hardware (reference machine: 8 GB RAM laptop).

## Product Purpose

Find a file by what it contains or depicts, not what it is called. Magi indexes folders the user chooses (text, code, PDF, Office, images via OCR, visual embeddings and QR) and serves hybrid keyword and vector search with ranked results and snippets. Success: the right file is one hotkey and a few keystrokes away, and the app is otherwise invisible.

## Positioning

Private semantic search: meaning-based retrieval over text, OCR'd text, images and QR codes, entirely on-device. Read-only on user folders, no telemetry, no analytics, no crash reporting; the only network use is the model downloads the user consents to. OS search is filename and keyword only; cloud assistants are not private.

## Operating Context

Lives in the tray / menu bar. Summoned by hotkey into a compact search window; also has settings and first-run onboarding (folder selection, model download consent). Indexing runs in the background, lazily loads models and unloads them when idle, and must stay light. Opens or reveals files only by `file_id` through `Host`.

## Capabilities and Constraints

- Hybrid search: FTS5 BM25 + multilingual-e5-small text vectors + SigLIP 2 image vectors, fused by RRF with filename/recency boosts.
- Incremental indexing with file-system watching; never writes to user-selected folders; never triggers cloud-placeholder downloads.
- UI is thin views over `Host`; UI logic lives in plain, testable functions. Localization via ICU4X (en, es).
- Project codename `magi` is a placeholder pending a final name (SPEC Open Questions); the product is currently shipped as "Magi".
- Test builds are unsigned.

## Brand Commitments

Name "Magi". Brand logo exploration exists at `docs/brand/PROTOTYPE-logos.html` (a prototype, not yet confirmed binding). No other binding visual commitments recorded.

## Evidence on Hand

Shipped releases (0.1.1) on GitHub; screenshots under `docs/screenshots/` and per-OS screenshot folders; design mockups in `mockup/` and `docs/symbolical_designs/`; eval queries in `eval/queries.jsonl`; benchmarks in `docs/benchmarks.md`. No testimonials, user counts or customer names exist; do not invent them.

## Product Principles

1. Privacy is the product: nothing leaves the machine, nothing in the user's folders is touched, and the UI says so plainly.
2. Recall in seconds: the path from hotkey to opened file is the primary flow; everything else stays out of its way.
3. Search by memory, not by name: results explain why they matched (snippet, match type) so users trust fuzzy results.
4. Light on the machine: perform on an 8 GB laptop; background work yields to the user.
5. Bilingual by default: English and Spanish are equal citizens in copy and retrieval.

## Accessibility & Inclusion

No formal standard recorded. Keyboard-first operation is a product requirement for the search window; English and Spanish localization are required.
