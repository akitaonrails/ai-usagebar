#!/usr/bin/env bash
set -euo pipefail
source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# Shared installer validates GTK, Ayatana and binary overrides before writing.
AI_USAGEBAR_INSTALL_DESKTOP=cosmic bash "$source_dir/../linux-mint/install.sh"
install -Dm755 "$source_dir/ai-usagebar-cosmic-tray" "$HOME/.local/bin/ai-usagebar-cosmic-tray"
install -Dm644 "$source_dir/ai-usagebar-symbolic.svg" "$HOME/.local/share/ai-usagebar/tray/ai-usagebar-symbolic.svg"
echo 'Installed AI Usage Bar for COSMIC. Start ~/.local/bin/ai-usagebar-cosmic-tray or log in again.'
