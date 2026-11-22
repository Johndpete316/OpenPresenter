<p align="center"><img src="docs/branding/png/openpresenter-128.png" width="96" alt="OpenPresenter"></p>

# OpenPresenter

Worship presentation software for Windows: an operator window, a full-screen output
window, a slide editor and a phone remote. It reads and writes ProPresenter 6 (`.pro6`)
presentations, and has its own format (`.opres`, themes `.optheme`) for things a `.pro6`
file can't hold.

- **Plan and status:** [`docs/PLAN.md`](docs/PLAN.md), editor: [`docs/EDITOR-PLAN.md`](docs/EDITOR-PLAN.md)
- **Building the installer and setting up the show PC:** [`docs/windows-setup.md`](docs/windows-setup.md); releases and code signing (CI): [`docs/release.md`](docs/release.md)
- **Branding (icon, colours, UI pattern):** [`docs/branding/ICON.md`](docs/branding/ICON.md)
- **Licence:** [`docs/LICENSING.md`](docs/LICENSING.md)

## Layout

| Crate | What |
|---|---|
| `crates/app` | the program (`openpresenter.exe`): operator UI, output window, editor |
| `crates/core` (`opcore`) | show state, commands, playlists, settings, library |
| `crates/doc` | native presentation model, `.opres`, `.pro6` conversion, edits, undo |
| `crates/pro6` | `.pro6` parser and writer |
| `crates/render` | slide rendering (no window/GPU dependencies) |
| `crates/media` | libmpv wrapper |
| `crates/web` | phone remote server |
| `crates/pp6tool` | `.pro6` conversion and diagnostics CLI (`pp6tool.exe`) |

```sh
cargo run -p app -- --help          # the program
cargo run -p pp6tool -- --help      # the .pro6 tool
packaging/build-installer.sh        # Windows installer (see docs/windows-setup.md)
```

## Licence

Copyright (C) 2026 John Peterson.

OpenPresenter is free software: you can redistribute it and/or modify it under the terms
of the GNU General Public License version 3, as published by the Free Software
Foundation. It is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See [`LICENSE`](LICENSE) for the full text and
[`docs/LICENSING.md`](docs/LICENSING.md) for what that means in practice.

"ProPresenter" is a trademark of Renewed Vision. OpenPresenter is not affiliated with or
endorsed by Renewed Vision; it only reads and writes the `.pro6` file format.
