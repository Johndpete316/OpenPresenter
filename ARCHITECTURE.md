# OpenPresenter architecture

How the program is put together: the design goals, the stack, the crates, the threads,
and what happens between pressing **Next** and the slide appearing on the projector.
For the plan, decisions and history see [`docs/PLAN.md`](docs/PLAN.md); for the `.pro6`
format see [`docs/format-notes.md`](docs/format-notes.md).

## Design goals

In priority order (from the plan):

1. **Speed for the operator.** Fast cold start (~150–200 ms), and a slide change that's a
   texture swap (under 1 ms from command to buffer swap, measured in
   [`docs/perf.md`](docs/perf.md)). Nothing on the show path waits on disk, network or
   decoding.
2. **Errors that don't stop the show.** Problems are plain sentences for the operator plus
   detailed logs. A fault in a side feature (phone remote, thumbnails, a bad file) is
   reported, and the output keeps running. Only a broken core invariant closes the
   program, with a crash report.
3. **`.pro6` fidelity.** ProPresenter 6 files open, render with the same fonts, sizes and
   canvas, and save back as `.pro6`. Anything not edited is written back byte for byte.

Three rules follow from these and shape most of the code:

- **One owner of show state.** A single core thread owns everything about the show; every
  change is a `Command` sent to it, and everyone else reads immutable snapshots.
- **The output never waits for the UI.** Rendering slides and presenting the output run on
  their own threads, fed from the core directly.
- **Unknown data is kept, never guessed.** The `.pro6` parser keeps every element and
  attribute it doesn't understand as raw XML and writes it back unchanged.

## Stack

| Area | What | Why |
|---|---|---|
| Language | Rust (edition 2024, stable) | Speed without a GC pause mid-service; `unsafe` is forbidden workspace-wide except the GL and mpv FFI modules |
| Windows and input | `winit` 0.30 | One event loop for the operator and output windows |
| OpenGL | `glutin` 0.32 + `glow` 0.17 (GL 3.3) | Output window with vsync on its own thread |
| Operator UI | `egui` 0.37 via `egui-winit` + `egui_glow` | Immediate-mode UI; `eframe` isn't used because it would own the event loop the output window needs |
| Text layout | `cosmic-text` + `fontdb` | Lays out RTF runs with the system's real fonts; missing fonts are errors, never silent substitutes |
| Video and images | libmpv (loaded at runtime with `libloading`) | Plays almost any format straight into the output's GL framebuffer; a missing DLL is a message, not a failed start |
| `.pro6` | `quick-xml`, `base64`, `encoding_rs`, a hand-written RTF subset | Byte-exact round trips |
| Phone remote | `axum` + `tokio` (single-threaded runtime) | HTTP + WebSocket on the LAN, page embedded in the binary |
| Threads and channels | `crossbeam-channel` | Bounded/unbounded channels and `select!` between workers |
| Logging and crashes | `tracing`, `tracing-appender`, `rfd` | Daily log files, in-memory rings for the UI and crash reports, native crash dialog |
| Storage | `serde` with TOML (settings, playlists) and JSON (`.opres`, library cache) | Human-readable files |
| Other | `keyring` (ESV token), `ureq` (ESV API), `trash` (delete to Recycle Bin) | |
| Packaging | Inno Setup, GitHub Actions (`windows-2025`), Azure Artifact Signing (pending) | See [`docs/release.md`](docs/release.md) |

## Crates

```
pro6 ──► doc ──► opcore ──► web
          │        │         │
          └► render◄─────────┘
                   │
media ─────────────┴──► app (openpresenter.exe)
pro6, doc, render ─────► pp6tool (pp6tool.exe)
```

| Crate | Owns | Depends on |
|---|---|---|
| `pro6` | The `.pro6` model, XML parser and writer, RTF reader/writer, raw-XML round-trip (`Extra`), slide timers in `cues` | — |
| `doc` | The **native model** used everywhere (`Document → Group → Slide → Element`), `.opres` files, `.pro6` ↔ native conversion, every edit (`Edit`), undo history, themes, lyrics parsing | `pro6` |
| `opcore` (`crates/core`) | `ShowState`, `Command`, the core actor, playlists, settings, library index, Bible references | `doc` |
| `render` | Slide → RGBA frame: text layout, effects, images, canvas → output placement. No window or GPU | `doc` |
| `media` | libmpv wrapper: players, render API into a GL framebuffer, audio devices | — |
| `web` | Phone remote: server, PIN auth, WebSocket pushes, slide images | `opcore`, `render` |
| `app` | The program: windows, event loop, operator UI, editor, output thread, workers | all of the above |
| `pp6tool` | CLI: library report, round-trip check, bulk rescale, render to PNG, convert | `pro6`, `doc`, `render` |

`pro6`, `doc`, `opcore` and `render` build and test with no window, GPU or mpv, which is
why most behaviour is covered by plain `cargo test`.

### Two document models

`pro6::Document` mirrors the file exactly. `doc::Document` is what the app works with.
Opening a `.pro6` converts it to the native model **and keeps the original
`pro6::Document` alongside it**. Saving a `.pro6` starts from that original, applies
only what changed (each native slide and element remembers its source by UUID), and
writes it out, so untouched slides and unknown data come back byte-identical. A change
a `.pro6` can't hold is caught before it's made (`doc::check` → `NeedsNative`) and
offered as a `.opres` save instead. Nothing converts silently.

## Threads

| Thread | Owns | Talks to |
|---|---|---|
| **main** | winit event loop, operator window + egui, input and clicker keys | sends `Command`s; reads snapshots; drives the output window |
| **core** | `ShowState` (the single owner), loop timer | receives `Command`s; publishes snapshots to every subscriber |
| **core-io** | file loads and saves for the core | core |
| **raster** | turns the live slide (and its neighbours) into output-sized frames | subscribes to core snapshots directly; sends frames to the output |
| **output-render** | the output window's GL context, slide textures, mpv players, vsync present | receives frames and media commands |
| **mpv-events** | libmpv's event callbacks (video ended, position) | posts to the output and UI |
| **web** + **web-state** | phone remote server; bridges snapshots into async | sends `Command`s; pushes state to phones |
| workers | library index, thumbnails, media thumbnails, media scan, editor canvas, exports, ESV, themes | report back to the UI through `UserEvent`s |

**Show-critical threads** are `main`, `core`, `raster` and `output-render`: a panic there
writes a crash report, shows a dialog and exits. A panic anywhere else writes the report,
shows a toast, and that thread stops while the show carries on (`crash::CRITICAL_THREADS`).
File loads and saves also catch panics per file, so one bad file can't stop file work.

## The main loop: from Next to the projector

```
 clicker / key / button / phone
            │  Command::Next
            ▼
 ┌──────────────────────┐   Arc<ShowState> snapshot   ┌──────────────┐
 │ core thread          │ ──────────────────────────► │ raster       │
 │  apply the command   │                             │  frame for   │
 │  to ShowState        │ ──► main (operator UI)      │  live slide  │
 │  publish a snapshot  │ ──► web (phones)            │  (cached)    │
 └──────────────────────┘                             └──────┬───────┘
                                                             │ RenderCmd::Show
                                                             ▼
                                                    ┌──────────────────┐
                                                    │ output-render    │
                                                    │  texture swap,   │
                                                    │  mpv background, │
                                                    │  vsync present   │
                                                    └──────────────────┘
```

1. **Input.** A key in either window (`App::handle_key` → `KeyBindings::command_for`), a
   toolbar or grid click (`UiAction::Core`), or a phone button (`web`) becomes a
   `Command`. Every source sends the same commands, recorded for crash reports.
2. **Core.** The core thread's `run` loop blocks on its channel, or until the loop timer's
   deadline when a timed presentation is running (so a busy UI can't delay an
   auto-advance). It applies the command to `ShowState`: moves the live slide (skipping
   disabled slides, crossing into the next playlist item), updates layers, starts or
   stops loops, queues loads and saves on `core-io`. Then it publishes: every subscriber
   gets an `Arc<ShowState>`, cheap to clone because the big parts (documents, playlists)
   are `Arc`s inside.
3. **Raster.** Subscribed to the core directly, not through the UI. It renders the live
   slide at the output's size with `render::render_slide` (text layer and slide colour
   kept separate so Clear Text / Clear Background are instant), keeps a small cache keyed
   by document, slide, size and stretch, and pre-renders the next and previous slides.
   A slide change is usually a cache hit.
4. **Output.** The output thread uploads the frame as a texture (often already uploaded
   by a preload) and draws: black → background media (mpv, into a framebuffer) → the
   slide's colour → the text. Then it presents with vsync and reports the latency back.
5. **Operator UI.** In parallel, the snapshot wakes the main thread (`UserEvent::Snapshot`),
   which redraws egui: preview, grid, playlists, panels. The UI never blocks on any of
   this; it only reads the latest snapshot and sends commands.

### The UI event loop

`App` implements winit's `ApplicationHandler`:

- `window_event`: input, resizes and redraws for the operator and output windows.
- `user_event`: everything workers report (`UserEvent`): new snapshot, thumbnails
  ready, library index done, media scan, audio devices, output presented or failed,
  notices.
- `about_to_wait`: timed jobs (delayed background switch, captures, benchmarks) set the
  next wake-up with `ControlFlow::WaitUntil`.

Each operator frame runs `ui::draw`, which draws the panels and returns a list of
`UiAction`s; `App` then applies them (send a `Command`, save settings, open a window).
Keeping the UI a function of `(UiState, ShowState, Settings)` → actions keeps drawing
free of side effects.

## Editing

The editor works on a **copy**: `ShowState.editing` holds the presentation being edited,
with undo/redo snapshots. The output and the grid keep the saved version until **Save**,
and a live presentation switches to the saved version at the next slide change, never by
itself. Grid right-click edits (quick text fixes, groups, timers) go through the same
`Edit` path and are saved straight away. Every `Edit` either applies completely or fails
and changes nothing.

## Storage

| What | Where | Format |
|---|---|---|
| Settings | `%APPDATA%\OpenPresenter\config\settings.toml` | TOML |
| Playlists | `%LOCALAPPDATA%\OpenPresenter\data\playlists\` (one file each; deletes go to `deleted\`) | TOML |
| Logs, crash reports | `%LOCALAPPDATA%\OpenPresenter\data\logs\` | text, daily rolling |
| Library index, thumbnails | `%LOCALAPPDATA%\OpenPresenter\cache\` | JSON, PNG |
| Presentations | the church's library folders | `.pro6` (XML), `.opres` (JSON), themes `.optheme` |
| ESV API token | Windows Credential Manager | — |

Saves are atomic (temp file, flush, rename) and keep the previous file as `.bak`. A
settings file that doesn't parse is copied aside before defaults are used.

## Errors and logging

- Each crate has typed errors (`thiserror`) with context (path, XML path, UUID) and a
  `user_message()`: one plain sentence and what happens next.
- Recoverable problems (bad file, missing font, missing media) show as a toast and in
  **Settings → Log**, and the program keeps running. Missing fonts are never substituted
  silently.
- `tracing` writes debug detail to daily log files, keeps the last 400 lines in memory
  for crash reports and recent info lines for the UI.
- The panic hook (`crash.rs`) writes the crash report: version, OS, backtrace, show
  state, recent commands, recent log lines.

## Phone remote

`web` runs axum on a single-threaded tokio runtime, bound to the PC's LAN address
(port 8686). A PIN shown on the operator screen logs a phone in (HttpOnly, SameSite=Strict
session cookie; wrong PINs are rate limited). The page is embedded in the binary. A bridge
thread turns core snapshots into two WebSocket messages, live state and playlists/slides,
sent only when they change. Slide images are rendered on the server with the same
renderer as the output. Phones send the same `Command`s as the operator.

## Testing

- `cargo test --workspace`: unit tests in every crate; integration tests drive the real
  core actor (`crates/core/tests/actor.rs`), real `.pro6` round trips
  (`crates/doc/tests`, `crates/pro6/tests`) and the real web server
  (`crates/web/tests/remote.rs`). Fixtures with a bundled font live in `fixtures/`.
- The app has development flags for checking the real thing: `--capture` (save the
  output's pixels), `--capture-ui` (screenshot the operator window), `--bench` (slide
  change latency), `--windowed-output`, `--open-timers` and similar.
- CI (GitHub Actions, Windows only) runs format, clippy with warnings as errors, and all
  tests. A pass on `main` builds the installer; a new version number publishes a release.
