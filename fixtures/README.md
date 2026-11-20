# Fixtures

Hand-written `.pro6` files built from the unofficial spec. **None of these come from real
ProPresenter**, so they test the parser against our *assumptions* (see `docs/format-notes.md`).
Corpus testing against real files comes later.

## Editing

Don't edit the generated `.pro6` files by hand. Edit the template in `src/*.pro6.in` (or
`src/invalid/*.pro6.in`) and the readable payloads in `src/text/*.rtf|*.txt`, then run:

```sh
fixtures/encode.sh
```

`@@RTF:name@@` / `@@TXT:name@@` placeholders are replaced with base64 of
`src/text/name.rtf` / `src/text/name.txt`. The generated files are committed.

## Valid

| File | Canvas | Covers |
|---|---|---|
| `song_1024x768.pro6` | 1024×768 | Groups **Verse 1** (2 slides), **Chorus** (2 slides, 2nd `enabled="false"`), **Verse 2** (1 slide, 4 lines). Windows-style RTF, Arial `\fs110`, centred, white. Shadow with a long float (`0.3333333432674408`) for round-trip. |
| `two_fonts_1920x1080.pro6` | 1920×1080 | One element with **Arial `\fs120`** + **Georgia `\fs80`**, two colours, Cocoa-style RTF (`\` + newline), `\'e9` and `\u8212?` escapes. Unnamed group with transparent colour; group **Ref**. Spec shadow example, stroke width 2, `&amp;` in an attribute. |
| `unknowns.pro6` | 1024×768 | **Synthetic** structure: unknown root/group/text attributes, unknown root child, a non-group item in `groups`, a non-text display element, a populated `arrangements` array, a non-empty slide `cues` array, timeline `timeCues`/`mediaTracks` (loop on, 30 s), `WinFlowData`/`WinFontData`. Element names prefixed `Synthetic` are invented and not real PP6. |
| `render_1024x768.pro6` | 1024×768 | Uses the **bundled fonts** below. Slide 1 centred + middle; slide 2 left/right paragraphs, sans + serif, white + yellow, top; slide 3 bold, bottom, `drawingShadow="true"` (warning); slide 4 uses **Futura PT**, deliberately missing (`MissingFont`). |

Totals across valid fixtures: 4 files; canvases 1024×768 ×3, 1920×1080 ×1; 7 groups;
12 slides (1 disabled); fonts Arial (3 files), Georgia (1), Liberation Sans (1), Liberation Serif (1), Futura PT (1, never installed).

## Fonts (`fonts/`)

Liberation Sans (Regular, Bold) and Liberation Serif (Regular), SIL Open Font License 1.1
(`fonts/LICENSE-Liberation.txt`). Render tests load **only** these, so results don't depend
on what's installed. Liberation Sans is metric-compatible with Arial, but the renderer never
substitutes it for Arial: a missing font is an error.

## Invalid (`invalid/`)

Each must fail with a specific `ParseErrorKind`:

| File | Expected error |
|---|---|
| `truncated.pro6` | `Xml` (file ends inside open elements) |
| `bad_base64.pro6` | `Base64` |
| `bad_rect.pro6` | `BadRect` |
| `wrong_root.pro6` | `NotPro6Root` |
| `missing_canvas.pro6` | `MissingAttr` (`width`) |
| `unbalanced_rtf.pro6` | `Rtf` (parses as XML; fails when RTF is read) |
