#!/usr/bin/env bash
# Point package/contents/icons/providers at the tray/popover SVG catalog
# (windows/popover/src/icons/providers/) without copying bytes into git.
# Dev install uses a symlink; `make install-plasmoid` copies into DESTDIR.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/windows/popover/src/icons/providers"
DST="$ROOT/kde-plasmoid/package/contents/icons/providers"

if [ ! -d "$SRC" ]; then
    echo "✗ missing provider icon catalog: $SRC" >&2
    exit 1
fi

mkdir -p "$(dirname "$DST")"
rm -rf -- "$DST"
# Absolute symlink so plasmashell resolves icons after the package itself is
# symlinked into ~/.local/share/plasma/plasmoids/.
ln -sfn "$SRC" "$DST"
echo "› Provider icons → $SRC"
