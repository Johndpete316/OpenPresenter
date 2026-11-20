# Windows host: setup and acceptance checks

Everything below was built and checked on Linux. These are the steps that still need the Windows
show PC (M2, M3 and M5 acceptance).

## Build

On the Linux dev machine:

```sh
cargo xwin build --release --target x86_64-pc-windows-msvc -p app -p pp6tool
# → target/x86_64-pc-windows-msvc/release/pp6.exe and pp6tool.exe
```

Or take the `pp6-windows` artifact from CI.

## Install on the show PC

1. Copy `pp6.exe` (and `pp6tool.exe`) into a folder, e.g. `C:\PP6\`.
2. **For backgrounds/video:** download a libmpv build (e.g. shinchiro *mpv-dev-x86_64* from
   mpv-winbuild-cmake), and put **`libmpv-2.dll` or `mpv-2.dll`** next to `pp6.exe`. The app loads it at runtime. Without
   it, slides still work and the operator sees "Video and image backgrounds are unavailable…".
   No `mpv.lib` is needed (nothing links against mpv at build time).
3. Install every font the slides use. Find them with:
   `pp6tool.exe report "D:\ProPresenter\Library"` (see the FONTS section).
4. GPU driver with OpenGL 3.3.

Data locations: settings `%APPDATA%\PP6\config\settings.toml`; logs, crash reports and the
restore file under `%LOCALAPPDATA%\PP6\data\`; caches under `%LOCALAPPDATA%\PP6\cache\`.
(Exact folders are printed on startup in the log.) `--data-dir DIR` puts everything in one
folder instead.

## M2: output on a second display

```bat
pp6.exe --open "C:\path\to\song.pro6" --monitor 1
```

- The output opens borderless fullscreen on display 1. The toolbar's Display list shows the
  numbering.
- Try the arrow keys and PageUp/PageDown in either window; toggle **Output ON/OFF** in the
  toolbar (OFF must release the display; ON re-opens it).
- Timing: `pp6.exe --open song.pro6 --monitor 1 --bench 30` prints cold start and slide-change
  latency. Add the numbers to `docs/perf.md` (Windows row).
- Pixel check: `pp6.exe --open song.pro6 --monitor 1 --capture out.png`, then compare with
  `pp6tool.exe render song.pro6 --out pngs --size <display size>`. They should be identical.
- Side-by-side with PP6: same slide, same display. Compare font, size and position. This answers
  the `\fs` → pixel and vertical-alignment unknowns.

## M3: run a service with a clicker

1. Settings → Folders → add the library folder. The library fills in the background.
2. Build a playlist with `+`, then **Save As…**.
3. Close the operator window focus, click into the output (or leave the operator focused) and
   use the clicker: PageDown/PageUp (or arrows) step through slides, skipping disabled ones; the
   clicker's blank button (`.` or `b`) is Clear All. F1/F2/F3 = Clear All / Background / Text.
4. Switch arrangement mid-song from the grid's Arrangement list: the slide on screen must stay.
5. Kill the process mid-service (Task Manager) and relaunch: it should come back on the same
   slide (if within 6 h).
6. Key bindings live in `settings.toml` under `[keys]` (e.g. `next = ["PageDown", "Right"]`).
   Restart to apply.

## M5: looping video under text, audio routing

1. Settings → Folders → add a media folder with a video that has sound.
2. Click the video in the Media panel: it plays looping under the slide text, inside the slide
   area (black bars outside).
3. Settings → Audio: pick the output device; the video's sound should move to it.
4. Clear Background (F2) removes the video; Clear Text (F3) leaves it playing.
5. Click another media file: the one after the current file is pre-loaded, so it switches instantly.

## Known issues on the Linux dev machine (not the app)

`mpv` and `ffmpeg` there currently fail to start (`libsndio.so.7` / `libjxl.so.0.12` missing
after a partial upgrade). `sudo pacman -Syu` should fix it. Media was tested with those two
libraries unpacked from the local pacman cache into a scratch folder via `LD_LIBRARY_PATH`.
Nothing on the system was changed.
