---
name: Magi
description: A quiet, native pane for finding files by memory; system type, one cool accent, tonal layers instead of chrome.
colors:
  accent-dark: "#4cc2ff"
  accent-light: "#0a64d8"
  ink-dark: "#f3f3f5"
  ink-light: "#1b1b1f"
  mute-dark: "#b4b6bd"
  mute-light: "#5a5a5e"
  panel-dark: "#202024"
  panel-light: "#f6f6f8"
  window-dark: "#202020"
  window-light: "#f5f5f7"
  card-dark: "#2b2b2b"
  card-light: "#ffffff"
  sidebar-dark: "#191919"
  sidebar-light: "#ececec"
  ok-dark: "#6ccb5f"
  ok-light: "#0f7b0f"
  warn-dark: "#fcb452"
  warn-light: "#9d5d00"
  err-dark: "#ff99a4"
  err-light: "#c42b1c"
  match-mark: "#ffd600"
  kind-pdf: "#c4314b"
  kind-word: "#185abd"
  kind-excel: "#1e7145"
  kind-powerpoint: "#c43e1c"
  kind-code: "#5c2d91"
  kind-image: "#0f766e"
  kind-text: "#6b7280"
typography:
  heading:
    fontFamily: "system UI (platform default)"
    fontSize: "26px"
    fontWeight: 600
  body:
    fontFamily: "system UI (platform default)"
    fontSize: "14px"
  snippet:
    fontFamily: "system UI (platform default)"
    fontSize: "12.5px"
    lineHeight: 1.45
  label:
    fontFamily: "system UI (platform default)"
    fontSize: "12px"
    fontWeight: 600
rounded:
  glyph: "3px"
  row: "4px"
  nav: "6px"
  field: "8px"
  card: "10px"
spacing:
  row-y: "7px"
  row-x: "13px"
  stack: "10px"
  card-x: "16px"
  card-y: "12px"
components:
  result-row:
    rounded: "{rounded.row}"
    padding: "7px 10px 7px 13px"
  settings-card:
    backgroundColor: "{colors.card-light}"
    rounded: "{rounded.card}"
    padding: "12px 16px"
  file-glyph:
    size: "32px"
    rounded: "{rounded.glyph}"
---

# Design System: Magi

## Overview

**Creative North Star: "The Quiet Pane"**

Magi is a sheet of the operating system that happens to know where your files are. The search window is a single pane that appears on a hotkey, answers, and leaves; settings and onboarding are the same pane grown to a window. Nothing in the interface competes with the user's own files, whose thumbnails and file-type glyphs are the most colorful things on screen.

The system borrows the host OS's voice rather than imposing one: platform-default type, native window materials (blurred on Windows 11, solid on Windows 10), a macOS/Windows-settings-style grouped-box layout, and a single cool blue accent that appears only where the user's attention is needed (selection, match source, progress, primary action). Density is compact and list-like; motion is a 150 ms ease-out fade, never decoration. Settings adds three authored moments, each marking a state change: the nav accent bar sliding to the new section (240 ms), the section fading in (220 ms), and the Index counts rolling to new values.

**Key Characteristics:**
- System type, no bundled typeface; hierarchy by size and weight, not family.
- Tonal layering (panel, window, card, sidebar) with hairline borders; shadow only as a faint lift in light mode.
- One accent, used sparingly; status colors carry meaning and are never decorative.
- File types are told apart by small saturated glyph squares using each format's owning-app color.
- Light and dark are first-class, follow the OS unless forced.

## Colors

Cool-neutral surfaces with one azure accent; the only saturated colors are the accent, status tones and file-type glyphs.

### Primary
- **Azure Signal** (dark `#4cc2ff`, light `#0a64d8`): selection tint (alpha ~14%), selection bar, match-source text on the selected row, progress, primary buttons (text on it flips to black in dark, white in light), focus. Also the "busy" status tone.

### Neutral
- **Graphite Panel** (dark `#202024`) / **Pearl Panel** (light `#f6f6f8`): the search window; alpha comes from the backdrop setting so the OS blur shows through.
- **Window Slate** (`#202020`) / **Window Mist** (`#f5f5f7`): the opaque settings and onboarding window.
- **Card Charcoal** (`#2b2b2b`) / **Card White** (`#ffffff`): grouped boxes inside settings. Fields inside a card take the window color to sit below it.
- **Sidebar Black** (`#191919`) / **Sidebar Fog** (`#ececec`): settings navigation rail.
- **Ink** (`#f3f3f5` / `#1b1b1f`): primary text. **Mute** (`#b4b6bd` / `#5a5a5e`): secondary text, paths, dates, icons at rest.
- **Hairline** (white 8% / black 8%): every border and divider.

### Status and semantic
- **Ok** (`#6ccb5f` / `#0f7b0f`), **Warn** (`#fcb452` / `#9d5d00`), **Err** (`#ff99a4` / `#c42b1c`): dots and pills; the word beside a dot is colored only for Warn and Err.
- **Match Highlight** (`#ffd600` at 30% dark / 45% light): background behind matched terms in snippets, text raised to Ink.

### File-type glyphs
PDF `#c4314b`, Word `#185abd`, Excel `#1e7145`, PowerPoint `#c43e1c`, code `#5c2d91`, image `#0f766e`, text/other `#6b7280`, always with white label text.

### Named Rules
**The One Accent Rule.** Azure Signal marks what the user is acting on or what the system is doing; it never decorates. If a screen has accent on more than the selected row, the active control and one status, something is over-colored.

**The Tint Floor Rule.** Over a blurred backdrop, light mode never drops below 85% panel opacity and dark mode never below 75%: dark text washes out on a dark wallpaper, and Mute washes out over a bright one. The slider's range is squeezed into the part above the floor.

## Typography

**All roles:** the platform UI font (Segoe UI Variable, SF Pro, system sans on Linux). No custom font is loaded.

**Character:** invisible and native; the files and snippets are the content, so the type must not have a voice of its own.

### Hierarchy
- **Heading** (600, 26px): settings and onboarding page titles.
- **Body** (400, 14px): the query, result file names, settings labels, buttons.
- **Snippet** (400, 12.5px, line-height 1.45, 2-line clamp): result excerpts in Mute with highlighted matches.
- **Caption** (400, 12px / "text-xs"): paths, dates, match sources, setting notes, footer.
- **Label** (600, 12px, Mute): group sub-headings.
- **Keycap** (16px): shortcut caps in onboarding and the hotkey hint.

### Named Rules
**The Native Voice Rule.** Never bundle or specify a typeface; weight and size carry hierarchy so each OS renders its own best text.

## Layout

The search window is one centered column that grows with its results (it measures its children and resizes to fit): an input row, an optional hotkey hint, a thin loading bar, the results list, and a status footer. Rows are 32 px glyph/thumbnail plus a two-line block plus right-aligned date and match source. Spacing is a 10 px stack rhythm with 12-18 px gutters; rows use 7 px vertical padding for scan density.

Settings (900x620, min 860x480) and onboarding (880x600, min 800x520) are fixed-window desktop layouts: a sidebar rail plus a content column of 10 px-spaced groups. There is no mobile or fluid breakpoint system; the design adapts by window minimum size and by rows truncating with ellipsis (paths truncate in the middle).

## Elevation & Depth

Depth is tonal, not shadowed. Layers step from window to card to field by lightness, each edged with a 1 px hairline. In light mode only, cards and groups add Tailwind-style extra-small shadow so a white box reads against a near-white window. Dark mode has no shadows. In the search window depth comes from the OS backdrop (Blurred on Windows 11, solid on Windows 10), not from CSS-like effects.

### Named Rules
**The Flat-At-Rest Rule.** Surfaces are flat; the only lift is the faint light-mode card shadow. State (hover, press, selection) is a background tint, and press nudges an element down by a few pixels rather than scaling it.

## Shapes

Soft, small, consistent: 3 px for glyph squares and thumbnails, 4 px for result rows, 6 px for nav items and hints, 8 px for fields and keycaps, 10 px for cards and grouped boxes; pills and dots are fully round. The selected result gets a 3 px fully-rounded accent bar centered on its left edge, spanning about a third of the row height.

## Components

### Result row
Glyph or thumbnail, file name (truncating), optional page and folder in Mute, detail (snippet or middle-truncated path), date on the right with the match source under it. A snippet row's folder follows the name behind a folder icon, always: a bare folder name reads as part of the file's; a path row shows its last two folders, the full path in a tooltip. Muted text is checked against WCAG at the tint floor (`muted_text_stays_readable_at_the_tint_floor`): 5.8:1 (dark) and 5.4:1 (light) over a mid-grey blur, 3.5:1 and 4.5:1 over the worst backdrop (pure white / pure black). "Related" is named only when it is the row's sole source; the folder name is caption size. A source tag is "Words", "Related", "Looks like", "Text in image", "QR code" or "File name", and "File name" appears only when the words really are in the name. While a Related, Looks like, Text in image or QR row is selected and results are complete, the footer says why in one short phrase in place of the index status. Selected: Azure selection tint plus the accent bar, and the match source turns accent. Pressed: the tint doubles. Copy feedback swaps the match source for an accent check and "Copied" with the 150 ms fade.

### Grouped settings box
Rows in one 10 px-rounded, hairline-bordered Card with dividers that start where the text does (42 px inset). Each row is an icon, a label with a Mute note, then its control; minimum height 44 px.

### Status dot and badge
7 px dot in a tone color; a badge is the same tone at 12% fill with matching text in a pill. Words beside dots stay Mute unless Warn or Err.

### Buttons, switches, inputs
gpui-component widgets re-skinned: primary is the accent (hover 90%, active 80%). In dark mode, unchecked checkboxes, field borders and the switch track lift to Mute at 60% so they stay visible; the switch thumb is Ink.

### Keycaps
8 px-rounded caps with 16 px labels (platform glyphs where the OS has them) showing the global hotkey in onboarding and under the input when search was not opened by the hotkey.

### Navigation (settings)
Sidebar rail on Sidebar color; a ghost "Search files" item at its foot (hotkey in its tooltip), the way back to the product; items are 6 px-rounded with a 15 px icon and 10 px gap; the selected item takes the Chip tint (white or black at ~6-8%).

## Do's and Don'ts

### Do:
- **Do** let the OS draw its materials: Blurred on Windows 11 search, Opaque on Windows 10, native title bars.
- **Do** keep every border a 1 px hairline of Ink at ~8% alpha.
- **Do** show why a result matched on every row, not only the selected one.
- **Do** animate arrival with the 150 ms ease-out fade only (settings' three moments above are the exception), and rely on GPUI's reduced-motion end state.
- **Do** use file-type glyph colors only on the 32 px glyph squares.

### Don't:
- **Don't** add a bundled typeface, gradient text, or decorative illustration.
- **Don't** use the accent as a fill for large surfaces or as a second status color.
- **Don't** add drop shadows in dark mode or heavier shadows in light mode.
- **Don't** use bright status colors for the word beside a dot unless it is a problem (Warn, Err).
- **Don't** let the logo mark, which is single-color by design, appear in more than one color.
