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
|Image element (foreground)|Unverified|`RVImageElement` (real files): existing ones are read, drawn, and their position / rotation / opacity written back; new ones aren't written|
|Image as slide background|Unverified|real files use a full-canvas `RVImageElement`; no slide media cue seen|
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
- [x] Insert an image (file dialog, drag-and-drop, paste from clipboard): PNG, JPEG
- [x] Copy imported images into the library media folder; reference them by relative path plus hash
- [x] Foreground image element: move, resize (aspect locked by default), rotate, opacity, flip H/V
- [x] **Set as Background:** fills the canvas with Fit / Fill / Stretch; always behind all elements
- [x] Adjust panel (Canva-style sliders):
  - brightness, contrast, saturation, temperature, tint, vignette (amount, size)
  - live preview
  - reset per slider, and "reset all"
- [x] Adjustments applied ~~in a GPU shader~~ at render time (CPU, see status); originals never modified
- [x] Large images downscaled to a cached texture at canvas resolution ~~on the I/O worker~~ (on the render threads, see status)
- **Accept:** an adjusted background and foreground image render identically in editor and output; the editor stays responsive while dragging sliders.
- **Status (2026-11-21):** done. Acceptance test: `adjusted_images_render_the_same_after_saving_and_reopening` in `crates/render/tests/render.rs`.
  - **Two deviations from the plan, for John to confirm:**
    - **Adjustments run on the CPU in `render`, not in a GPU shader.** Thumbnails, the phone remote and `pp6tool render` have no GL context, so a shader would make them differ from the output (the audit's open question). One CPU path keeps every view identical. Brightness and contrast use a lookup table, the vignette a distance table, and rows are split across threads: about 3 ms for a full-HD picture, 7 ms for a whole 1920×1080 slide per slider step (release build). The canvas renders on its own thread and only the newest request, so dragging a slider never blocks the UI.
    - **Decoding and downscaling happen on the render threads** (output, editor canvas, thumbnails), not the core I/O worker, in one cache shared by all of them. Pictures are kept at most 4096 px on the long side, plus resized copies per drawn size (LRU, 512 MB). The thumbnail grid renders every slide, so a presentation's pictures are decoded before a slide goes live; a resize to the output size can still take ~10–20 ms at go-live the first time.
  - **Insert:** Insert → Image (file dialog), dropping files on the editor, or pasting (a copied picture, or a copied PNG/JPEG file). Insert → Text adds a text box styled like the nearest one (a copy, so a `.pro6` holds it; with no text box in the presentation it's a plain one and needs native mode).
  - **Media folder:** imports are copied on a worker thread to `Media/` next to the presentation, named `<name>-<first 8 of SHA-256>.<ext>` (the same picture is stored once), and referenced by that relative path plus the full SHA-256. A missing file shows "wasn't found" on the canvas and in the Image tab, and isn't drawn (the rest of the slide is). A changed file is drawn with a warning.
  - **Image element:** select, move, resize (keeps its shape; Shift resizes freely), rotate, nudge, Delete key. Toolbar: Adjust, Flip H, Flip V, Opacity, Set as Background, Straighten. Image tab: name, file, size, opacity, flips, Adjust, Set as Background, Delete.
  - **Set as Background** removes the element and makes it the slide's background image in one undo step. The Slide tab has the background image: Fit / Fill / Stretch, Adjust, Replace, Remove. It's drawn over the slide's background colour and under every element, as part of the slide (so Clear Text hides it, like a full-canvas image in PP6).
  - **Adjust panel:** brightness, contrast, saturation, temperature, tint, vignette amount and size; ↺ per slider and Reset all; live on the canvas; a slider drag is one undo step.
  - **Images in `.pro6` files** (`RVImageElement`, seen in the Welcome file): imported as image elements when they use the plain values seen in real files (no scaling, offset, fill, shadow, stroke or rounded corners); anything else stays raw and undrawn. Their `source` is an absolute Windows path, so on another PC they show as missing. Moving, resizing, rotating and opacity are written back to the `.pro6`; flips and adjustments need native mode.
  - **Compatibility:** inserting images, background images, flips and adjustments are Unverified (vignette NativeOnly), so they ask first. A native document exported to `.pro6` leaves new images and background images out.
  - **Also new everywhere:** image elements from `.pro6` files are now drawn (output, thumbnails, phone remote, `pp6tool render`) instead of warned about.

### E4 — Alignment, Layers, Snapping
- [x] Position panel (arrange, align to page, align selection, distribute, exact fields)
- [x] Layers tab (reorder, lock, hide, rename)
- [x] Multi-select, and moving or resizing several elements together
- [x] Snapping per Snapping ("sane"); magenta guides; equal-spacing labels
- [x] Safe-area overlay at the output resolution (toggle); optional grid (off by default)
- **Accept:**
  - snapping unit tests for target priority, one snap per axis, and the screen-pixel threshold at several zoom levels
  - a manual check that snapping feels natural
- **Status (2026-11-21):** built; the unit tests pass. **The manual feel check is still to do** (needs a person with a mouse).
  - **Snapping** (`crates/app/src/editor/snap.rs`, 10 unit tests: priority on ties, nearest wins, one snap per axis, threshold at 50/100/200%, safe area, equal spacing, grid only when on, resize edges):
    - Targets in priority order: canvas centre lines, canvas edges, safe-area edges (5% in), other elements' edges and centres, equal spacing (continuing a row's gap, or centring between two), then the grid when it's on (40 px).
    - 6 screen points, at any zoom. The nearest target wins per axis; at the same distance the higher priority one does.
    - Moving snaps the (rotated) box's bounds by its edges and centre. Resizing snaps only the dragged edges, and only for upright boxes resized freely (not while keeping the shape).
    - Alt drags freely; Shift while moving keeps to one axis (that axis's snap only); arrow nudges never snap.
    - Magenta guide lines; equal spacing shows the gaps with their size in pixels. Guides show only while dragging.
  - **Bottom bar:** Snap, Guides, Safe area (yellow outline 5% in from each edge, on the canvas, which the output scales to fit), Grid. The position readout follows the selection.
  - **Selection:** click, Shift-click to add or remove, drag a marquee on the empty canvas (selects what it touches; Shift adds), Tab / Shift+Tab cycle through the slide's elements. Several selected: one dashed box with handles; drag to move together, handles resize them together (keeping the group's shape unless Shift is held); arrows nudge all; Delete removes all; each is one undo step. Hidden elements can't be clicked on the canvas (select them in Layers).
  - **Position tab:** Forward / Backward / To Front / To Back, align to page (the selection moves as one), align selection (2+), distribute horizontally / vertically (3+, the outer two stay put), and X / Y / W / H / rotation fields for one element.
  - **Layers tab:** front to back, drag to reorder, click to select (Shift adds), double-click to rename, Lock and Hide. Raw `.pro6` elements are listed but can't be selected.
  - **Compatibility:** order and rename write to `.pro6`. Lock and hide are Unverified (the matrix), so they ask first; a native document exported to `.pro6` leaves hidden elements out.
  - The inspector tabs are now Text / Image / Position / Layers / Slide; Effects and Adjust open from the toolbars as side panels.

### E5 — Copy Style / Apply to All
- [x] Copy Style button and shortcuts; paste to selection / slide / group / presentation
- [x] A single undo step for any scope
- **Accept:** apply a style to a 30-slide presentation in one action; one undo reverts all of it.
- **Status (2026-11-21):** done. Acceptance test: `paste_to_a_thirty_slide_presentation_is_one_undo_step` in `crates/doc/tests/style.rs`.
  - **What's copied** (`doc::style`): for text, the first run's font, size, colour, bold, italic, underline and letter spacing, the first paragraph's alignment and line spacing, vertical alignment, outline and shadow; for images, the adjustments and opacity. Pasting text style restyles every run and paragraph of the target box; the text itself is untouched.
  - **Toolbar:** Copy Style (one element selected) and Paste Style ▾ (once something is copied) with Selected / All on this slide / All slides in this group / All slides in this presentation, each showing how many elements it would change. Text styles go on text boxes, image styles on images. Also in the bar when several are selected.
  - **Shortcuts:** Ctrl+Alt+C copies, Ctrl+Alt+V pastes onto the selection (not while typing).
  - **One step:** a paste is one `Batch` edit whatever the scope, so it's one undo; elements that already have the style aren't touched. A style with native-only parts (outline, shadow, spacing) asks first, like the toolbar does.
  - The plan mentions a paint-roller icon; the button is labelled "Copy Style" (no icon set has been added for the editor yet).

### E6 — Fit Text (Button)
- [x] **Fit Text** button in the text toolbar. It shrinks the font size until the text fits the box, uniformly across runs so relative sizes are kept. It never runs automatically.
- [x] Also available for all slides in a group or presentation, fixing every overflowing box in one step
- [x] Respects a minimum size (a setting). If the text still doesn't fit at the minimum, it shows a readable warning instead of shrinking further.
- **Accept:** overflow badges clear after Fit Text; one undo reverts it.
- **Status (2026-11-21):** done. Acceptance test: `fitting_a_presentation_clears_every_overflow_in_one_step` (`crates/app/src/editor/fit.rs`); `fit_text_shrinks_until_it_fits_everywhere` (`crates/render/tests/render.rs`) checks the result fits at 1024×768, 1920×1080, 1280×720 and 320×240.
  - **How it measures:** `render::fit_text` uses the renderer's own layout (so "fits" means the badge goes away), at canvas scale with one pixel to spare so it also fits at other output sizes. It finds the largest factor that fits (binary search) and scales every run by it, in half-pixel steps (what a `.pro6` stores).
  - **Toolbar:** Fit Text (this box) and ▾ → every overflowing box in this group / presentation. One edit, one undo step.
  - **Minimum size:** Settings → General → Editor → "Fit Text minimum size" (default 24 px). It applies to the size most of the box's text uses, so smaller words in the same box can go below it. If the text still doesn't fit there, the box is shrunk to the minimum and a message says to shorten the text or enlarge the box. Boxes that can't be measured (font not installed, damaged text) are left alone and counted in a message.

### E7 — Themes
- [x] A theme is a named set of:
  - canvas size
  - background (colour, or image plus adjustments)
  - text box position and size
  - full text style, outline, shadow, alignment
- [x] Create a theme from the current slide; edit a theme in the editor (theme editing mode on a sample slide)
- [x] Apply a theme to: a slide, a group, a presentation, or **many presentations** from the library (multi-select)
- [x] `Native` docs keep a live link: editing the theme restyles linked slides, after a confirmation listing how many slides and presentations change
- [x] `Pro6` docs: the theme applies as a copy of its styles (no link, no warning)
- [x] Applying a theme with a different canvas size reuses the rescale logic from `PLAN.md` M4
- **Accept:** apply a theme to 10 fixtures in one action; the output matches the theme; one undo per presentation.
- **Status (2026-11-21):** done. Acceptance test: `theme_applies_to_ten_presentations_with_one_undo_each` in `crates/core/tests/actor.rs` (the five fixtures twice, one `ApplyTheme` command; each matches the theme and has one undo step that restores it). `editing_a_theme_restyles_linked_slides_in_native_presentations` covers the live link.
  - **Model** (`doc::theme`): name, canvas size, background colour, background image (with adjustments), the main text box's place, and a full text style (the E5 `TextStyle`: font, size, colour, B/I/U, letter and line spacing, alignment, vertical alignment, outline, shadow). Files: `<first library folder>/Themes/<name>.optheme` (JSON). A background image path is stored relative to the theme file, and re-pointed relative to each presentation it's applied to.
  - **Applying:** every text box on a slide gets the style; the first also gets the theme's place. "This presentation" (and many presentations) with a different canvas size first rescales the presentation to the theme's canvas with the M4 rule (uniform fit, centred; sizes, outline and shadow by the same factor), reimplemented on the native model. Applying to some slides of a presentation with another canvas size scales the theme into its canvas instead.
  - **Themes window** (Theme… in the top bar): make a theme from this slide; pick a theme; apply to this slide / group / presentation (one undo step); apply to presentations ticked in a searchable library list (one `ApplyTheme` core command; the core loads each, and each gets one undo step and stays unsaved), then "Save all changed"; Edit theme; Delete theme (moved to `Themes/deleted`).
  - **Link:** native presentations remember the theme on each slide; `.pro6` ones get a copy. A theme whose features `.pro6` can't hold (background image, outline, shadow, spacing) says so in the window, and applying it makes a `.pro6` presentation native (and linked).
  - **Theme editing:** Edit theme opens the theme as a one-slide sample presentation in the editor ("Theme: name"; Done goes back). Saving writes the `.optheme`; then the library folders are searched (on a worker thread) for native presentations with slides linked to it, and a prompt says how many slides in how many presentations would change: Restyle them / Not now. Restyling applies the theme to the linked slides and saves each presentation, except one with other unsaved changes, which is restyled but left unsaved (with a message).
  - **Fixed on the way:** a `.opres` made by `pp6tool convert` was marked as a `.pro6`-mode document, so saving it in the app would have written `.pro6` XML into the `.opres`. `pp6tool convert` now marks it native, and a `.opres` is always saved as JSON.
  - **Not done:** the plan's "many presentations from the library (multi-select)" is a checklist in the Themes window, not multi-select in the operator's library list.

### E8 — Paste Lyrics to Slides
- [x] **Paste Lyrics** opens a full-height text view with a live slide preview beside it
- [x] A blank line starts a new slide. A line matching `[Name]` (e.g. `[Verse 1]`, `[Chorus]`, `[Ref]`) starts a new group with that name.
- [x] Repeated group names reuse the same group, so a second `[Chorus]` points at the existing Chorus. The arrangement follows the paste order.
- [x] Group colours: known names reuse their existing colour; defaults per name are configurable
- [x] Applies the chosen theme, or the current presentation's style
- [x] Works on a new presentation, or replaces or appends to the current one
- [x] Reverse direction: show an existing presentation as editable lyrics text with `[Group]` headers, re-flowed on apply
- **Accept:** pasting a song with verses, chorus and repeats produces the right groups, slides and arrangement, and round-trips through the text view unchanged.
- **Status (2026-11-21):** done. Acceptance test: `paste_a_song_and_round_trip_it_through_the_text_view` in `crates/doc/tests/lyrics.rs` (groups, slide counts, arrangement Verse 1 → Chorus → Verse 2 → Chorus → Bridge → Chorus; the text view shows exactly the pasted text; applying it unchanged changes nothing; it survives a `.pro6` save). `reflowing_an_existing_presentation_keeps_untouched_slides_byte_identical` checks that re-flowing the song fixture's own text gives a byte-identical `.pro6`.
  - **Window** (Paste Lyrics… in the top bar): opens with the presentation as text (the reverse direction: groups in the selected arrangement's order, `[Name]` lines, blank lines between slides, a repeat is just its `[Name]`). Text on the left, the resulting slides on the right, rendered by `render` a moment after typing stops, with the group count, slide count and order.
  - **Rules** (`doc::lyrics`): `[Name]` starts a group; a name used again is a repeat (case doesn't matter), and lines under a repeat are ignored if the group already has slides (the window counts them); a blank line starts a slide; text before any name goes in a group called "Lyrics"; a name with nothing under it anywhere is dropped.
  - **Replace:** groups that keep their name keep their slides: a slide whose text is unchanged is untouched (same UUID and `.pro6` XML), a changed one keeps its look and takes the new words, extra slides are new. Groups not in the text are removed (and from arrangements). The arrangement follows the text: the selected arrangement is updated, or a "Lyrics" arrangement is made when the order isn't simply each group once.
  - **Append:** new groups go after the existing ones; a name the presentation already has points at that group. The selected arrangement gets the order added.
  - **New presentation:** a `.opres` named in the window, in the first library folder, with the current presentation's first text slide as the style (raw `.pro6` elements left out); saved on a worker thread, then opened in the editor and added to the library.
  - **Style of new slides:** a copy of the presentation's first text slide, or a chosen theme (applied to the new slides only).
  - **Colours:** groups that keep their name keep their colour; new ones take the first default whose name their name starts with (Settings → General → Editor → Group colours for Paste Lyrics: Verse, Pre-Chorus, Chorus, Bridge, Tag, Intro, Outro, Ending; editable).
  - **One step:** Replace and Append are one `SetLyrics` edit, one undo step.
  - **Limits:** slides without text aren't in the text view, so Replace removes them (the window says how many). Only the first text box with text is the lyrics; a slide's other text boxes keep their text when re-flowed, and are emptied on new slides.

## Unknowns to Verify
- [ ] Every `Unverified` row in the Compatibility Matrix
- [ ] PP6 element type for images (foreground) and how slide backgrounds are stored. **Foreground: `RVImageElement`** (real files; `source` is a URL-encoded absolute path, `scaleBehavior` / `scaleSize` / `imageOffset` meanings unknown, only the defaults are imported). Backgrounds: real files use a full-canvas `RVImageElement`; no slide media cue seen yet. Writing a new `RVImageElement` and flips (`flippedHorizontally="1"`?) still need a PP6 check.
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
