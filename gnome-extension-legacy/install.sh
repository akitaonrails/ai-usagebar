#!/usr/bin/env bash
# Dev install for the AI Usage Bar GNOME Shell extension.
# Symlinks this folder into ~/.local/share/gnome-shell/extensions and
# compiles the GSettings schema. Run, then reload the shell.
set -euo pipefail

UUID="ai-usagebar@akitaonrails.github.io"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$HOME/.local/share/gnome-shell/extensions/$UUID"

echo "› Compiling schema…"
glib-compile-schemas "$SRC/schemas"

echo "› Linking $DEST → $SRC"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
ln -s "$SRC" "$DEST"

echo
echo "✓ Installed (dev symlink)."
echo
echo "Next:"
echo "  1. Reload GNOME Shell:"
echo "       • X11  → Alt+F2, type 'r', Enter"
echo "       • Wayland → log out and back in"
echo "  2. Enable it:"
echo "       gnome-extensions enable $UUID"
echo "  3. Settings (interval, bars, position):"
echo "       gnome-extensions prefs $UUID"
echo
echo "Logs:  journalctl -f -o cat /usr/bin/gnome-shell"
