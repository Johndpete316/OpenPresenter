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
│  ├─ core/              # ShowState, Command, playlist model, settings
│  ├─ render/            # slide → GPU: text layout (cosmic-text), canvas scaling, compositing
│  ├─ media/             # libmpv wrapper, audio device list
│  ├─ web/               # axum server, PIN auth, WebSocket, static page
│  ├─ app/               # egui operator UI + output window, wires everything together
│  └─ pp6tool/           # CLI: corpus report, round-trip check, bulk rescale, render-to-PNG
├─ fixtures/             # hand-written .pro6 files from the spec
└─ docs/
```
`pro6` and `core` must build and test without any GUI, GPU, or mpv dependency. Crates are added in the milestone that first needs them.

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
- [ ] Cargo workspace skeleton, CI (Linux test + Windows build), `rustfmt`/`clippy` config
- [ ] `pro6` data model from the spec; every node keeps unknown attributes/children as raw XML (`extra`) for round-tripping
- [ ] Parser with `quick-xml`; base64 decode of payloads
- [ ] RTF reader: extract font table, font face per run, `\fs` size (half-points), colour, plain text. Evaluate `rtf-parser`; hand-roll the subset if it doesn't fit
- [ ] Read canvas size from the document root (attribute names to confirm — see Unknowns)
- [ ] Read `RVDisplaySlide.enabled` into the model
- [ ] Parse `RVTimeline` attributes (`duration`, `loop`, `timeOffset`, `selectedMediaTrackIndex`); keep `timeCues`/`mediaTracks` as raw `extra` until M8
- [ ] Hand-written fixtures in `fixtures/` covering: multiple groups, multiple slides per group, multi-line text, two fonts, non-default canvas size (1024×768), a disabled slide
- [ ] `pp6tool report <dir>`: per file parse OK/fail with error; counts of canvas sizes; fonts used; group names; element/attribute names seen including unknowns; files with media cues
- **Accept:** all fixtures parse; report runs on `fixtures/`; `cargo test -p pro6` green with no GUI deps.

### M1 — Writer + Round-Trip
- [ ] Writer that emits unchanged nodes and `extra` data exactly as parsed
- [ ] `pp6tool roundtrip <dir>`: load → save → semantic compare; report diffs
- [ ] On text edit: regenerate `PlainText` + `RTFData`; drop `WinFlowData`/`WinFontData` for that element (behind a flag until verified in PP6)
- [ ] Preserve group and slide UUIDs exactly
- **Accept:** fixtures round-trip with zero semantic diff.

### M2 — Rendering + Output Spike (Windows-Critical)
- [ ] `render`: lay out a slide onto its own canvas size with `cosmic-text` using RTF face/size; positions from `RVRect3D` in canvas pixels
- [ ] Canvas → output scaling: uniform fit, centred, black bars. Never stretch
- [ ] `pp6tool render <file> --out dir/` → PNG per slide (for visual diff against PP6)
- [ ] Output window: `winit` + `glutin` + `glow`, borderless fullscreen on a chosen monitor, own thread, vsync
- [ ] Output disable = destroy window and release the monitor; re-enable recreates it
- [ ] Missing-font detection → typed error
- [ ] Measure cold start and slide-change latency; log them
- **Accept:** fixture slides render correctly to PNG and to a second monitor on Windows.

### M3 — Core + Operator MVP
- [ ] `core`: `ShowState`, `Command`, broadcast channel, crash-restore state file
- [ ] Logging + panic hook + crash report wired in
- [ ] Library index cache (path, title, mtime, search tokens); background revalidation; full parse only on open
- [ ] egui operator UI per layout: toolbar, Preview, Library selector/search, Playlist, Presentation grid, Media bin
- [ ] Presentation grid shows slides under group headers with PP6 group name + colour; arrangement order when one is selected
- [ ] Arrangement picker in operator view: lists the presentation's arrangements, defaults to `selectedArrangementID`, switchable live without leaving the current slide
- [ ] Disabled slides shown greyed out in the grid; skipped by `Next`/`Prev` from every source (UI, clicker, web); still clickable directly
- [ ] Separate clear layers: Clear Text, Clear Background, Clear All as toolbar buttons with their own configurable key bindings
- [ ] Background thumbnail generation + cache
- [ ] Clicker / keyboard: configurable bindings (default PageDown/PageUp/arrows/Clear), work whether operator or output window has focus
- [ ] Native playlists: simple file of presentation paths + arrangement ID; pre-parse the next item
- **Accept:** can run a service from fixtures on Windows with a clicker.

### M4 — Bulk Resolution Tool
- [ ] `pp6tool rescale --to 1024x768 <files|dir>`
- [ ] Scales: document canvas size, element `position`, RTF `\fs`, shadow offset/blur, stroke width
- [ ] Same aspect → proportional scale. Aspect change → uniform fit + centre; `--stretch` opt-in only
- [ ] `--dry-run` default; per-file report old → new; writes to `--out` dir or backs up originals before in-place write
- [ ] `--preview` renders before/after PNGs
- **Accept:** rescaled fixtures render correctly; round-trip clean.

### M5 — Media + Settings
- [ ] `media`: libmpv render API into output FBO; text composited above
- [ ] Layers: background (colour/image/video) → slide text; each layer clearable independently (`ClearText` / `ClearBackground` / `ClearAll`)
- [ ] Media bin: backgrounds and videos; pre-decode next
- [ ] Settings: output monitor select / disable; audio device from mpv `audio-device-list`
- **Accept:** looping video background under slide text on Windows; audio routed to chosen device.

### M6 — Web Remote
- [ ] axum on its own tokio thread, bound to LAN interface
- [ ] PIN: 4–6 digits generated at launch, shown in UI, regenerate button; correct PIN → session cookie; rate-limit failures
- [ ] Static mobile page: current + next slide text; Next / Prev / Clear buttons → same `Command`s (Clear → `ClearAll`)
- [ ] WebSocket push on state change
- [ ] Doc note: Windows Firewall prompt on first run
- **Accept:** phone on LAN controls slides; wrong PIN rejected.

### M7 — Editor
- [ ] Edit slide text, add/remove/reorder slides
- [ ] Create/rename/recolour/reorder groups; move slides between groups
- [ ] Edit arrangements
- [ ] Save to `.pro6`
- [ ] Later: element layout, font, colour, position
- **Accept:** edited file opens correctly in PP6.

### M8 — Announcements: Media Cues + Looping / Auto-Advance
Only the Announcements presentation uses these.
- [ ] Reverse-engineer slide cues, `timeCues`, and media tracks from the real Announcements file
- [ ] Find where per-slide timing lives (timeline cues vs slide cues) and model it
- [ ] Auto-advance: core-owned timer advances slides per the stored timing; skips disabled slides
- [ ] Loop: honour `RVTimeline.loop` — wrap from last enabled slide to first
- [ ] Operator controls: Start/Stop loop button + indicator showing loop is running and time to next slide
- [ ] Timer runs on the core actor, not the UI thread; a UI stall must not delay an advance
- [ ] Until supported: load the presentation with text and a non-fatal "media cues / auto-advance not supported yet" warning
- **Accept:** Announcements file loops on its own with the same timing as PP6.

## Unknowns to Verify
Track findings here as real files arrive.
- [ ] Canvas size attribute names on `RVPresentationDocument`
- [ ] Actual canvas sizes in the library (expected mostly 1024×768)
- [ ] RTF `\fs` → canvas pixel conversion matches PP6 (side-by-side screenshot diff)
- [ ] Arrangement structure; whether arrangements reference groups by UUID
- [ ] Whether PP6 on Windows uses stale `WinFlowData`/`WinFontData` over `RTFData` after edits
- [ ] `RVRect3D` field order (`{x y z w h}` assumed)
- [ ] Shadow string field meanings
- [ ] Media cue / timeline structure (Announcements only)
- [ ] Where per-slide auto-advance durations are stored
- [ ] Behaviour when the operator presses Next/Prev/Clear or picks another presentation while a loop is running (stop loop? keep timing?) — **ask John**
- [ ] Arrangement element structure and how `selectedArrangementID` maps to it

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
