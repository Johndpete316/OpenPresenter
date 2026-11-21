# Editor audit (before the editor redesign)

Written 2026-11-21, before E0 of the Slide Editor Redesign plan. It covers what the M7 editor is today, where each part goes in the new milestones, and what is kept, refactored or replaced.

## What exists

### `pro6` crate: document model and edit operations
- **Model** (`model.rs`, `extra.rs`, `raw.rs`):
  - Typed view over the raw XML.
  - Every typed node keeps an `Extra`: the original element minus the typed children, plus where those children sat.
  - The writer re-emits the raw text for any value that hasn't changed, so all fixtures round-trip byte-identically.
  - Display elements are `Text(TextElement)` or `Other(RawElement)`.
  - Images and shapes stay raw. They are kept, never interpreted.
- **TextElement** already has typed fields for:
  - `position` (`RVRect3D`)
  - `rotation`, `opacity`, `locked`
  - `vertical_alignment`
  - `shadow`, `stroke`
  - `fill_color`
  - the `drawing_shadow` / `drawing_stroke` / `drawing_fill` flags
  - the RTF / PlainText / WinFlow / WinFont payloads

  Nothing can edit those fields except the text.
- **RTF** (`rtf.rs`):
  - `parse` gives paragraphs (align) → runs (font, size, colour, bold, italic, underline).
  - `write` produces RTF from the same structure.
  - `scale_sizes` is used by rescale.
  - No line spacing, letter spacing or per-run outline/shadow.
- **Edit operations** (`ops.rs`, `edit.rs`):
  - `Op` is an enum of 14 forward-only operations:
    - SetText
    - Add / Duplicate / Remove / Move slide
    - Add / Rename / Recolor / Move / Remove group
    - Add / Rename / SetGroups / Remove arrangement
  - Each one validates before it mutates, so a failed op changes nothing.
  - **There is no revert.**
  - `SetText` writes the whole box in the first run's style and drops WinFlow/WinFont. Dropping them is still unverified in PP6.
- **Arrangements** (`arrangement.rs`): read through the raw tree. The writes use the assumed `RVSongArrangement`/`groupIDs` structure, which has since been verified against real files.
- **Rescale** (`rescale.rs`): canvas resize with font scaling. E7 reuses it for themes with a different canvas size.

### `pp6core` crate: where edits are applied
- `Command::Edit { presentation, op }`:
  - The actor clones the open `Document`, applies the op and swaps the `Arc` in `ShowState.docs`.
  - The path is marked in `unsaved`.
  - **Edits apply to the live document.** If the edited presentation is live, the live slide follows by UUID and the output updates straight away.
- `Command::Save`: `save_with_backup` on the I/O thread. It keeps `<name>.pro6.bak` and writes atomically.
- `Command::Revert`: drops the open doc and reloads it from disk.
- Unsaved edits are not in the crash-restore file. The crash-restore file isn't used at all any more ("start blank").

### `render` crate
- `render_slide(fonts, doc, slide, w, h)` lays out text with cosmic-text into a CPU RGBA buffer, at canvas size scaled uniformly to fit.
- The same function draws all of these, which is the WYSIWYG base the plan wants:
  - output (uploaded to GL)
  - operator and editor thumbnails
  - `pp6tool render`
  - the phone remote previews
- **Not drawn yet**, and each one only produces a warning:
  - text shadow
  - text outline
  - rotation
  - image elements
  - shapes

### `app` crate: the editor UI (`editor.rs`, 631 lines)
- Edit mode is a mode of the presentation grid ("Editor" button). Clicking a tile selects it; nothing goes live.
- The UI has:
  - thumbnail tiles under group headers, using the group colour
  - per-tile `< > Dup Del` buttons
  - a per-group header with colour picker, rename, `^ v`, `+ Slide`, and delete with confirmation
  - a collapsible Arrangements panel: create, rename, delete, reorder entries with `< > x`, add a group
  - a slide text box under the grid: a multiline plain-text field, applied with "Apply text" or Ctrl+Enter
  - Save / Revert and an unsaved dot in the header; quitting with unsaved edits asks Save / Don't save / Cancel
- There is **no** canvas view, element selection, drag, handles, rulers, zoom, undo, rich text, image tools, or drag-and-drop reordering.

## Tests covering the editor today
| Test | Covers |
|---|---|
| `pro6/tests/ops.rs` (4 tests) | Every `Op` through write → re-parse; unique UUIDs; arrangements follow group rename/move/remove; failed edits change nothing |
| `pro6/tests/edit.rs` (6 tests) | `set_text` regenerates RTF and PlainText only; the WinPayloads flag; mixed fonts take the first run's style; CRLF; refusal without a style; the PP6 placeholder |
| `core/tests/actor.rs::edits_follow_the_live_slide_save_and_revert` | Edits while live, refused edit → notice, save + `.bak`, revert |
| **New:** `core/tests/editor_regression.rs` | open → set text → reorder slides → move and rename group → add arrangement and edit it → `save_with_backup` → reopen from disk; the reopened summary equals the edited one; the backup is byte-identical to the original; a second save is stable |
| `pp6tool roundtrip fixtures/` | Byte-identical round-trip of every parseable fixture |

## How each part maps to the new milestones

| Existing part | Milestone | Plan |
|---|---|---|
| `pro6` typed + raw model, writer, byte-identical round-trip | E0 | **Keep.** This is the `.pro6` reader/writer (the "round-trip writer from M1" in the plan). |
| `rtf.rs` parse/write (runs: font, size, colour, B/I/U, paragraph align) | E0, E2 | **Keep, extend** with line/letter spacing once verified. It becomes the conversion between native runs and RTF. |
| `Op` enum (14 ops, apply-only) | E0 | **Refactor** into undoable commands (see Questions, 1). The op logic and its validation are kept. |
| `Command::Edit` applying to the live doc | E0/E1 | **Replace** with an editor working copy (the plan: "Editing vs Live"). This is a behaviour change from M7 (see Questions, 2). |
| `Command::Save` + `save_with_backup` | E0 | **Keep** for `Pro6` mode. Add the `.opres` save next to the original for `Native` mode. |
| `Command::Revert` | E1 | **Keep** (it becomes "close without saving" / reload). |
| `render::render_slide` | E1–E3 | **Keep and extend** (shadow, outline, rotation, images, adjustments). The editor canvas uses it at canvas size. |
| Thumbnail worker (`thumbs.rs`) | E1 | **Keep.** It already renders through `render`; the slide strip uses it. |
| `editor.rs` grid edit mode | E1 | **Replace** the layout (slide strip / canvas / inspector / bars). The group header, arrangement panel and op wiring are moved into the slide strip and the Slide tab, not rewritten. |
| Plain-text slide box | E2 | **Replace** with in-canvas rich text editing. It stays as the fallback for slides without a text box until E2 lands. |
| `rescale.rs` | E7 | **Keep**: themes with another canvas size. |
| Raw `RVImageElement` passthrough | E3 | **Keep** until the real PP6 image structure is known. Today the only image element is in the *synthetic* fixture (`unknowns.pro6`), which is invented XML. A real file with an image is needed. |

## Decisions (John, 2026-11-21)

The audit found four conflicts with the plan. John decided each one:

1. **Undo is snapshot-based.**
   - Each undo step keeps the previous immutable document (cap of 200 steps per document).
   - Commands describe each step (label, features used) but don't implement their own revert.
2. **The editor works on a copy.**
   - Edits never reach the output until Save.
   - Saving a live presentation updates the output on the next slide change.
   - This replaces the M7 rule "edits to the live slide show on the output straight away".
3. **The native model is used app-wide.**
   - A new `doc` crate holds the native model, used by core, render, thumbnails, the phone remote and pp6tool.
   - `.pro6` is only load → convert and convert → save.
   - Converted nodes keep their original pro6 XML, so untouched parts of a `.pro6` save are byte-identical.
4. **Exporting a `Native` document to `.pro6` asks each time.**
   - When there is something incompatible, a dialog offers two choices:
     - skip the incompatible slides
     - write them without the unsupported parts
   - Either way, it lists what was left out.

## Other notes for later milestones
- **GPU adjustments (E3).** Rendering is CPU today, and thumbnails, `pp6tool render` and the phone remote have no GL context. A shader-only adjustment would make them differ from the output.
  - Proposed: the shader for the output and editor canvas, plus a CPU reference implementation of the same maths in `render` for the GL-less paths, with a test comparing the two.
  - To decide at E3.
- **Shadow, outline and rotation aren't drawn by `render` yet.** E2's acceptance ("renders the same in editor, output and `pp6tool render`") needs them drawn first.
- **WinFlowData/WinFontData drop on text edits** is still unverified in PP6 (PLAN.md M7). It matters for every `Pro6`-mode save.
- **Name.** The plan calls the app OpenPresent (`.opres`, `.optheme`). The crates, binary and installer are still `pp6`. The extensions can be added without renaming anything; a rename is a separate decision.
