# Performance measurements

Record cold start and slide-change latency here whenever they change.

**How to measure:** `openpresenter <file> --bench N [--windowed --window-size WxH] [--fonts DIR]` prints
cold start and slide-change latency statistics. `-v` adds per-slide raster times to the log.

- **Cold start** = process start (`main`) → first slide frame presented (buffer swap returned).
  It does not include OS process creation or loading the executable.
- **Slide change latency** = command received on the event-loop thread → buffer swap returned on
  the render thread, for a pre-rendered (cached) frame. On drivers where swap doesn't block for
  vsync (NVIDIA on Wayland here), this is "frame submitted", not "light on screen". The display adds
  up to one refresh interval (16.7 ms at 60 Hz) plus compositor latency.
- **Raster** = CPU layout + rasterisation of one slide at output size (raster worker, off the
  show-critical threads; neighbours are pre-rendered so this is normally hidden).

## M2 baseline (2026-11-20)

Machine: Linux (CachyOS, kernel 7.3), Wayland, NVIDIA RTX 3080, driver 620.70.06, OpenGL 3.3 core,
1001 system font faces. Release build. Fixture `render_1024x768.pro6` (4 slides, one with a missing
font) with `--fonts fixtures/fonts`. **Windowed** (not fullscreen), 3 runs of `--bench 30
--bench-interval-ms 100` at 1920×1080.

| Date | Build | Machine | Metric | Value | Notes |
|---|---|---|---|---|---|
| 2026-11-20 | M2 release | Linux / RTX 3080 / Wayland | Cold start | 110–139 ms | process start → first slide presented; system font load 9–13 ms of that |
| 2026-11-20 | M2 release | Linux / RTX 3080 / Wayland | Slide change (cached) | median 0.46–0.49 ms, p95 0.54–2.19 ms, max 2.1–10.9 ms | command → buffer swap; see caveat above |
| 2026-11-20 | M2 release | Linux / RTX 3080 / Wayland | Raster 1920×1080 slide | mean 7.2–10.3 ms, max 9.2–18.8 ms | hidden by neighbour pre-rendering |
| 2026-11-20 | M2 release | Linux / RTX 3080 / Wayland | Font loading | 9–13 ms | 1001 faces, fontconfig cache warm |

## M3 (2026-11-20): through the core, with the operator UI

Same machine and fixture; `openpresenter --open … --windowed-output --bench 30 --bench-interval-ms 100`,
3 runs. Commands now go key/UI → core actor → raster worker (subscribed to the core directly) →
output render thread. The operator window (egui) runs on the event-loop thread at the same time.

| Date | Build | Machine | Metric | Value | Notes |
|---|---|---|---|---|---|
| 2026-11-20 | M3 release | Linux / RTX 3080 / Wayland | Cold start | 147–169 ms | operator window + output window + core + font load |
| 2026-11-20 | M3 release | Linux / RTX 3080 / Wayland | Slide change (cached) | median 0.59–0.68 ms, p95 0.72–0.78 ms, max 0.73–0.84 ms | command sent → buffer swap |

## M5 (2026-11-20): with a looping video background

Same machine; 640×480 H.264 test video looping under the text (`--background`), 30 changes at
150 ms intervals, 3 runs. The first background trigger creates the two mpv players (≈170 ms of
GL setup). In normal use that happens when the output is idle after startup; with
`--background` at startup, the first change waits for it.

| Date | Build | Machine | Metric | Value | Notes |
|---|---|---|---|---|---|
| 2026-11-20 | M5 release | Linux / RTX 3080 / Wayland | Cold start, video background | 181–207 ms | first slide is sent before media, so mpv setup doesn't delay it |
| 2026-11-20 | M5 release | Linux / RTX 3080 / Wayland | Slide change, video playing | median 1.1–1.3 ms; most ≤ 3 ms; some 15–17 ms while playback starts (one vsync wait) | first change after startup ≈ 80 ms while players are created |
| 2026-11-20 | M5 release | Linux / RTX 3080 / Wayland | mpv player setup | ≈ 110 ms first, ≈ 60 ms second | `mpv_render_context_create`; mpv_create itself ≈ 2 ms |
| 2026-11-20 | M5 release | Linux / RTX 3080 / Wayland | Slide change, no media | median 0.70 ms, max 0.85 ms | |

## Windows (2026-11-20): first runs on Windows

Machine: Windows 11 (build 26200) laptop, Intel Arc 140V, OpenGL 3.3 (driver 32.0.101.6737), 361
font faces. Output **fullscreen on a real second display** (DISPLAY2, 1024×768), operator window on
the 2880×1920 main display. Release build cross-compiled with cargo-xwin. Fixture
`song_1024x768.pro6` (Arial), `--bench 30 --bench-interval-ms 150`.

| Date | Build | Machine | Metric | Value | Notes |
|---|---|---|---|---|---|
| 2026-11-20 | first Windows build | Win11 / Arc 140V | Cold start | **6.5 s** | libmpv (121 MB) loaded at startup held the Windows loader lock; fonts 3.9 s |
| 2026-11-20 | libmpv deferred | Win11 / Arc 140V | Cold start | **1.3–2.2 s** | fonts 0.48–0.68 s; libmpv now loads after the first slide (≈0.2 s) |
| 2026-11-20 | libmpv deferred | Win11 / Arc 140V | Slide change (cached) | median 0.43–0.54 ms, p95 1.5 ms, max 1.8 ms (one 70 ms outlier while libmpv loaded just after startup) | command → buffer swap, fullscreen output |
| 2026-11-20 | libmpv deferred | Win11 / Arc 140V | Welcome loop | advances every 10.0 s (33.37 → 43.37) | real `AA001 Welcome.pro6`, auto-started |

**Not yet measured:** Windows (the production host), fullscreen on a second display, and real
`.pro6` files. The Windows numbers matter most. Fonts load differently there, and on a library
PC with hundreds of fonts installed, font loading may dominate cold start.
