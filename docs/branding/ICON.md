# OpenPresenter — App Icon
![[construction.png]]
## Concept
- **Screen** — rounded white frame (from concept D), open at the top-right corner.
- **Arrow** — orange advance chevron (from concept A) breaking out through the gap: "open" + "next slide".
- **Lyric lines** — two centred bars; the second at 55 % opacity reads as the upcoming line.
- **Stand** — orange neck + base; makes it read as a display, not a document.
- **Tile** — dark gradient squircle so it sits well on both light and dark Windows taskbars.
## Colours
| Token | Value | Use |
|---|---|---|
| `bg-top` | `#2A2A31` | Tile gradient start (top) |
| `bg-bottom` | `#121215` | Tile gradient end (bottom) |
| `tile-edge` | `#FFFFFF` @ 7 % | 3 px inner edge highlight |
| `accent-start` | `#FFA845` | Arrow + stand gradient (top-left) |
| `accent-end` | `#F06A1D` | Arrow + stand gradient (bottom-right) |
| `ink` | `#FFFFFF` | Screen frame, first lyric line |
| `ink-dim` | `#FFFFFF` @ 55 % | Second lyric line |
For flat UI use (no gradient), the accent midpoint is `#F88A31`.
## Geometry (512 × 512 artboard)
| Element | Spec |
|---|---|
| Tile | 480 × 480 at (16, 16), corner radius 108 |
| Content box | 352 × 352 centred (80 → 432) — all marks stay inside |
| Mark offset | whole mark group shifted +10 px down for optical centring |
| Screen | outer 316 × 244, stroke 36, corner radius 70, round caps; gap from top edge x = 314 to right edge y = 230 (cap ends, after the +10 offset) |
| Line 1 | 140 × 22, radius 11, centred on x = 256 |
| Line 2 | 100 × 22, radius 11, centred on x = 256, 22 px below line 1 |
| Stand neck | trapezoid 32 → 40 wide, tucked under the screen stroke |
| Stand base | 152 × 28, radius 14, centred on x = 256 |
| Arrow | 50 × 100 chevron, stroke 34, round caps + joins |
## Small-Size Variant (16–24 px)
`openpresenter-small.svg` is a simplified drawing used only for the 16 and 24 px exports:
- One lyric line instead of two.
- Strokes roughly 1.7× heavier (screen 60, arrow 56).
- Tile fills the artboard (no 16 px margin).
The full drawing is used from 32 px up.
## Arrow (UI Asset)
| File | Use |
|---|---|
| `arrow/arrow.svg` | Orange gradient — branding, splash, accents |
| `arrow/arrow-mono.svg` | `stroke="currentColor"` — buttons, toolbar, "Next" in the web remote; takes the surrounding text colour |
| `arrow/arrow-{16,24,32,64,128}.png` | Raster, height in px |
- ViewBox is 84 × 134, trimmed to the stroke edges — scale by height.
- Flip horizontally (`transform="scale(-1 1)"`) for a Previous/Back arrow.
## Files
```
docs/branding/
├─ ICON.md                       # this file
├─ construction.png              # keyline sheet
├─ svg/
│  ├─ openpresenter.svg            # master (32 px and up)
│  └─ openpresenter-small.svg      # 16–24 px variant
├─ png/
│  └─ openpresenter-{16,24,32,48,64,128,256,512,1024}.png
├─ windows/
│  ├─ openpresenter.ico            # 16, 24, 32, 48, 64, 128, 256 (16/24 from small variant)
│  ├─ wizard-small-55x58.bmp     # Inno Setup WizardSmallImageFile (100 %)
│  └─ wizard-small-110x116.bmp   # Inno Setup WizardSmallImageFile (200 %)
└─ arrow/
   ├─ arrow.svg
   ├─ arrow-mono.svg
   └─ arrow-{16,24,32,64,128}.png
```
## Usage (as wired in this repo)
`docs/branding/` is the source pack. The copies the build uses live next to the code:

| Where | File | How |
|---|---|---|
| `.exe` icon (Explorer, taskbar pin, Alt-Tab) | `crates/app/assets/openpresenter.ico` | `crates/app/build.rs` embeds it as resource 1 via `embed-resource` (`openpresenter.rc`); cross-builds need `llvm-rc` |
| Window icons on Windows | same resource | `with_icon` in `crates/app/src/main.rs` loads resource 1 at small and 64 px |
| Window icon on Linux | `crates/app/assets/openpresenter-64.png` | `include_bytes!` in `with_icon` |
| Settings → General header | `crates/app/assets/openpresenter-64.png` | drawn by `theme::brand_header` |
| Phone remote | `svg/openpresenter.svg`, `arrow/arrow-mono.svg` | inlined in `crates/web/src/page.rs` |
| Installer | `windows/openpresenter.ico`, `windows/wizard-small-*.bmp` | `packaging/openpresenter.iss`: `SetupIconFile`, `WizardSmallImageFile` |

## UI Pattern
New UI follows the icon: dark neutral ground (`bg-top`/`bg-bottom`), white ink, one orange
accent (`#F88A31` flat, or the `accent-start` → `accent-end` gradient on large marks) for
what's live / what advances, rounded corners, heavy round-capped strokes. The colours are
in `crates/app/src/theme.rs` (`theme::brand`) and as CSS variables in the phone remote.

## Rules
- Don't recolour the tile or change the white/orange split.
- Don't place the mark without the tile below 64 px — it relies on the dark ground for contrast.
- Keep the arrow pointing right in the logo; flipped arrows are UI-only.
- Regenerate rasters from the SVGs; never upscale a PNG.
