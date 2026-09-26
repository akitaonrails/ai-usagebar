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
if [[ -n "${AI_USAGEBAR_BIN:-}" && ( "${AI_USAGEBAR_BIN:0:1}" != / || ! -x "$AI_USAGEBAR_BIN" ) ]]; then
  echo 'AI_USAGEBAR_BIN must be an absolute path to an executable.' >&2
  exit 1
fi
if [[ -n "${AI_USAGEBAR_TUI_BIN:-}" && ( "${AI_USAGEBAR_TUI_BIN:0:1}" != / || ! -x "$AI_USAGEBAR_TUI_BIN" ) ]]; then
  echo 'AI_USAGEBAR_TUI_BIN must be an absolute path to an executable.' >&2
  exit 1
fi
if [[ ! -x "$user_home/.local/bin/ai-usagebar" && ! -x "$user_home/.cargo/bin/ai-usagebar" && ! -x /usr/local/bin/ai-usagebar && ! -x /usr/bin/ai-usagebar && -z "${AI_USAGEBAR_BIN:-}" ]]; then
  echo 'Install ai-usagebar in ~/.local/bin, ~/.cargo/bin, /usr/local/bin or /usr/bin, or set AI_USAGEBAR_BIN.' >&2
  exit 1
fi

install -Dm755 "$source_dir/ai-usagebar-tray" "$user_home/.local/bin/ai-usagebar-tray"
install -Dm644 "$source_dir/tray_model.py" "$user_home/.local/bin/tray_model.py"
for icon in "$source_dir"/../omarchy/icons/*.svg; do
  install -Dm644 "$icon" "$user_home/.local/share/ai-usagebar/tray/icons/$(basename "$icon")"
done
install -d "$user_home/.local/share/applications" "$user_home/.config/autostart"
python3 - "$source_dir" "$user_home" <<'PY'
import json
import os
import pathlib
import sys

source_dir = pathlib.Path(sys.argv[1])
user_home = pathlib.Path(sys.argv[2])
sys.path.insert(0, str(source_dir))
from tray_model import installed_binary

overrides = {name: os.environ[env] for name, env in (
    ("ai-usagebar", "AI_USAGEBAR_BIN"),
    ("ai-usagebar-tui", "AI_USAGEBAR_TUI_BIN"),
) if os.environ.get(env)}
if overrides:
    config_path = user_home / ".local/share/ai-usagebar/tray/binaries.json"
    try:
        existing = json.loads(config_path.read_text())
        if isinstance(existing, dict):
            overrides = {**existing, **overrides}
    except (OSError, ValueError):
        pass
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(json.dumps(overrides) + "\n")
    config_path.chmod(0o600)

def quote_exec(path):
    value = str(path)
    if any(ord(char) < 32 for char in value):
        raise ValueError("HOME contains a control character")
    for char in ("\\", '"', "$", "`"):
        value = value.replace(char, "\\" + char)
    return '"' + value + '"'

replacements = {
    "@TRAY_EXEC@": quote_exec(user_home / ".local/bin/ai-usagebar-tray"),
    "@TUI_EXEC@": quote_exec(installed_binary("ai-usagebar-tui", str(user_home),
                                              os.environ.get("AI_USAGEBAR_TUI_BIN"))),
}
for template, destination in (
    ("ai-usagebar.desktop", user_home / ".local/share/applications/ai-usagebar.desktop"),
    ("ai-usagebar-tray.desktop", user_home / ".config/autostart/ai-usagebar-tray.desktop"),
):
    rendered = (source_dir / template).read_text()
    for placeholder, value in replacements.items():
        rendered = rendered.replace(placeholder, value)
    destination.write_text(rendered)
    destination.chmod(0o644)
PY
install -Dm644 "$source_dir/../windows/tray-icon.png" "$user_home/.local/share/icons/hicolor/256x256/apps/ai-usagebar.png"
echo 'Installed AI Usage Bar for Linux Mint. Start ai-usagebar-tray or log in again.'
