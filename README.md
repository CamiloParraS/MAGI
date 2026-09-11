# magi

Local semantic file search. Indexes the contents of files in folders you
choose (text, code, PDFs, Office documents, images via OCR and visual
embeddings, QR codes) and lets you search by meaning, in English or Spanish,
entirely on-device — no cloud, no telemetry.

See [`SPEC.md`](SPEC.md) for the full product spec and implementation plan,
and [`AGENTS.md`](AGENTS.md) for the short pointer agents should read first.

## Status

Early bootstrap (M0). Not yet usable.

## Privacy

- Read-only on your files: magi never creates, modifies, moves, or deletes
  anything inside the folders you index.
- No network access except downloading model files you've consented to.
- No telemetry, no analytics, no crash reporting.

## Development

See [`SPEC.md` §4](SPEC.md#4-setup--what-you-need-before-writing-code) for
prerequisites, then:

```sh
just setup
just dev
```

`just check` runs the full local quality gate (fmt, clippy, tests, frontend
lint/typecheck/test).
