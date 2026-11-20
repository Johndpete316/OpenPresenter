#!/usr/bin/env bash
# Regenerate fixtures/*.pro6 from fixtures/src/**/*.pro6.in.
#   @@RTF:name@@  -> base64 of src/text/name.rtf
#   @@TXT:name@@  -> base64 of src/text/name.txt
# The generated .pro6 files are committed; this script just keeps them readable to edit.
set -euo pipefail
cd "$(dirname "$0")"

render() {
  local in="$1" out="$2" content
  content="$(cat "$in")"
  while [[ "$content" =~ @@(RTF|TXT):([A-Za-z0-9_.-]+)@@ ]]; do
    local kind="${BASH_REMATCH[1]}" name="${BASH_REMATCH[2]}" ext b64
    ext=$([[ "$kind" == RTF ]] && echo rtf || echo txt)
    # printf %s strips nothing; files are stored without trailing newline by convention.
    b64="$(base64 -w0 < "src/text/$name.$ext")"
    content="${content//"@@$kind:$name@@"/$b64}"
  done
  printf '%s\n' "$content" > "$out"
  echo "wrote $out"
}

for f in src/*.pro6.in; do render "$f" "$(basename "${f%.in}")"; done
for f in src/invalid/*.pro6.in; do render "$f" "invalid/$(basename "${f%.in}")"; done
