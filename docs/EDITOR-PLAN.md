# OpenPresent — Slide Editor Redesign — Implementation Plan

The basic editor from **M7** in `PLAN.md` is already built. This plan **redesigns and expands** that editor; it does not start a new one.

- Read `PLAN.md` first. Its priorities, error-handling rules and working rules all apply here.
- The audit of the existing editor, and John's decisions on the conflicts it found, are in `docs/editor-audit.md`.
- Existing M7 behaviour must keep working at every step: text editing, slides, groups, arrangements and saving to `.pro6`. The one exception is live edits (see Editing vs Live).
- Work one milestone at a time. Don't start the next until the current one's acceptance criteria pass.
- Don't add features that aren't listed here. Ask first.

## Before E0 — Audit the Existing Editor
- [x] `docs/editor-audit.md` (2026-11-21)
- [x] Regression test for current M7 behaviour: `crates/core/tests/editor_regression.rs`
- [x] Conflicts raised with John and decided (see the audit's Decisions)

## Goals
- Full text and image alignment.
- Full rich text: font, colour, size, outline, drop shadow.
- Full image support:
  - import
  - set as background or foreground
  - basic adjustments (brightness, saturation, colour, vignette)
  - alignment
  - layers
- Themes, lyrics paste-to-slides, a fit-text button, sane snapping, copy style / apply to all.
- Editing workflow modelled on **Canva**:
  - a contextual toolbar that changes with the selection
  - side panels for effects and adjustments
  - one-click alignment
  - copy style
- Layout modelled on **ProPresenter**: slide strip left, canvas centre, inspector right.
- Speed still comes first:
  - The editor canvas uses the same `render` crate as the live output (WYSIWYG, one code path).
  - The editor never blocks on disk.

## Layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ Top bar: Insert [Text] [Image]  |  Undo Redo  |  Theme ▾  | Paste Lyrics │
├──────────────────────────────────────────────────────────────────────┤
│ Contextual toolbar (Canva-style, changes with selection)               │
│  Text:  Font ▾  Size [-][ 72 ][+]  Colour  B I U  Align ▾  Spacing     │
│         Effects  Fit Text  Copy Style  Position                        │
│  Image: Edit/Adjust  Flip  Opacity  Set as Background  Copy Style  Position │
├───────────┬──────────────────────────────────────────┬───────────────┤
│ Slide     │  Rulers                                  │ Inspector     │
│ strip     │  ┌────────────────────────────────┐      │ tabs:         │
│ (grouped  │  │        Canvas (doc size)       │      │ Text / Image  │
│ by slide  │  │                                │      │ Position      │
│ group,    │  └────────────────────────────────┘      │ Layers        │
│ colours)  │                                          │ Slide         │
├───────────┴──────────────────────────────────────────┴───────────────┤
│ Bottom bar: X Y W H readout | snapping ⦿ | guides ⦿ | safe area ⦿ | zoom | Slide notes | BG colour │
└──────────────────────────────────────────────────────────────────────┘
```

- **Slide strip:**
  - thumbnails under group headers (name and PP6 colour)
  - drag to reorder, including between groups
  - right-click menu: duplicate, delete, disable, new group
- **Side panels** (Effects, Adjust, Position) open in the inspector column, Canva-style, and close back to the tab view.
- **Overflow indicator:** a red badge on any text box whose text is bigger than the box, and on its slide thumbnail.

## Native Format + `.pro6` Compatibility

### Native Format
- Extensions:
  - **`.opres`** (OpenPresent presentation)
  - **`.optheme`** (themes)

  Each extension is defined in one constant.
- The file is a single JSON document (`serde`) with a `format_version`. Each version bump adds a migration function.
- Text is stored as structured runs, **not RTF**. Each run has: font, size, colour, weight, style, underline, outline and shadow. RTF is only produced when writing `.pro6`.
- Group, slide, element and arrangement UUIDs are carried over from `.pro6` unchanged.
- Media is referenced by relative path plus a SHA-256 hash, so a moved or changed file gives a readable error. Originals are never modified.
- Image adjustments are stored as parameters and applied at render time on the GPU, so they are non-destructive.
- Themes are separate files: `Library/Themes/<name>.optheme`.

### Compatibility Modes

Each document has a mode:

|Mode|When|Saves as|
|---|---|---|
|`Pro6`|Opened from `.pro6`, or a new presentation (default)|`.pro6` (round-trip writer from `PLAN.md` M1)|
|`Native`|After the user accepts a native-only feature on any slide|`.opres`|

- Every editor feature declares its compatibility in one place: `fn pro6_support(feature) -> Supported | NativeOnly | Unverified`.
- In `Pro6` mode, an edit that would use a `NativeOnly` feature shows a modal **before** it applies:
  > **This won't work in ProPresenter 6.** _Vignette_ can't be saved in a `.pro6` file. If you continue, slide "Chorus 2" will only open correctly in OpenPresent. [ Don't do it ] [ Continue anyway ]
- **Don't do it:** the edit is discarded and nothing changes.
- **Continue anyway:**
  - the edit applies
  - the slide is flagged `pro6_incompatible`, with a badge in the slide strip
  - the document switches to `Native` mode
- On the first save in `Native` mode, `<name>.opres` is written **next to** the original `.pro6`.
  - The original `.pro6` is never overwritten or deleted.
  - From then on, the library shows the `.opres` in place of the `.pro6`.
- `Unverified` features are treated as `NativeOnly` until they are verified against PP6 (see Compatibility Matrix).
- Exporting a `Native` document to `.pro6` **asks each time** (John, 2026-11-21).
  - When there's something incompatible, a dialog offers two choices: skip the incompatible slides, or write them without the unsupported parts.
  - Either way, it lists what was left out.

### Compatibility Matrix

Filled in from real files and testing in PP6. Starting state:

|Feature|`.pro6`|Notes|
|---|---|---|
|Font family / size / colour / bold / italic / underline|Supported|RTF|
|Horizontal text alignment|Supported|RTF paragraph|
|Vertical text alignment|Supported|`RVTextElement.verticalAlignment`|
|Line / letter spacing|Unverified|RTF `\sl` / `\expnd`|
|Element position / size / rotation / opacity|Supported|`position`, `rotation`, `opacity`|
|Text drop shadow|Unverified|element `shadow` string exists; per-run?|
|Text outline|Unverified|stroke dictionary exists on element|
|Image element (foreground)|Unverified|not in gist — find element type in real files|
|Image as slide background|Unverified|slide cues / media — find in real files|
|Image brightness / saturation / colour|Unverified||
|Vignette|NativeOnly||
|Layer lock / hide|Unverified|`locked` exists; hide unknown|
|Theme link|Native metadata|in `Pro6` mode themes apply as a copy (no live link), so no warning|

## Interaction Rules

### Snapping ("sane")
- Snapping only happens while dragging or resizing with the mouse. Arrow-key nudges never snap (1 px; Shift = 10 px).
- The threshold is in **screen pixels** (default 6 px), not canvas units, so it feels the same at any zoom.
- Snap targets, in priority order:
  1. canvas centre lines
  2. canvas edges
  3. safe-area edges
  4. other elements' edges and centres
  5. equal spacing between elements
- At most one snap per axis at a time: the nearest target wins, so targets never fight.
- No grid snapping unless the grid is turned on (off by default).
- Hold **Alt** while dragging to bypass snapping. Snapping can also be toggled in the bottom bar.
- The active snap shows as a magenta guide line. Snapping to equal spacing also shows a distance label (Canva-style). Guides disappear on mouse release.
- **Shift** while dragging constrains to one axis. **Shift** while resizing keeps the aspect ratio (images keep it by default).

### Selection + Alignment (Canva-style)
- Click to select, Shift-click to add, drag a marquee to multi-select, Tab to cycle through elements.
- Position panel:
  - **Arrange:** Forward, Backward, To Front, To Back.
  - **Align to page:** Left, Centre, Right, Top, Middle, Bottom.
  - **Align selection** (2+ selected): the same six, relative to the selection bounds.
  - **Distribute** (3+ selected): horizontally or vertically.
  - Exact X / Y / W / H / rotation fields.
- Layers tab: elements listed top to bottom; drag to reorder, lock, hide, rename.

### Copy Style
- **Copy Style** button (paint roller) in the contextual toolbar. Shortcuts: Ctrl+Alt+C to copy, Ctrl+Alt+V to paste.
- Copies the full style of the selected element:
  - text: all run attributes, alignment, spacing, outline, shadow
  - image: adjustments, opacity
- Paste onto:
  - the selected element(s)
  - **all text boxes on this slide**
  - **all slides in this group**
  - **all slides in this presentation**
- One undo step, whatever the scope.

### Undo / Redo
- Every edit is a command. One user action is one undo step: a drag is one step, not one per frame.
- Undo is snapshot-based (John, 2026-11-21): each step keeps the previous immutable document.
- Undo history is per open document, kept in memory, and survives switching slides.

### Editing vs Live
- The editor works on a copy. Edits don't change the live output until the document is saved.
- Saving a presentation that is currently live updates the live output on the next slide change.
- This replaces the M7 behaviour (John, 2026-11-21).

## Milestones

### E0 — Native Format + Compatibility System
- [x] New `doc` crate, used app-wide (John, 2026-11-21):
  - native document model
  - JSON read/write with `format_version` and migrations
- [x] Lossless conversion `.pro6` model → native model → `.pro6`. The round-trip must stay clean for compatible documents.
- [x] `pro6_support(feature)` registry, the compatibility mode on the document, and the per-slide `pro6_incompatible` flag
- [x] Save rules: `Pro6` mode → `.pro6`; `Native` mode → `.opres` next to the original, which is left untouched
- [x] Library indexes `.opres` and prefers it over a `.pro6` with the same name
- [x] Command pattern and undo/redo stack
- [x] `pp6tool convert` (pro6 → opres) for testing
- **Accept:** fixtures convert both ways with zero semantic diff; undo/redo is unit-tested.
- **Status (2026-11-21):** done.
  - **Round trip.** Every fixture exports byte-identical when unchanged. After each editor operation the document survives a `.pro6` round trip with zero semantic difference. Native edits produce the same groups, slides, texts and arrangements as the M7 `pro6` operations. `.opres` JSON round-trips. All of these are tests in `crates/doc/tests/convert.rs`.
  - **`pp6tool convert`** checks the way back before writing anything and never overwrites a file. `.opres` → `.pro6` takes `--skip-incompatible`.
  - **How export works:**
    - The `.opres` embeds the original `.pro6` XML.
    - Every native group, slide and element records the original node it came from (`src`), so moves and copies keep their XML.
    - Export changes only what differs from a fresh conversion of the original.
    - Things the native model doesn't describe are kept and not drawn, the same as before: cues, media, shapes, timeline, unknown XML.
  - **Text** is stored as runs. RTF is regenerated only for boxes whose text changed; that drops WinFlow/WinFont, as M7 did.
  - **Unreadable text** (damaged RTF, a missing font-table entry, no RTF) is kept and flagged on the box. It's reported when drawn and can't be edited, the same as before.
  - **Editing works on a copy** (`ShowState::editing`), with undo history per document (200 steps). Save makes the copy the saved document; the output switches to it at the next slide change, following the live slide by UUID. Revert drops the copy. Undo/Redo buttons and Ctrl+Z / Ctrl+Shift+Z sit in the current editor header until E1.
  - **Native-only edits:**
    - `Command::Edit` refuses an edit that needs native mode unless `accept_native` is set ("Continue anyway").
    - With it set, the touched slides are flagged and the document switches to native mode.
    - No edit uses a native-only feature until E2/E3, so the consent modal arrives with the first one. The rule itself is tested.
  - **Exporting a native document** to `.pro6` (`doc::export_pro6`, with skip or simplify) exists in the library and `pp6tool`. The app's "ask each time" dialog comes with the first native-only feature.
  - **Changed outline, shadow or arrangement colour:** exporting one gives a readable "can't be written to .pro6 yet" error, because nothing edits them before E2.
  - The M7 `pro6::ops` are no longer used by the app. They stay, with their tests, for `pp6tool` and as the reference the native edits are checked against.

### E1 — Editor Shell (Rework of the M7 Layout)
- [x] Rework the existing M7 layout into the layout above:
  - top bar and contextual toolbar
  - slide strip with groups
  - canvas with rulers and zoom (fit, 50/100/200%)
  - inspector tabs
  - bottom bar
- [x] Canvas renders through `render` at document canvas size, scaled to the viewport
- [x] Slide strip:
  - keep the existing M7 slide and group operations
  - add any that are missing: reorder, move between groups, duplicate, delete, disable/enable, new/rename/recolour group
- [x] Selection, move, resize and rotate handles; arrow-key nudging
- [x] Slide notes and background colour in the bottom bar
- [x] Open, save, close; dirty indicator; prompt on close with unsaved changes
- **Accept:** open a fixture, rearrange slides and groups, move a text box, save, reopen — identical.
- **Status (2026-11-21):** done. The acceptance test is `e1_rearrange_move_save_reopen_identical` in `crates/core/tests/editor_regression.rs`.
  - **Layout.** The editor fills the window under the app toolbar and has the layout above.
  - **Top bar:** title with an unsaved dot, Undo/Redo, Save (Ctrl+S), Revert, Close editor. Insert, Theme and Paste Lyrics are added with E2/E3, E7 and E8.
  - **Contextual bar.** Shows what's selected, Edit text, Straighten, and a locked note. The text controls arrive with E2.
  - **Inspector tabs:** Text (the box's text, with its font, size and colour shown) and Slide (enabled, group, timer, notes, background). Position and Layers come with E4, Image with E3.
  - **Canvas:**
    - Drawn on its own thread by `render`, the output's renderer, rendering only the newest request at the on-screen pixel size.
    - While dragging, it renders a preview copy, so it stays WYSIWYG.
    - Zoom: Fit, 50, 100 and 200%, with scroll-panning when zoomed in.
    - Rulers in canvas pixels mark the selected element's extent.
  - **Handles:**
    - Click to select; drag to move; eight resize handles keep the opposite edge in place, also when rotated; a rotate handle.
    - Double-click edits the text, and Esc deselects.
    - Locked boxes can't be moved.
    - A drag is one edit, sent on release.
  - **Arrow keys** nudge 1 px (Shift 10 px). A run of nudges, or a colour being dragged in a picker, is one undo step: `EditMerging`, two-second window. In the editor, arrow keys from the operator window no longer trigger Next/Prev; clicker keys (PageUp/PageDown etc.) still do.
  - **Slide strip:**
    - Arrangements panel on top.
    - Group headers with colour, double-click rename, `+` and a menu (rename, add slide, move up/down, delete).
    - Tiles: drag to reorder, including between groups. The right-click menu has duplicate, add after, disable/enable, move to group, new group after, delete.
    - Badges show the slide label and "not PP6".
  - **Closing** with unsaved edits asks Save / Don't save / Cancel. That covers both Close editor and the toolbar's Editor button.
  - **Rotation is now drawn** by `render`, in the output, thumbnails, `pp6tool render` and the phone remote. It is clockwise about the box centre, which is assumed (PLAN.md Unknowns).
  - **Tests:** the canvas and strip interactions run headless against a plain egui context with synthetic pointer and key input (`crates/app/src/editor.rs` tests). A hands-on check with a mouse is still to do.

### E2 — Rich Text
- [x] In-canvas text editing with caret and selection, built on the `cosmic-text` editor with per-run attributes
- [x] Font picker from installed fonts (`fontdb`), with search and preview; missing-font error per the `PLAN.md` rules
- [x] Size field with −/+ steppers; colour picker with recent colours
- [x] Bold / italic / underline
- [x] Horizontal align (left, centre, right, justify) and vertical align (top, middle, bottom)
- [x] Line spacing and letter spacing
- [x] Effects panel with live preview:
  - **Outline:** width, colour
  - **Drop shadow:** colour, angle, distance, blur, opacity
- [x] Overflow detection → red badge on the box and its thumbnail
- [x] Each attribute wired through `pro6_support` (warning modal when needed)
- **Accept:** every attribute renders the same in the editor, the output window and `pp6tool render`.
- **Status (2026-11-21):** done.
  - **Acceptance.** The editor canvas, the output and `pp6tool render` all draw with `render_slide`. `styled_text_renders_the_same_after_saving_and_reopening` in `crates/render/tests/render.rs` styles a box with every attribute, saves it as `.opres` and reopens it, with identical pixels.
  - **Editing in place:**
    - Double-click a box, or press Enter with one selected.
    - Typing, Enter (new paragraph), Shift+Enter (line break), Backspace/Delete (Ctrl = word), arrows / Home / End (Ctrl = word / whole text), Shift to select, mouse click/drag/double-click, Ctrl+A, copy / cut / paste, Ctrl+B / I / U.
    - Esc, clicking outside, another slide, or Save ends it as **one** undo step.
    - The canvas shows the text live through `render`.
    - **Built on cosmic-text, not its `Editor`:** caret, selection and hit testing come from the same cosmic-text layout the output uses (`render::layout_box`, tested to agree with the drawn text). The text model is the document's own runs (`doc::rich`, unit-tested). This avoids converting between cosmic-text's buffer and our runs on every keystroke.
  - **Toolbar** (appears when a text box is selected):
    - Font picker with search and each name drawn in its own font. A font that isn't installed shows in red.
    - Size with −/+, colour with eight recent colours, B / I / U.
    - Four alignments and three vertical alignments.
    - Spacing popover: line ×, letter px.
    - Effects.
    - While editing, changes apply to the selection, or to what you type next. Otherwise they apply to the whole box as one edit; slider runs merge into one undo step.
  - **Effects** opens in the inspector column with a Back button: outline (width, colour) and drop shadow (colour, opacity, angle, distance, blur).
  - **Rendering:** `render` now draws underline, outline, drop shadow, letter spacing and line spacing.
    - Outline uses a Euclidean distance transform, so corners are round.
    - Shadow is a three-pass box blur of the text plus outline.
    - Both are in the same pixel path as everything else.
  - **Overflow** is reported by the renderer. It shows as a red "Text doesn't fit" badge on the canvas box and "doesn't fit" on strip tiles. Editor thumbnails are now rendered fresh, not from the disk cache, which was keyed by the saved file and could show stale slides.
  - **"This won't work in ProPresenter 6"** modal: Don't do it / Continue anyway. It appears for outline, shadow, line spacing and letter spacing (all Unverified). Turning an effect off needs no prompt.
  - **Exporting a native document to `.pro6`** leaves out changed outlines, shadows and spacing (write without the unsupported parts). Turning off an existing outline or shadow is written to `.pro6`.
  - **Outline and shadow are per text box, not per run.** That's simpler to edit; see Unknowns for what real files do.
  - **Existing `.pro6` outlines and shadows** (`drawingStroke` / `drawingShadow`) are now drawn instead of warned about. The shadow offset's y is read as pointing up (Cocoa), so `{4, -4}` draws down-right. That is assumed; see Unknowns.
  - **Test fonts:** the bundled ones have no italic faces, so italic is tested through warnings only.

### E3 — Images
- [ ] Insert an image (file dialog, drag-and-drop, paste from clipboard): PNG, JPEG
- [ ] Copy imported images into the library media folder; reference them by relative path plus hash
- [ ] Foreground image element: move, resize (aspect locked by default), rotate, opacity, flip H/V
- [ ] **Set as Background:** fills the canvas with Fit / Fill / Stretch; always behind all elements
- [ ] Adjust panel (Canva-style sliders):
  - brightness, contrast, saturation, temperature, tint, vignette (amount, size)
  - live preview
  - reset per slider, and "reset all"
- [ ] Adjustments applied in a GPU shader at render time; originals never modified
- [ ] Large images downscaled to a cached texture at canvas resolution on the I/O worker
- **Accept:** an adjusted background and foreground image render identically in editor and output; the editor stays responsive while dragging sliders.

### E4 — Alignment, Layers, Snapping
- [ ] Position panel (arrange, align to page, align selection, distribute, exact fields)
- [ ] Layers tab (reorder, lock, hide, rename)
- [ ] Multi-select, and moving or resizing several elements together
- [ ] Snapping per Snapping ("sane"); magenta guides; equal-spacing labels
- [ ] Safe-area overlay at the output resolution (toggle); optional grid (off by default)
- **Accept:**
  - snapping unit tests for target priority, one snap per axis, and the screen-pixel threshold at several zoom levels
  - a manual check that snapping feels natural

### E5 — Copy Style / Apply to All
- [ ] Copy Style button and shortcuts; paste to selection / slide / group / presentation
- [ ] A single undo step for any scope
- **Accept:** apply a style to a 30-slide presentation in one action; one undo reverts all of it.

### E6 — Fit Text (Button)
- [ ] **Fit Text** button in the text toolbar. It shrinks the font size until the text fits the box, uniformly across runs so relative sizes are kept. It never runs automatically.
- [ ] Also available for all slides in a group or presentation, fixing every overflowing box in one step
- [ ] Respects a minimum size (a setting). If the text still doesn't fit at the minimum, it shows a readable warning instead of shrinking further.
- **Accept:** overflow badges clear after Fit Text; one undo reverts it.

### E7 — Themes
- [ ] A theme is a named set of:
  - canvas size
  - background (colour, or image plus adjustments)
  - text box position and size
  - full text style, outline, shadow, alignment
- [ ] Create a theme from the current slide; edit a theme in the editor (theme editing mode on a sample slide)
- [ ] Apply a theme to: a slide, a group, a presentation, or **many presentations** from the library (multi-select)
- [ ] `Native` docs keep a live link: editing the theme restyles linked slides, after a confirmation listing how many slides and presentations change
- [ ] `Pro6` docs: the theme applies as a copy of its styles (no link, no warning)
- [ ] Applying a theme with a different canvas size reuses the rescale logic from `PLAN.md` M4
- **Accept:** apply a theme to 10 fixtures in one action; the output matches the theme; one undo per presentation.

### E8 — Paste Lyrics to Slides
- [ ] **Paste Lyrics** opens a full-height text view with a live slide preview beside it
- [ ] A blank line starts a new slide. A line matching `[Name]` (e.g. `[Verse 1]`, `[Chorus]`, `[Ref]`) starts a new group with that name.
- [ ] Repeated group names reuse the same group, so a second `[Chorus]` points at the existing Chorus. The arrangement follows the paste order.
- [ ] Group colours: known names reuse their existing colour; defaults per name are configurable
- [ ] Applies the chosen theme, or the current presentation's style
- [ ] Works on a new presentation, or replaces or appends to the current one
- [ ] Reverse direction: show an existing presentation as editable lyrics text with `[Group]` headers, re-flowed on apply
- **Accept:** pasting a song with verses, chorus and repeats produces the right groups, slides and arrangement, and round-trips through the text view unchanged.

## Unknowns to Verify
- [ ] Every `Unverified` row in the Compatibility Matrix
- [ ] PP6 element type for images (foreground) and how slide backgrounds are stored
- [ ] Whether shadow and outline are per element or per run in `.pro6`.
  - Real hymn files use **per-run RTF outlines** (`\outl \strokewidth \strokec`, PLAN.md "Found in real files"). These aren't read or drawn yet.
  - The editor's outline is per box, using the element's stroke.
  - Decide with John whether to import RTF outlines (to the box outline when uniform) before relying on outlines in real files.
- [ ] Shadow offset direction: `{x, y}` read with y up (Cocoa), so `{4.95, -4.95}` is drawn down-right. Compare a shadowed slide with PP6.
- [x] Export-to-`.pro6` behaviour for `Native` documents: ask each export (John, 2026-11-21)

## Additional Tooling

Adds to the crate list in `PLAN.md`.

|Purpose|Crate|
|---|---|
|Native format|`serde_json`|
|Media hashing|`sha2`|
|Font enumeration|`fontdb` (via `cosmic-text`)|
|Clipboard (paste lyrics / images)|`arboard`|
|File dialogs|`rfd`|
|Image decode|`image` (PNG, JPEG)|
|GPU adjustments|custom GLSL fragment shader via `glow` (no extra crate)|
