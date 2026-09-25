#!/usr/bin/env bash
set -euo pipefail

source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
user_home="${HOME:?HOME is required}"

if ! command -v python3 >/dev/null || ! python3 -c 'import gi; gi.require_version("Gtk", "3.0"); from gi.repository import Gtk; assert hasattr(Gtk, "StatusIcon")' >/dev/null 2>&1; then
  echo 'Python 3 with PyGObject and GTK3 is required.' >&2
  exit 1
fi
if ! python3 -c 'import gi; gi.require_version("XApp", "1.0"); from gi.repository import XApp' >/dev/null 2>&1; then
  echo 'The Linux Mint XApp introspection library is required.' >&2
  exit 1
fi
if [[ ! -x "$user_home/.local/bin/ai-usagebar" ]]; then
  echo 'Install the official ai-usagebar binary in ~/.local/bin first.' >&2
  exit 1
fi

install -Dm755 "$source_dir/ai-usagebar-tray" "$user_home/.local/bin/ai-usagebar-tray"
install -Dm644 "$source_dir/tray_model.py" "$user_home/.local/bin/tray_model.py"
for icon in "$source_dir"/../omarchy/icons/*.svg; do
  install -Dm644 "$icon" "$user_home/.local/share/ai-usagebar/tray/icons/$(basename "$icon")"
done
install -d "$user_home/.local/share/applications" "$user_home/.config/autostart"
sed "s|@BINDIR@|$user_home/.local/bin|g" "$source_dir/ai-usagebar.desktop" > "$user_home/.local/share/applications/ai-usagebar.desktop"
sed "s|@BINDIR@|$user_home/.local/bin|g" "$source_dir/ai-usagebar-tray.desktop" > "$user_home/.config/autostart/ai-usagebar-tray.desktop"
install -Dm644 "$source_dir/../windows/tray-icon.png" "$user_home/.local/share/icons/hicolor/256x256/apps/ai-usagebar.png"
echo 'Installed AI Usage Bar for Linux Mint. Start ai-usagebar-tray or log in again.'
