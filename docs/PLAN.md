# ProPresenter 6 Rewrite — Implementation Plan
> [!info] For Claude Code
> Read this whole file before starting. Work one milestone at a time, in order. Each task is a small, reviewable change. Do not start the next milestone until the current one's acceptance criteria pass. When something in the `.pro6` format is unknown, **do not guess silently** — add it to [[#Unknowns to Verify]] and preserve the raw data.

## Goal
Replacement for ProPresenter 6 for church use. Reads and writes existing `.pro6` files (backward compatible with PP6). Dev machine is Linux (CachyOS); production host is **Windows**.

## Priorities (in order)
1. **Speed** — fast cold start, snappy input. The slide operator is the primary user. Show-critical paths (operator UI, output render, clicker input) never block on disk, network, or decoding.
2. **Error handling** — human-readable errors, extensive backend logs, quick and clean crash with a report on unrecoverable faults.
3. **`.pro6` fidelity** — font, font size, and resolution must render the same as PP6. Slide groups must be preserved.

## Known Facts About the Content
- Slides are almost all black background, white text.
- Output display and most slides are believed to be 1024×768 (unverified — read from files, never hard-code).
- Only one presentation (Announcements) uses media cues.
- Playlists are rebuilt weekly — **no PP6 playlist import**. Native playlist format only.
- Slide groups (Verse 1, Verse 2, Chorus, Ref, …) and their colours/names must be kept.
- No real `.pro6` files are available yet. Build from the spec + hand-written fixtures; real-file corpus testing comes later.

## Format Reference
Spec: https://gist.github.com/gooderist/ae3054f25390b3befabcc623b0c6c184 (unofficial, reverse-engineered, partial). Project observations: [format-notes.md](format-notes.md).
- XML, UTF-8, base64-encoded string payloads.
- Hierarchy: `RVPresentationDocument` → `RVTimeline`, `groups[RVSlideGrouping]`, `arrangements[?]` → `RVSlideGrouping.slides[RVDisplaySlide]` → `displayElements[RVTextElement]`.
- `RVSlideGrouping`: `name`, `color`, `uuid`.
- `RVDisplaySlide`: `backgroundColor`, `highlightColor`, `enabled`, `hotKey`, `label`, `notes`, `UUID`, `chordChartPath`, `cues[?]`.
- `RVTextElement`: `displayName`, `UUID`, `opacity`, `fillColor`, `locked`, `rotation`, `bezelRadius`, `verticalAlignment`, `revealType`, `position` (`RVRect3D`, e.g. `{56 42 0 1808 996}`), `shadow` (pipe-delimited), stroke `dictionary`.
- Text payloads (base64 NSString): `PlainText`, `RTFData` (**primary**), `WinFlowData`, `WinFontData` (Windows-only, unconfirmed; slides reportedly render without them).
- Undocumented: arrangements, slide cues, timeline cues, media tracks, many attribute meanings.

## Architecture
### Threads
| Thread | Owns | Notes |
|---|---|---|
| Core actor | `ShowState` (live presentation, slide index, output layers, playlist) | Single owner; all mutation via `Command` |
| Operator UI | egui main window | Sends `Command`, renders from broadcast state |
| Output render | Output window, GL context, vsync | Independent of UI frame timing |
| Media | libmpv instance(s) | Renders into output GL FBO |
| Web | tokio runtime + axum | Phone remote |
| I/O worker | Library index, parsing, thumbnails | Never blocks UI |

### Command Bus
- One `Command` enum: `Next`, `Prev`, `ClearText`, `ClearBackground`, `ClearAll`, `GoTo { presentation, slide }`, `SelectArrangement(uuid)`, `SetBackground(..)`, `OutputEnable(bool)`, `StartLoop`, `StopLoop`, …
- `Next`/`Prev` skip slides where `enabled` is false.
- Operator UI, clicker keys, and web remote all send the same commands. State changes broadcast to all consumers.

### Workspace Layout
```
pp6/
├─ Cargo.toml            # workspace
├─ crates/
│  ├─ pro6/              # .pro6 model, parser, writer, RTF handling (no GUI deps)
│  ├─ doc/               # native model used app-wide, .opres, .pro6 conversion, edits, undo
│  ├─ core/              # ShowState, Command, playlist model, settings
│  ├─ render/            # slide → GPU: text layout (cosmic-text), canvas scaling, compositing
│  ├─ media/             # libmpv wrapper, audio device list
│  ├─ web/               # axum server, PIN auth, WebSocket, static page
│  ├─ app/               # egui operator UI + output window, wires everything together
│  └─ pp6tool/           # CLI: corpus report, round-trip check, bulk rescale, render-to-PNG
├─ fixtures/             # hand-written .pro6 files from the spec
└─ docs/
```
`pro6`, `doc` and `core` must build and test without any GUI, GPU, or mpv dependency. Crates are added in the milestone that first needs them.

## Error Handling Rules
- `thiserror` typed errors per crate (`ParseError`, `WriteError`, `RenderError`, `MediaError`, `OutputError`, `WebError`). Every error carries context: file path, XML element path, UUID where relevant.
- Each error type implements a `user_message()` → one plain sentence + what happens next. Example: *"Slide uses font 'Futura PT', which isn't installed on this PC."*
- **Recoverable** (bad file, missing font, missing media): show in UI, log, keep running.
- **Unrecoverable** (core state invariant broken, GL context lost and can't be recreated): panic.
- Panic hook: write crash report (version, OS, backtrace, last N log lines, `ShowState` snapshot, recent commands) to the logs dir, show native dialog (`rfd`) with summary + report path, exit immediately.
- Logging: `tracing` with a span per command and per file load; rolling daily files via `tracing-appender`; debug to file, info to UI.
- Persist current playlist + slide position so a relaunch restores where the operator was.
- **Missing fonts never silently fall back.**

## Milestones
### M0 — Workspace + `.pro6` Parser + Corpus Tool
- [x] Cargo workspace skeleton, CI (Linux test + Windows build), `rustfmt`/`clippy` config
- [x] `pro6` data model from the spec; every node keeps unknown attributes/children as raw XML (`extra`) for round-tripping
- [x] Parser with `quick-xml`; base64 decode of payloads
- [x] RTF reader: extract font table, font face per run, `\fs` size (half-points), colour, plain text. Evaluate `rtf-parser`; hand-roll the subset if it doesn't fit
- [x] Read canvas size from the document root (attribute names to confirm — see Unknowns)
- [x] Read `RVDisplaySlide.enabled` into the model
- [x] Parse `RVTimeline` attributes (`duration`, `loop`, `timeOffset`, `selectedMediaTrackIndex`); keep `timeCues`/`mediaTracks` as raw `extra` until M8
- [x] Hand-written fixtures in `fixtures/` covering: multiple groups, multiple slides per group, multi-line text, two fonts, non-default canvas size (1024×768), a disabled slide
- [x] `pp6tool report <dir>`: per file parse OK/fail with error; counts of canvas sizes; fonts used; group names; element/attribute names seen including unknowns; files with media cues
- **Accept:** all fixtures parse; report runs on `fixtures/`; `cargo test -p pro6` green with no GUI deps.
- **Status (2026-11-19):** done. RTF: `rtf-parser` rejected, subset hand-rolled (see [format-notes.md](format-notes.md)). `pp6tool.exe` cross-builds with `cargo xwin`.

### M1 — Writer + Round-Trip
- [x] Writer that emits unchanged nodes and `extra` data exactly as parsed
- [x] `pp6tool roundtrip <dir>`: load → save → semantic compare; report diffs
- [x] On text edit: regenerate `PlainText` + `RTFData`; drop `WinFlowData`/`WinFontData` for that element (behind a flag until verified in PP6)
- [x] Preserve group and slide UUIDs exactly
- **Accept:** fixtures round-trip with zero semantic diff.
- **Status (2026-11-20):** done. All parseable fixtures round-trip **byte-identical** (`pp6tool roundtrip fixtures/`). The Win payload drop is `WinPayloads::{Keep, Drop}`, a required argument with no default; M7 picks the default once PP6 behaviour is verified. Still to do: open `pp6tool roundtrip --out` copies and `set_text` output in real PP6.

### M2 — Rendering + Output Spike (Windows-Critical)
- [x] `render`: lay out a slide onto its own canvas size with `cosmic-text` using RTF face/size; positions from `RVRect3D` in canvas pixels
- [x] Canvas → output scaling: uniform fit, centred, black bars. Never stretch
- [x] `pp6tool render <file> --out dir/` → PNG per slide (for visual diff against PP6)
- [x] Output window: `winit` + `glutin` + `glow`, borderless fullscreen on a chosen monitor, own thread, vsync
- [x] Output disable = destroy window and release the monitor; re-enable recreates it
- [x] Missing-font detection → typed error
- [x] Measure cold start and slide-change latency; log them
- **Accept:** fixture slides render correctly to PNG and to a second monitor on Windows.
- **Status (2026-11-20):** everything is built and verified on Linux. PNGs are checked visually. The GL output, captured with `pp6 --capture`, matches `pp6tool render` pixel-for-pixel at 4:3, 16:9 and 1:1, including black bars and background. Disable/enable and the missing-display error were exercised. `pp6.exe` cross-builds. **Still to do for acceptance:** run on the Windows host with a second display. Suggested commands: `pp6.exe --list-monitors`, then `pp6.exe <file> --monitor N`; arrow keys and D, stdin `e` to re-enable; `--bench 30`; `--capture out.png`. Record the numbers in perf.md.
- **Windows (2026-11-20):** fixture and real slides render to PNG on Windows (Bookman Old Style hymn checked), and the output runs **fullscreen on a real second display** (DISPLAY2 1024×768) next to the operator window. Round-trip is byte-identical on Windows. Windows-only bugs found and fixed: operator window crash ("Unable to create Texture object": creating the output's WGL context left the operator context not current); startup stall from loading libmpv during startup (6.5 s → 1.3–2.2 s). **Still to do:** a visual comparison against PP6 (not installed on the test laptop).
- **Several playlists (John, 2026-11-20):** the team runs two a week, so the Playlists panel is a tree: each playlist collapsible with its items under it. Playlists save automatically (one TOML file each in `<data>/playlists`; deleting moves the file to `deleted/`); Import copies a file in. Next/Prev cross items only inside the live presentation's playlist. Library double-click / + adds to the playlist last used.
- **Clear is one-shot (John, 2026-11-20):** Clear All / Slides / BkGrds act once and show no lasting "cleared" state. Clear Background stops the background media (choose one to bring it back); Clear Text takes the slide off until a slide is shown. The toolbar no longer highlights them and the preview has no "All cleared" label.
- **Start blank (John, 2026-11-20):** launching no longer resumes the previous slide/playlist position (the crash-restore file isn't read or written). Replaces the M3 "relaunch on the same slide" check.
- **Blank slides (John, 2026-11-20):** PP6 stores "Double-click to edit" in a new, empty text box and doesn't display it; John's blank slides rely on that. That exact text (`pro6::PLACEHOLDER_TEXT`, trimmed) is not drawn, not sent to the phone remote, not indexed for search, and edits as empty. Other-language PP6 builds may use a different placeholder (unverified).
- **Prev across playlist items (John, 2026-11-20):** Prev on the first slide goes to the previous playlist item's last enabled slide (mirror of Next). Crossing only happens when the live presentation is in the playlist.
- **Media folders follow the disk (John, 2026-11-20):** a media folder is scanned recursively on a background thread (hidden/system folders skipped, symlinks not followed, depth 24, 50,000-file cap reported). The Video/Image panel shows the folder tree with counts; a folder shows its own files then each sub-folder's under a heading. Rescanned when folders are added or removed (restart to pick up new files).
- **Operator layout (John, 2026-11-20):** structured like PP6, which the team knows: toolbar with round icons (Clear All / Slides / BkGrds, Editor, Video/Image; Output, Display, Remote, Settings), left column Preview → Library (search) → Playlist, right side slide grid over the Video/Image bin. The log moved into Settings → Log (problems show a toast and a toolbar badge). Grid tiles carry the group colour bar and "N. [Group]" label like PP6, with a tile-size slider. Icons: Phosphor (MIT), bundled in `crates/app/assets/`.
- **M3 architecture note:** winit allows one event loop per process, and `eframe` takes it. The output window must share the operator UI's event loop, so M3 should use `egui-winit` + `egui_glow` directly instead of `eframe`. The output's GL still runs on its own thread. glow is pinned to 0.17 to match `egui_glow` 0.37. **Confirm with John before M3.**

### M3 — Core + Operator MVP
- [x] `core`: `ShowState`, `Command`, broadcast channel, crash-restore state file
- [x] Logging + panic hook + crash report wired in
- [x] Library index cache (path, title, mtime, search tokens); background revalidation; full parse only on open
- [x] egui operator UI per layout: toolbar, Preview, Library selector/search, Playlist, Presentation grid, Media bin
- [x] Presentation grid shows slides under group headers with PP6 group name + colour; arrangement order when one is selected
- [x] Arrangement picker in operator view: lists the presentation's arrangements, defaults to `selectedArrangementID`, switchable live without leaving the current slide
- [x] Disabled slides shown greyed out in the grid; skipped by `Next`/`Prev` from every source (UI, clicker, web); still clickable directly
- [x] Separate clear layers: Clear Text, Clear Background, Clear All as toolbar buttons with their own configurable key bindings
- [x] Background thumbnail generation + cache
- [x] Clicker / keyboard: configurable bindings (default PageDown/PageUp/arrows/Clear), work whether operator or output window has focus
- [x] Native playlists: simple file of presentation paths + arrangement ID; pre-parse the next item
- **Accept:** can run a service from fixtures on Windows with a clicker.
- **Status (2026-11-20):** everything is built; core behaviour is covered by tests, and the UI was checked with `--capture-ui` screenshots on Linux. **Still to do for acceptance:** a Windows run with a real clicker. Notes:
  - Operator UI uses `egui-winit` + `egui_glow` (not `eframe`), as agreed.
  - Core crate is `crates/core`, package `pp6core` (`core` clashes with Rust's built-in crate).
  - The library index cache is JSON, not bincode: `bincode` 3.0.0 is a deprecation release.
  - Key bindings are configured in `settings.toml` (`[keys]`); there's no in-app editor.
  - Next at the last slide moves to the first enabled slide of the **next playlist item**, using its stored arrangement (decided by John, 2026-11-20). It loads first if needed, without blocking. Prev at the first slide stays put. The raster worker and the "Next" preview look ahead into the next item, so the change is instant.
  - Triggering a slide brings back cleared text, but a cleared background stays cleared.
  - Relaunch restores the live slide only if the restore file is < 6 h old.
  - Data folders: settings in the per-user config dir; logs, restore file and caches in the per-user data and cache dirs (`directories` crate; `--data-dir` overrides).

### M4 — Bulk Resolution Tool
- [x] `pp6tool rescale --to 1024x768 <files|dir>`
- [x] Scales: document canvas size, element `position`, RTF `\fs`, shadow offset/blur, stroke width
- [x] Same aspect → proportional scale. Aspect change → uniform fit + centre; `--stretch` opt-in only
- [x] `--dry-run` default; per-file report old → new; writes to `--out` dir or backs up originals before in-place write
- [x] `--preview` renders before/after PNGs
- **Accept:** rescaled fixtures render correctly; round-trip clean.
- **Status (2026-11-20): done.** Tests show a rescaled document renders like the original fitted to the same output (bbox within 2 px, >99.5% identical pixels) and writes back identically. Notes:
  - Beyond `\fs`, the RTF's absolute paragraph and kerning sizes (`\sl \sa \sb \li \ri \fi \tx \expnd \expndtw \up \dn`) are also scaled, edited in place. Leaving them unscaled would distort the layout. **Review with John.**
  - `--stretch` scales text and effects by the smaller axis factor (fonts can't stretch non-uniformly).
  - WinFlowData/WinFontData of rescaled text are dropped by default (`--keep-win-payloads` keeps them); same unknown as M1.

### M5 — Media + Settings
- [x] `media`: libmpv render API into output FBO; text composited above
- [x] Layers: background (colour/image/video) → slide text; each layer clearable independently (`ClearText` / `ClearBackground` / `ClearAll`)
- [x] Media bin: backgrounds and videos; pre-decode next
- [x] Settings: output monitor select / disable; audio device from mpv `audio-device-list`
- **Accept:** looping video background under slide text on Windows; audio routed to chosen device.
- **Status (2026-11-20):** built and verified on Linux. Output captures show an image and a looping video under the text, inside the canvas area with black bars, right side up. Looping is confirmed past the clip end, the pre-loaded switch is instant, and the audio device list loads. **Still to do for acceptance:** a Windows run with `mpv-2.dll`, plus a check of audio routing by ear (see docs/windows-setup.md). Notes:
  - libmpv is loaded **at runtime** (`libloading`) instead of the `libmpv2` crate. A missing DLL becomes an operator message rather than a failed start, and building for Windows needs no `mpv.lib`.
  - Images go through mpv too (`image-display-duration=inf`), so there is one code path.
  - Background media fits inside the canvas area (mpv keeps aspect); fill/stretch options aren't implemented.
  - The operator Preview pane shows the slide text only, not the background media.
  - CI doesn't bundle `mpv-2.dll` (it isn't vendored); add it next to the exe when installing.

### M6 — Web Remote
- [x] axum on its own tokio thread, bound to LAN interface
- [x] PIN: 4–6 digits generated at launch, shown in UI, regenerate button; correct PIN → session cookie; rate-limit failures
- [x] Static mobile page: current + next slide text; Next / Prev / Clear buttons → same `Command`s (Clear → `ClearAll`)
- [x] WebSocket push on state change
- [x] Doc note: Windows Firewall prompt on first run
- **Accept:** phone on LAN controls slides; wrong PIN rejected.
- **Status (2026-11-20):** built and tested on Linux with a real server, HTTP and WebSocket against the real core. Login, wrong PIN (401), lockout (429 + Retry-After), commands, push, logout on a new PIN and unauthenticated WebSocket rejection all pass. The page was checked at phone size in headless Chrome. **Still to do for acceptance:** a phone on the church LAN (see docs/windows-setup.md). Notes:
  - The page is embedded in the binary (no `tower-http` / static files to deploy).
  - Binds to the PC's LAN IPv4 (auto-detected) unless `[web] bind` is set; port 8686.
  - PIN 4 digits by default (4–6 configurable). Five wrong PINs lock that IP out for 30 s, doubling up to 10 min. A new PIN logs out every phone. Sessions are HttpOnly + SameSite=Strict cookies (also blocks cross-site command posts); there's no HTTPS (LAN only).
- **Slide previews and playlist browsing (John, 2026-11-21):** after first use, John asked for real slide previews and playlist navigation, with the phone kept simple and the desktop showing more.
  - Slides are drawn on the server by the output's renderer (`render_slide`), as PNGs with a transparent background, on the remote's own font copy. Image URLs are unique per loaded document (an edit makes a new URL), so browsers cache them for good. Until fonts are loaded, images answer 503 and the page retries. A slide that can't be drawn (e.g. missing font) shows its text instead.
  - The background media shows as a poster frame (`media_thumbs::poster`, cached on disk) under the slide image; a playing video isn't streamed. Clear Text / Clear Background hide the layers. A slide's own background colour is part of the slide image, so it hides with Clear Text (minor difference from the output).
  - One page, two layouts. Phone: current and next previews, Prev / Next / Clear (side by side when held sideways). Wide screen (≥ 900 × 500): playlists | slide grid | current/next, plus Clear Text, Clear Background and Output on/off, and arrow / Space / Page Up / Page Down keys (a clicker on a laptop works).
  - Browsing never touches the operator's screen: it happens in the page. Clicking a grid tile sends `GoTo`; an unloaded presentation is loaded with the new `Command::Preload`, which doesn't change `selected`. Clicking a tile goes live straight away, like the operator grid. Phones have no grid, so a stray tap while scrolling can't put a slide up.
  - Two WebSocket messages: live/next state (every change) and playlists + slides (only when playlists, loaded documents or arrangements change). Presentations are named by path hashes; paths never reach the browser, and the server accepts only presentations in a playlist or live.
  - `cargo run -p web --example remote_demo [port] [background-image]` serves the page with the fixtures, for trying it in a browser.

### M7 — Editor
- [x] Edit slide text, add/remove/reorder slides
- [x] Create/rename/recolour/reorder groups; move slides between groups
- [x] Edit arrangements
- [x] Save to `.pro6`
- [ ] Later: element layout, font, colour, position
- **Accept:** edited file opens correctly in PP6.
- **Status (2026-11-20):** built. Every operation is tested through a full write → re-parse round trip (unique UUIDs, arrangements follow group renames, moves and removals; failed edits change nothing). Core tests cover edits while live, save with backup, and revert. Edit mode was checked with `--capture-ui`. **Still to do for acceptance:** open edited files in PP6 (see docs/windows-setup.md, M7). Notes:
  - Edit mode lives in the grid ("Edit"); clicking selects, never goes live. Slides show in natural group order.
  - New slides copy the layout, font and size of a neighbouring slide (fresh UUIDs throughout, including cues). A presentation with no slides can't get a new one (no invented fonts).
  - Text edits use the first run's style for the whole box (mixed fonts in one box become one font). They drop WinFlowData/WinFontData; that's the first thing to verify in PP6.
  - Arrangement editing writes the *assumed* `RVSongArrangement`/`groupIDs` structure.
  - Save keeps the previous file as `<name>.pro6.bak` (one rolling backup) and saves atomically. Revert reloads from disk. Quitting with unsaved edits asks Save / Don't save / Cancel. There's no undo yet, and unsaved edits aren't in the crash-restore file.
  - If the live slide is edited, the output updates; if it's deleted, the screen keeps it until the next trigger.
  - **Superseded by the editor redesign (John, 2026-11-21; docs/EDITOR-PLAN.md E0):**
    - Edits go to an editor copy, with undo/redo.
    - The output changes only after Save, at the next slide change.
    - Revert discards the copy.
    - All presentations are now held in the native `doc` model; `.pro6` is converted on load and written back byte-identical where nothing changed.

### M8 — Announcements: Media Cues + Looping / Auto-Advance
Only the Announcements presentation uses these.
- [x] Reverse-engineer slide cues, `timeCues`, and media tracks from the real Announcements file
- [x] Find where per-slide timing lives (timeline cues vs slide cues) and model it
- [x] Auto-advance: core-owned timer advances slides per the stored timing; skips disabled slides
- [x] Loop: **verified mechanism is `loopToBeginning="true"` on the last slide's timer** (not `RVTimeline.loop`, which is `false` in the real file). After that slide it wraps to the first enabled slide.
- [x] Operator controls: Start/Stop loop button + indicator showing loop is running and time to next slide
- [x] Timer runs on the core actor, not the UI thread; a UI stall must not delay an advance
- [x] While looping (decided by John): Next/Prev step as usual and restart the timer for the slide they land on; any Clear (Text/Background/All) clears and **stops the loop**
- [x] While looping (decided by John): selecting/opening another presentation does **not** stop the loop
- [x] Loop-running indicator **near the Preview pane** (John), with time to the next slide
- [x] Putting a *different* presentation live **stops the loop** (John: no pausing)
- [x] Until supported: load the presentation with text and a non-fatal "media cues / auto-advance not supported yet" warning
- **Accept:** Announcements file loops on its own with the same timing as PP6.
- **Status (2026-11-20):** built against the **verified** format from `AA001 Welcome.pro6`. Each slide's `RVSlideTimerCue` gives its time, and `loopToBeginning` on the last slide loops back. The timer runs on the core thread. Like PP6, putting a timed presentation live starts it. John's rules apply: Next/Prev restart the timer; any Clear stops it (it stays stopped until Start loop or the presentation is put live again); selecting elsewhere keeps it; another presentation live stops it. Without a loop-back slide it stops at the end. The UI shows LOOP RUNNING + countdown + Stop near Preview. Timing tests run in real time. **Still to do for acceptance:** run the Welcome file on the Windows PC next to PP6 and compare timing (needs Bookman Old Style installed). **Windows run (2026-11-20):** the real Welcome file auto-started on Windows and advanced every 10.0 s. Note: at launch the first slide's countdown starts when it goes live, about 1 s before it's on screen. **Confirmed by John (2026-11-20):** auto-start when a timed presentation goes live, and it stays stopped after Clear until Start loop (or put live again).

## Unknowns to Verify
Track findings here as real files arrive. "M0:" notes say how the parser currently handles each one. None are verified against a real file yet.
- [x] Canvas size attribute names on `RVPresentationDocument`. **Verified (real files):** `width`/`height`.
- [x] Actual canvas sizes in the library. **Real files: all 1024×768** (3 of 3 so far).
- [ ] RTF `\fs` → canvas pixel conversion matches PP6 (side-by-side screenshot diff)
- [x] Arrangement structure. **Verified:** `<RVSongArrangement name uuid color><array rvXMLIvarName="groupIDs"><NSString>group-uuid</NSString>…`, referencing groups by UUID.
- [ ] Element `rotation` direction and pivot. Drawn clockwise about the box centre since the editor redesign (E1); compare a rotated box in PP6.
- [ ] Whether PP6 on Windows uses stale `WinFlowData`/`WinFontData` over `RTFData` after edits
- [x] `RVRect3D` field order. **Verified:** `{x y z w h}` (`{20 20 0 984 728}` = 20 px margin on 1024×768).
- [ ] Shadow string field meanings. M0: assumed `blur|r g b a|{x, y}`; raw fields always kept; mismatch → warning.
- [ ] Media cue / timeline structure. The Welcome file has only slide timers, no media cues and empty `timeCues`/`mediaTracks`; a file with real media cues is still needed.
- [x] Where per-slide auto-advance durations are stored. **Verified:** `RVSlideTimerCue` in each slide's `cues` (`duration` s, `actionType="9"` "Go to Next", `loopToBeginning="true"` on the last slide); the timeline is empty.
- [x] Behaviour when the operator presses Next/Prev/Clear while a loop is running. **John:** Next/Prev do what they normally do and restart the timer for the slide they land on. Clear clears the current slide and stops the loop. Selecting another presentation does **not** stop the loop (John). Putting a different presentation live stops the loop (John).
- [x] Arrangement element structure and `selectedArrangementID`. **Verified:** it holds the `uuid` of one `RVSongArrangement`.
- [ ] XML syntax for named children: `<array rvXMLIvarName="…">`, `<NSString rvXMLIvarName="…">`, `rvXMLDictionaryKey` (assumed, not shown in spec). M0: matched on element name + ivar; mismatch → kept raw + warning.
- [ ] Base64 payload line wrapping/whitespace. M0: whitespace stripped before decode; original text kept.
- [ ] PP6 RTF flavour on Windows (Cocoa `\cocoartf` vs Windows-style font table), and whether text effects (`\outl`, `\strokewidth`, `\shad`) appear. M0: both flavours parse; report lists unhandled control words.
- [ ] Whether a trailing `\par` means an empty last line in PP6. M0: ignored.
- [ ] Per-font `\fcharset` code pages. M0: only document `\ansicpg` is applied.
- [ ] Whether PP6 (Windows) opens and renders RTF written by `rtf::write` (Windows-style font table, `\pard\qX` per paragraph, `\uN?`). Check with an edited file in PP6.
- [ ] PlainText encoding (assumed UTF-8) and newline style PP6 writes. M1: `set_text` keeps whatever style the element already used, default `\n`.
- [ ] Whether a Cocoa-flavoured element should be rewritten in Cocoa style after an edit. M1: always Windows style.
- [ ] `verticalAlignment` values. **0 = middle verified** (2026-11-20: every element in the real files is 0, and John reports PP6 centres them vertically; my first guess of 0 = top was wrong). 1 = top, 2 = bottom are still assumed: needs a file with a top/bottom-aligned box saved from PP6.
- [ ] Text colour when RTF has no `\cf` ("auto"). M2: black, per RTF spec.
- [ ] Line height. M2: the font's ascent + descent + line gap at the run size. Compare with PP6 on multi-line slides.
- [ ] RTF font names: family names vs PostScript names (e.g. `ArialMT`, `Helvetica-Bold`). M2 resolves both.
- [ ] Text overflow beyond the element box (`adjustsHeightToFit`, shrink-to-fit?). M2: draws past the box (clipped to canvas) with a warning.
- [ ] `drawingBackgroundColor` semantics (slide vs document). M2: slide colour if the slide's flag is true, else the document's if its flag is true, else black.
- [ ] Shadow, stroke (outline), rotation, underline, and fill corner radius: not drawn in M2 (each produces a render warning).
- [ ] How PP6 layers slide background colour vs background media, and its media scaling (fit/fill). M5: media under an optional slide colour; media fits the canvas area.
- [ ] Does PP6 accept files edited here: new slides/groups (fresh UUIDs, no Win payloads), regenerated RTF, and arrangements written in the assumed structure?
- [x] Arrangement XML structure. **Verified** (see above); the M3/M7 assumption was right.
- [x] Next/Prev at the end of a presentation. **John:** Next moves to the next playlist item (implemented). Prev at the first slide stays put; not specified, so not changed.
- [ ] Characters missing from the slide's font: cosmic-text draws them from another font. M2 reports a warning; decide whether that is acceptable or should be an error.

### Found in real files (2026-11-20, `pp6-examples/`: Welcome, two hymns)
- [ ] **`WinFlowData` is a WPF `FlowDocument`** (XAML: paragraphs, runs, `FontFamily`, `FontSize`, `Foreground`), and **`WinFontData` is an `RVFont` XML** (kerning, line spacing, outline colour). PP6 for Windows may render from these rather than RTF, so dropping them on edit (M1/M7) **must be checked in PP6**. Alternative: regenerate `WinFlowData` from the edited text.
- [x] `\fs` → size: `\fs108` in RTF = `FontSize="54"` in `WinFlowData`, which supports half-points ÷ 2 = canvas px. Still confirm visually.
- [ ] RTF is Windows-flavoured (`\prortf1`, `\htmautsp`, `\loch\hich\dbch`, `\ltrch`) with paragraph sizes (`\li \fi \sa \sb`, all 0 so far) and **text outlines** (`\outl \strokewidth \strokec`, 87 runs); outlines aren't drawn yet.
- [x] `\rquote` etc. were dropped; now decoded.
- [ ] `RVSlideTimerCue@delayTime` (0 in all samples): meaning unknown; reported if non-zero, not applied.
- [ ] `RVImageElement` (images on slides; `source` is a URL-encoded Windows path) and `RVTransition` (document transition): kept as raw XML, not drawn or used yet.
- [ ] Shadow `n|r g b a|{x, y}`: first field was 0 with an offset of {4.95, -4.95}; probably blur radius, unverified.
- [ ] Group colours are written with 15 decimals (`0.498039215686275`); preserved exactly.

## Deployment requirements (found on Windows, 2026-11-20)
- [ ] **Code signing.** Windows 11 **Smart App Control** blocks unsigned `pp6.exe`/`pp6tool.exe` (and would block `libmpv-2.dll`); a Defender exclusion doesn't help. Before deploying to the church PC: check whether Smart App Control is on there, and plan to sign the executables and DLL with a trusted code-signing certificate (e.g. Microsoft Trusted Signing or a commercial OV/EV certificate; self-signed won't satisfy Smart App Control).
  - Decision (2026-11-20): ship unsigned for now. Signing must happen before the church PC if Smart App Control is on there (it can't be bypassed per app). Recommended: Azure Artifact Signing (formerly Trusted Signing), ideally in the church's name; the installer, `pp6.exe`, `pp6tool.exe` and `libmpv-2.dll` all get signed.
- [x] **Installer.** Inno Setup (built under Wine by `packaging/build-installer.sh`), per-machine install to `C:\Program Files\PP6`. Updates are a new installer sent over (Google Drive), not an auto-updater; same AppId = in-place upgrade. User data stays in AppData. Version + commit shown in Settings → General.

## Required Tooling
### Dev Machine (CachyOS / Arch)
```fish
# Rust
sudo pacman -S rustup
rustup default stable
rustup component add rustfmt clippy
rustup target add x86_64-pc-windows-msvc
# Windows cross-compile (MSVC target, downloads Windows SDK/CRT on first use)
sudo pacman -S clang lld llvm
cargo install cargo-xwin
# Native libs for winit/glutin/egui on Linux
sudo pacman -S libxkbcommon wayland libx11 libxcursor libxrandr libxi mesa
# libmpv for media crate
sudo pacman -S mpv
# Handy
cargo install cargo-nextest cargo-watch
# Installer (Inno Setup 6 runs under Wine; see docs/windows-setup.md)
sudo pacman -S wine
```
Build for Windows: `cargo xwin build --release --target x86_64-pc-windows-msvc`

### Windows Host (Runtime)
- `mpv-2.dll` (libmpv) shipped next to the `.exe` (e.g. from shinchiro mpv-winbuild-cmake `mpv-dev-x86_64` builds)
- `mpv.lib` import library needed at link time for the MSVC target — keep in `vendor/mpv/` and point the linker at it
- All fonts used by slides installed on the host (report from `pp6tool report` lists them)
- GPU driver with OpenGL 3.3+
- Windows Firewall rule for the web remote port

### CI
- GitHub Actions: `ubuntu-latest` → `cargo fmt --check`, `clippy -D warnings`, `nextest` on `pro6`/`core`/`web`/`pp6tool`
- `windows-latest` → full release build, upload `.exe` + `mpv-2.dll` artifact

### Crates
| Purpose | Crate |
|---|---|
| XML | `quick-xml` |
| Base64 | `base64` |
| RTF | `rtf-parser` (evaluate) or hand-rolled subset |
| UUID | `uuid` |
| Serialization (settings, playlists, index cache) | `serde`, `toml` / `ron`, `bincode` |
| Errors | `thiserror` (libs), `anyhow` (CLI only) |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` |
| Channels | `crossbeam-channel` |
| Operator UI | `egui`, `eframe` |
| Output window / GL | `winit`, `glutin`, `glutin-winit`, `glow` |
| Text shaping | `cosmic-text` |
| Images / PNG | `image` |
| Video / audio | `libmpv2` |
| Native dialogs | `rfd` |
| Web | `axum`, `tokio`, `tower-http` (static files) |
| CLI | `clap` |
| Tests | `cargo-nextest`, `insta` (snapshot tests for parser/report output) |

Use latest stable versions; confirm API compatibility between `egui`/`winit`/`glutin` versions before pinning.

## Working Rules for Claude Code
- One task per change; small commits with clear messages.
- Never hard-code canvas size, output resolution, or fonts.
- Never drop unknown `.pro6` data.
- Don't add features not listed here; ask instead.
- Keep `pro6` and `core` free of GUI/GPU/mpv deps.
- Every new error path gets a `user_message()` and a log line with context.
- Record each speed measurement (cold start, slide change) in `docs/perf.md` when it changes.
