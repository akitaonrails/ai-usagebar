#!/usr/bin/env bash
# KDE / Linux Plasma only — not used on Windows or macOS.
#
# When: only while installing or packaging the plasmoid (called from
# ./install.sh, or by hand before kpackagetool6). Never at widget runtime,
# CLI install, or tray startup.
#
# What: point package/contents/icons/providers at the shared tray catalog
# windows/popover/src/icons/providers/ via symlink so git does not duplicate
# the SVGs. `make install-plasmoid` copies that same catalog into DESTDIR
# instead of calling this script.
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
