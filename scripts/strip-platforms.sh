#!/usr/bin/env bash
# Re-delete desktop/packaging trees that come back when merging upstream.
# This fork ships Omarchy + the Rust CLI/TUI only.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

rm -rf gnome-extension kde-plasmoid macos nix packaging
rm -f flake.nix flake.lock .envrc screenshot.png
rm -f screenshots/kde-plasmoid.png screenshots/openai-weekly-only-widget.png

echo "stripped non-Omarchy platform trees"
