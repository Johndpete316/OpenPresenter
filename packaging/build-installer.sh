#!/usr/bin/env bash
# Build the OpenPresenter Windows installer from Linux: cross-compile, stage, run Inno Setup under Wine.
#
#   packaging/build-installer.sh [path/to/libmpv-2.dll]
#
# Needs: cargo-xwin, wine, and Inno Setup 6 installed in a Wine prefix (ISCC.exe).
# Env: MPV_DLL (default packaging/vendor/libmpv-2.dll), WINEPREFIX, ISCC (path inside
# the prefix, default C:\inno\ISCC.exe). Output: target/installer/OpenPresenter-Setup-<ver>-<commit>.exe
set -euo pipefail
cd "$(dirname "$0")/.."

mpv_dll="${1:-${MPV_DLL:-packaging/vendor/libmpv-2.dll}}"
iscc="${ISCC:-C:\\inno\\ISCC.exe}"
target=x86_64-pc-windows-msvc

[[ -f "$mpv_dll" ]] || { echo "libmpv-2.dll not found at '$mpv_dll' (pass its path)." >&2; exit 1; }
command -v wine >/dev/null || { echo "wine isn't installed." >&2; exit 1; }

version=$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="app"))')
build=$(git describe --always --dirty --exclude '*')
if [[ "$build" == *-dirty ]]; then
  echo "warning: uncommitted changes; the installer will say '$build'." >&2
fi

cargo xwin build --release --target "$target" -p app -p pp6tool

stage=target/installer/stage
rm -rf "$stage"
mkdir -p "$stage"
cp "target/$target/release/openpresenter.exe" "target/$target/release/pp6tool.exe" "$stage/"
cp "$mpv_dll" "$stage/libmpv-2.dll"
cp packaging/THIRD-PARTY.txt "$stage/"
# CRLF so Notepad and the installer's licence page show it properly.
sed 's/$/\r/' LICENSE > "$stage/LICENSE.txt"

WINEDEBUG=-all wine "$iscc" /Q \
  "/DStage=$(winepath -w "$stage")" "/DVersion=$version" "/DBuild=$build" \
  "$(winepath -w packaging/openpresenter.iss)"

out="target/installer/OpenPresenter-Setup-$version-$build.exe"
[[ -f "$out" ]] || { echo "Inno Setup didn't produce $out" >&2; exit 1; }
sha256sum "$out"
