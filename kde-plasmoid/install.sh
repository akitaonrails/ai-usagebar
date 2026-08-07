#!/usr/bin/env bash
# Dev install for the AI Usage Bar KDE Plasma 6 plasmoid.
# Symlinks the KPackage into ~/.local/share/plasma/plasmoids and restarts
# plasmashell. Run, then add the widget to your panel.
set -euo pipefail

ID="io.github.akitaonrails.ai-usagebar"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/package"
DEST="$HOME/.local/share/plasma/plasmoids/$ID"

if [ ! -f "$SRC/metadata.json" ]; then
    echo "✗ $SRC/metadata.json not found — run this from a checkout." >&2
    exit 1
fi

echo "› Linking $DEST → $SRC"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
ln -s "$SRC" "$DEST"

# plasmashell inherits the session PATH, not your interactive shell's, so a
# `cargo install` into ~/.cargo/bin is typically invisible to the widget. Say so
# now rather than letting it render a ⚠ with no explanation.
if ! command -v ai-usagebar >/dev/null 2>&1; then
    echo
    echo "⚠ ai-usagebar is not on PATH."
    echo "  Install it first (\`make install PREFIX=~/.local\` from the repo root),"
    echo "  or set the full path in the widget's settings — plasmashell does not"
    echo "  inherit your shell's PATH, so ~/.cargo/bin is usually not visible."
fi

echo
echo "› Restarting plasmashell…"
if systemctl --user restart plasma-plasmashell 2>/dev/null; then
    :
else
    kquitapp6 plasmashell 2>/dev/null || true
    sleep 2
    kstart plasmashell >/dev/null 2>&1 &
fi

echo
echo "✓ Installed (dev symlink)."
echo
echo "Next:"
echo "  1. Right-click the panel → \"Add or Manage Widgets…\""
echo "  2. Search for \"AI Usage\" and drag it onto the panel"
echo "  3. Settings (vendors, interval, colours):"
echo "       right-click the widget → \"Configure AI Usage Bar…\""
echo
echo "Editing the QML? The symlink means a plasmashell restart is enough:"
echo "  systemctl --user restart plasma-plasmashell"
echo
echo "Logs:  journalctl --user -f -u plasma-plasmashell"
