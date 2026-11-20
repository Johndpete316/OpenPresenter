# Windows host: setup and acceptance checks

Everything below was built and checked on Linux. These are the steps that still need the Windows
show PC (M2, M3 and M5 acceptance).

## Build the installer

On the Linux dev machine (one-time: `wine`, `cargo-xwin`, and Inno Setup 6 installed into a
Wine prefix, e.g. `wine innosetup-6.7.2.exe /VERYSILENT /CURRENTUSER "/DIR=C:\inno"`;
put the shinchiro `libmpv-2.dll` in `packaging/vendor/`, which is gitignored):

```sh
packaging/build-installer.sh
# → target/installer/PP6-Setup-<version>-<commit>.exe  (≈40 MB, sha256 printed)
```

Commit first: a build with uncommitted changes is stamped `-dirty`. Bump `version` in the
root `Cargo.toml` for each release that goes to the church. The version and commit show in
**Settings → General**, at the top of the log, in crash reports and in `pp6.exe --version`.

For a quick test build without the installer:
`cargo xwin build --release --target x86_64-pc-windows-msvc -p app -p pp6tool`.

## Install on the show PC

1. Copy `PP6-Setup-….exe` over (Google Drive works; Gmail blocks `.exe` attachments) and run
   it. It installs `pp6.exe`, `pp6tool.exe` and `libmpv-2.dll` to `C:\Program Files\PP6`,
   adds a Start-menu shortcut (a desktop one is optional), and registers in *Installed apps*.
   **Updating = running a newer installer**: it replaces the old version in place and offers
   to close pp6 if it's running. Uninstalling or upgrading never touches settings, playlists
   or logs. Silent install: `PP6-Setup-….exe /VERYSILENT /SUPPRESSMSGBOXES`.
2. The installer isn't code-signed yet. With SmartScreen: *More info → Run anyway*. With
   **Smart App Control on, unsigned apps can't run at all** (see Code signing in PLAN.md).
3. Install every font the slides use. Find them with:
   `pp6tool.exe report "D:\ProPresenter\Library"` (see the FONTS section).
4. GPU driver with OpenGL 3.3.

Data locations: settings `%APPDATA%\PP6\config\settings.toml`; logs, crash reports and
playlists under `%LOCALAPPDATA%\PP6\data\`; caches under `%LOCALAPPDATA%\PP6\cache\`.
(Exact folders are printed on startup in the log.) `--data-dir DIR` puts everything in one
folder instead. In the app, messages and the recent log are under **Settings → Log**; new
problems show a toast in the bottom-right corner and a warning badge in the toolbar.

## M2: output on a second display

```bat
pp6.exe --open "C:\path\to\song.pro6" --monitor 1
```

- The output opens borderless fullscreen on display 1. The toolbar's Display list shows the
  numbering.
- Try the arrow keys and PageUp/PageDown in either window; toggle **Output** in the toolbar
  (off must release the display; on re-opens it). **Display** picks the screen.
- Timing: `pp6.exe --open song.pro6 --monitor 1 --bench 30` prints cold start and slide-change
  latency. Add the numbers to `docs/perf.md` (Windows row).
- Pixel check: `pp6.exe --open song.pro6 --monitor 1 --capture out.png`, then compare with
  `pp6tool.exe render song.pro6 --out pngs --size <display size>`. They should be identical.
- Side-by-side with PP6: same slide, same display. Compare font, size and position. This answers
  the `\fs` → pixel and vertical-alignment unknowns.

## M3: run a service with a clicker

1. Settings → Folders → add the library folder. The library fills in the background.
2. Playlists: click **+** on the Playlists header (type a name, Enter). Make two (e.g. AM and
   PM). Drag songs from the library onto a playlist (or double-click / +: it goes into the
   playlist you last clicked). Drag items to reorder, or onto the other playlist to move them.
   Right-click a playlist to rename or delete it. Everything saves automatically (one file per
   playlist under the data folder's `playlists`; Settings → Folders shows where).
   Next past a song's last slide goes to the next song **in the same playlist**; Prev on the
   first slide goes to the previous song's last slide.
3. Close the operator window focus, click into the output (or leave the operator focused) and
   use the clicker: PageDown/PageUp (or arrows) step through slides, skipping disabled ones; the
   clicker's blank button (`.` or `b`) is Clear All. F1/F2/F3 = Clear All / Background / Text.
4. Switch arrangement mid-song from the grid's Arrangement list: the slide on screen must stay.
5. Relaunch: it starts blank (nothing live), by request.
6. Key bindings live in `settings.toml` under `[keys]` (e.g. `next = ["PageDown", "Right"]`).
   Restart to apply.

## M5: looping video under text, audio routing

1. Settings → Folders (or the folder button on the Video/Image panel) → add your media
   **root** folder. It's scanned with all its sub-folders; the left of the Video/Image panel
   shows the same folder tree (with file counts), and clicking a folder shows its files,
   grouped by sub-folder. Nothing is copied, moved or written. Hidden and system folders
   are skipped; an unreadable folder is reported.
2. Click a video with sound in the Video/Image panel: it plays looping under the slide text, inside the slide
   area (black bars outside).
3. Settings → Audio: pick the output device; the video's sound should move to it.
4. Clear Background (F2) removes the video; Clear Text (F3) leaves it playing.
5. Click another media file: the one after the current file is pre-loaded, so it switches instantly.

## M6: phone remote

1. Start `pp6.exe`. Click **Remote** in the toolbar: it shows the address
   (http://192.168.x.y:8686) and the PIN.
2. **Windows Firewall** asks on first start whether PP6 may use the network. Allow it on
   **private** networks. If you missed the prompt: Windows Security → Firewall → Allow an app →
   PP6 (private), or open TCP port 8686.
3. On a phone on the same Wi-Fi, open that address and enter the PIN. Next / Prev / Clear
   control the slides (Clear = Clear All). The page shows the current and next slide text and
   updates live.
4. Enter a wrong PIN: rejected. Five wrong PINs lock that phone out for 30 s, then longer.
5. **New PIN** (under Remote) logs every phone out.
6. Settings: `[web]` in `settings.toml` (`enabled`, `port`, `bind`, `pin_length` 4–6). The server
   listens on the PC's LAN address only (not all interfaces) unless `bind` says otherwise.

## M7: edited files open in PP6

Work on **copies** of real presentations.

1. Open a song, click **Editor** in the toolbar.
2. Change a slide's text (Apply), add a slide, duplicate one, move one to another group,
   rename and recolour a group, add a group, and build a new arrangement.
3. **Save** (the previous version is kept as `<name>.pro6.bak`).
4. Open the saved file in **PP6 on Windows** and check:
   - the new text shows (not the old text: this verifies dropping WinFlowData/WinFontData);
   - fonts and sizes match the neighbouring slides;
   - group names and colours are right, and the new arrangement appears and plays in order.
5. Also run `pp6tool roundtrip` on the folder: saved files must parse and be stable.

## M8: Welcome / announcements loop

1. Install **Bookman Old Style** (all three sample files use it).
2. Put `AA001 Welcome.pro6` live (click its first slide). The loop starts on its own, as in
   PP6, and the Preview pane shows **LOOP RUNNING · next slide in N s**.
3. Check the timing against PP6: 10 s per slide (one slide 7 s), the disabled slide skipped, and
   after the last slide it goes back to the first.
4. Next/Prev during the loop: step, and the countdown restarts for that slide.
5. Clear (any): the loop stops and stays stopped (Next won't restart it); **Start loop** resumes.
6. Select a song in the library: the loop keeps running. Put a song live: the loop stops.

## Known issues on the Linux dev machine (not the app)

`mpv` and `ffmpeg` there currently fail to start (`libsndio.so.7` / `libjxl.so.0.12` missing
after a partial upgrade). `sudo pacman -Syu` should fix it. Media was tested with those two
libraries unpacked from the local pacman cache into a scratch folder via `LD_LIBRARY_PATH`.
Nothing on the system was changed.
