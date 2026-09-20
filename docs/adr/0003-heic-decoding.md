# ADR-0003 — HEIC/HEIF decoding

- **Status:** Accepted for the Windows leg; the Linux and macOS CI legs are
  still owed, and so is the 48 MP budget (no valid fixture yet).
- **Date:** 2026-09-19
- **Milestone:** M4, Slice 1 (SPEC.md §7 M4 names this the first task of M4)

## Context

SPEC.md §7 M4 requires phone HEIC photos to be indexed on Windows, macOS and
Linux, and names two options:

- **Option A** — `libheif-rs` on all three OSes, dynamically linked, the
  native libs bundled with the app. This is what SPEC.md §3 currently records
  as the choice.
- **Option B** — native decoders per OS (macOS ImageIO, Windows WIC, libheif
  on Linux).

The spike found a third:

- **Option C** — `heic-rs` 0.1.1, a pure-Rust HEIC decoder (MIT OR
  Apache-2.0, `forbid(unsafe_code)`, one optional dependency). It did not
  exist when the spec was written: it was first published 2026-09-12.

Decision criteria, from SPEC.md §7 M4: builds on all three CI runners, bundle
size impact, decode time for a 12 MP and a 48 MP photo, and licence
obligations.

### Fixture inventory

Measured, not taken from the filename (`heic_rs::probe`, which reads the
container without decoding):

| Fixture                        | Pixels      | MP   | Bit depth | Chroma | Grid                |
| ------------------------------ | ----------- | ---- | --------- | ------ | ------------------- |
| `iphone_12mp_landscape.heic` ¹ | 4000 × 3000 | 12.0 | 8         | 4:2:0  | 6 × 8 tiles of 512² |
| `iphone_12mp_portrait.heic` ¹  | 3000 × 4000 | 12.0 | 8         | 4:2:0  | yes                 |
| `iphone_text_es.heic`          | 3000 × 4000 | 12.0 | 8         | 4:2:0  | yes                 |
| `iphone_qr.heic`               | 1834 × 1546 | 2.8  | 8         | 4:2:0  | yes                 |
| `shelf_christmas.heic`         | 4000 × 3000 | 12.0 | 8         | 4:2:0  | yes                 |

¹ Local-only, not committed (`fixtures/README.md`): these two photograph a
card naming the repo owner. `iphone_text_es.heic` and `shelf_christmas.heic`
cover the same 12 MP portrait and landscape cases in CI.

Every fixture is a **tile grid**, not a single coded image — a decoder that
does not compose grids decodes nothing useful here.

Note that despite their names these were shot on a Samsung Galaxy, not an
iPhone, which SPEC.md §7 M4 asks for; `fixtures/README.md` tracks that gap.
All of them have since been stripped of EXIF, XMP and Samsung's `sefd`
trailer, with the decoded pixels verified byte-identical before and after on
both backends.

**The 48 MP fixture does not exist yet.** The file that previously carried
the name `iphone_48mp_landscape.heic` is a 121 MB Netpbm P6 export
(5492 × 3672, 16-bit), not a HEIC; both decoders reject it at the container
(`NoFtypBox` / `BoxTooLarge`). SPEC.md §7 M4's "48 MP HEIC decodes in < 3 s
with an RSS delta < 400 MB" therefore **cannot be verified yet**. See
`fixtures/README.md`.

## Measurement

### Windows (reference machine, Windows 11, release build)

libheif 1.23.2 + libde265 1.1.1 from vcpkg (`x64-windows`, dynamic). Peak
working set polled every 50 ms, one process per decode, same method as
ADR-0005. **Single run per cell** — ADR-0005's two-run average still owed
before the decision is final.

| Fixture                           | Backend      | Decode | Peak WS      |
| --------------------------------- | ------------ | ------ | ------------ |
| `iphone_12mp_landscape` (12.0 MP) | `heic-rs`    | 135 ms | 45 MB        |
|                                   | `libheif-rs` | 233 ms | 56 MB        |
| `iphone_text_es` (12.0 MP)        | `heic-rs`    | 184 ms | **116 MB**   |
|                                   | `libheif-rs` | 235 ms | 45 MB        |
| `shelf_christmas` (12.0 MP)       | `heic-rs`    | 100 ms | 48 MB        |
|                                   | `libheif-rs` | 220 ms | 54 MB        |
| `iphone_qr` (2.8 MP)              | `heic-rs`    | 35 ms  | 17 MB        |
|                                   | `libheif-rs` | 56 ms  | 10 MB        |
| `iphone_12mp_portrait` (12.0 MP)  | `heic-rs`    | 231 ms | not captured |
|                                   | `libheif-rs` | 273 ms | not captured |

Both backends return identical dimensions on all five fixtures, including the
portrait ones (3000 × 4000 — the EXIF/`irot` rotation is applied by both, not
left to the caller). A 4 × 4 mean-RGB fingerprint agrees between them to
within a few units per channel, the expected difference between two chroma
upsamplers.

`heic-rs` is 1.3–2.3× faster. Its 116 MB outlier is the one number that
matters for the budget: it uses a rayon pool across grid tiles, so peak
memory scales with thread count, and the 48 MP case is 4× the pixels. That
has to be re-measured with `DecodeOptions::with_threads` before Option C can
be trusted against the 400 MB budget.

### Windows build notes

- `libheif-sys` locates libheif through the `vcpkg` crate, which defaults to
  the `x64-windows-static-md` triplet. With `vcpkg install libheif:x64-windows`
  (the dynamic triplet SPEC.md §4.3 prescribes, and the one LGPL requires),
  the build fails until **`VCPKGRS_DYNAMIC=1`** is set. Both CI and
  `docs`/`SPEC.md` §4.3 need that, and `libheif.dll`/`libde265.dll` must be on
  `PATH` at runtime.
- No LLVM/libclang is needed: `libheif-sys` 5.3.1 ships pre-generated
  bindings.
- `heic-rs` needs none of this: it is a normal Rust dependency.

### Linux

`libheif-rs` 3.0.0 requires **libheif ≥ 1.17.0**. Ubuntu 22.04 (jammy) ships
`libheif-dev 1.12.0-2build1` — below the minimum, so `apt install libheif-dev`
on the pinned CI runner (SPEC.md §3: `ubuntu-22.04`) cannot work. The
alternatives, both costly:

- `xtask build-libheif`: fetch and build a pinned libheif + libde265 from
  source (URL + SHA-256 pinned, same pattern as `fetch_pdfium`), and export
  `PKG_CONFIG_PATH`/`LD_LIBRARY_PATH` from the justfile and CI.
- Move the CI matrix to `ubuntu-24.04`, which raises the glibc floor for the
  AppImage and contradicts SPEC.md §1's Ubuntu 22.04 support line — a spec
  change, not just an ADR.

Option C has neither problem.

### macOS

Not yet run. `brew install libheif` for Option A; Option C needs nothing.

## Decision

**Option C: `heic-rs`.** `libheif-rs` stays the documented fallback, proven
to work by this spike.

The deciding argument is not the 1.3–2.3× speed. It is that Option C deletes
an entire class of work: no native library to install on three CI runners, no
LGPL notice, no dylib/DLL/`.so` bundling owed to M8, no `VCPKGRS_DYNAMIC`
incantation on Windows, and no Ubuntu 22.04 libheif-1.12 blocker — which
Option A has no cheap answer to. CLAUDE.md's "prefer pure-Rust crates" points
the same way.

Everything decoder-specific lives in `crates/magi-core/src/extract/heic.rs`
(`is_heic`, `decode_heic`), so reverting to `libheif-rs` is a one-file
rewrite. SPEC.md §3's HEIC row and §4.3's per-OS libheif install steps are
updated to match.

Two things this decision does **not** yet close, both tracked as M4 Slice 2
work:

1. **The 48 MP budget.** No valid 48 MP HEIC fixture exists
   (`fixtures/README.md`), so SPEC.md §7 M4's "< 3 s, RSS Δ < 400 MB" is
   unverified. The 12 MP numbers extrapolate to roughly 180–220 MB, but the
   116 MB outlier below says thread count matters more than pixel count.
2. **The three-OS CI build**, which SPEC.md §7 M4 requires. With Option C
   this is the existing `cargo test --workspace` on the existing matrix —
   there is nothing to install — but it still has to go green before M4 is
   signed off.

## Consequences

**If Option A had been chosen (`libheif-rs`, as SPEC.md §3 originally said):** an LGPL native
dependency, dynamically linked, with notices owed in
`THIRD_PARTY_LICENSES.md`; per-OS install steps in CI and in SPEC.md §4.3;
bundling work owed to M8 (dylibs inside the `.app` with `@rpath`, DLLs beside
the `.exe`, `.so`s in the AppImage); and the Ubuntu 22.04 problem above.
Mature: 780k downloads, a decade of libheif behind it.

**Option C, chosen:** all of the above disappears — no native
dependency, no LGPL notice, no bundling, no CI install step, no Ubuntu
version floor, and it decodes faster. The cost is maturity: 0.1.1, published
seven days ago, 252 downloads, one author. Mitigations: the decoder stays
behind `crates/magi-core/src/extract/heic.rs` so swapping back is a
single-file change; this spike has already proven `libheif-rs` works as that
fallback; and the fixture goldens catch a regression. SPEC.md §3's HEIC row is
updated accordingly.

`heic-rs` also has **no embedded-thumbnail API**: it decodes the primary item
only, with no way to select the `thmb` item. SPEC.md §7 M4's "use the
embedded HEIC thumbnail for the UI thumbnail when present, to avoid a full
decode" therefore has no cheap path. The measured cost of not having it is
100–230 ms per 12 MP photo, the same full decode OCR and embedding already
need, so Slice 2 builds thumbnails from that one decode rather than a second
pass. Revisit if the 48 MP numbers make it hurt.

**If Option B had been chosen (native per-OS):** Windows machines without the HEVC extension
cannot decode HEIC at all, which is SPEC.md §9 Q8 — it needs a human answer
before shipping the "HEIC not supported on this PC" message.
