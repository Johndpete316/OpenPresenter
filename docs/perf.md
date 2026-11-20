# Performance measurements

Record cold start and slide-change latency here whenever they change.

**How to measure:** `pp6 <file> --bench N [--windowed --window-size WxH] [--fonts DIR]` prints
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

**Not yet measured:** Windows (the production host), fullscreen on a second display, and real
`.pro6` files. The Windows numbers matter most. Fonts load differently there, and on a library
PC with hundreds of fonts installed, font loading may dominate cold start.
