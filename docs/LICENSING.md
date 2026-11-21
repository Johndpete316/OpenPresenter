# Licensing

OpenPresenter is licensed under the **GNU General Public License, version 3 only**
(SPDX: `GPL-3.0-only`). The full text is in [`LICENSE`](../LICENSE) at the repository root;
the installer shows it on its licence page and installs it as
`C:\Program Files\OpenPresenter\LICENSE.txt`. Inside the program it's under
**Settings → General → GNU General Public License v3.0**.

Copyright (C) 2026 John Peterson.

## What the GPL means here

| You want to… | Allowed? | Conditions |
|---|---|---|
| Install and run it (church PCs, any number) | Yes | None. The GPL doesn't restrict use. |
| Change it for your own use | Yes | None, as long as you don't give the changed version to others. |
| Give the installer to another church or person | Yes | Give them the **source code** too (see below), keep the licence and copyright notices, and don't add restrictions. |
| Give them a changed version | Yes | As above, plus mark it as changed (with a date), and your changes are GPL-3.0 too. |
| Sell it | Yes | Same conditions; the buyer gets the same rights. |
| Fold it into a closed-source program | **No** | Anything that includes OpenPresenter code and is distributed must be GPL-3.0. |

### Providing the source

Whenever an installer or `.exe` is given to someone outside the church, the matching
source must be available to them (GPL section 6). Any one of these works:

- Include a zip of the repository at the commit the installer was built from. The commit
  is in the installer's file name (`OpenPresenter-Setup-<version>-<commit>.exe`):
  `git archive --format=zip -o OpenPresenter-src-<commit>.zip <commit>`.
- Publish the repository (e.g. GitHub) and tell them where the matching commit is.
- A written offer, valid for three years, to provide the source on request.

The installer also contains `libmpv-2.dll` (GPL, a third-party build linked from
`packaging/THIRD-PARTY.txt`), so its source must be available the same way. Pointing to the
upstream release only counts while it stays online (GPL-3.0 section 6(d)); the safe option
is to keep a copy of the matching mpv-winbuild-cmake source next to each release you give
out.

Installing it on the church's own computers isn't distribution to a third party, so
nothing extra is needed for that.

## Third-party components

All bundled components have GPL-3.0-compatible licences. They're listed in
[`packaging/THIRD-PARTY.txt`](../packaging/THIRD-PARTY.txt), which the installer installs
next to the program:

- **libmpv** (video/media): GPL v2 or later (parts LGPL), which is compatible with GPL-3.0.
- **Phosphor Icons** (toolbar icons): MIT.
- **egui default fonts**: Ubuntu Font Licence, OFL 1.1, MIT.
- **Rust crates** from crates.io: MIT, Apache-2.0, BSD, ISC, Zlib, BSL-1.0, MPL-2.0,
  Unicode-3.0, Unlicense. Apache-2.0 and MPL-2.0 are compatible with GPL-3.0 (not with
  GPL-2.0-only, which is one reason the project is GPL-3.0).

Before adding a dependency, check its licence:

```sh
cargo metadata --format-version 1 | python3 -c 'import json,sys
for p in json.load(sys.stdin)["packages"]: print(p["license"], p["name"])' | sort | uniq
```

Not allowed: proprietary or "non-commercial" licences, SSPL, and GPL-2.0-only code with no
"or later" option.

## The branding

The icon and arrow in `docs/branding/` are original work, part of this project, and under
the same GPL-3.0 licence.

"ProPresenter" is a trademark of Renewed Vision. OpenPresenter is an independent project,
not affiliated with or endorsed by Renewed Vision. The name is only used to say which
file format (`.pro6`) the program reads and writes; keep it that way in the UI and docs.

## Notices in the program

GPL section 0 asks an interactive program to show a copyright notice, that there's no
warranty, and how to see the licence. That's the header at the top of **Settings →
General** (`theme::brand_header`). Keep it when changing that screen.

## Changing the licence

The licence is set in the root `Cargo.toml` (`[workspace.package] license`) and every crate
inherits it. To allow later GPL versions as well, change it to `GPL-3.0-or-later` and say so
in the Settings notice and in the README. Code contributed under GPL-3.0-only can't be
relicensed without its authors' agreement.
